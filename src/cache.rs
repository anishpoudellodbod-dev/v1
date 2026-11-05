use crate::resource::{DatabaseResource, ResourceName};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Default)]
pub struct Cache {
    pub cache: HashMap<ResourceName, HashMap<i32, DatabaseResource>>,
}

impl Cache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn read(&self, resource_name: ResourceName, id: i32) -> Option<DatabaseResource> {
        self.cache.get(&resource_name)?.get(&id).cloned()
    }

    pub fn read_all(&self, resource_name: ResourceName) -> Result<Vec<&DatabaseResource>, ()> {
        self.cache
            .get(&resource_name)
            .map(|resources| resources.values().collect())
            .ok_or(())
    }

    /// Recursively traverses the JSON tree and checks the predicate in a single pass,
    /// eliminating all intermediate `Vec<&Value>` heap allocations.
    fn matches_path(value: &Value, path: &[&str], query_lower: &str) -> bool {
        let Some((&current, rest)) = path.split_first() else {
            return Self::value_contains(value, query_lower);
        };

        match value {
            Value::Object(map) => {
                if let Some(v) = map.get(current) {
                    Self::matches_path(v, rest, query_lower)
                } else {
                    map.values()
                        .any(|v| Self::matches_path(v, path, query_lower))
                }
            }
            Value::Array(arr) => {
                if let Ok(index) = current.parse::<usize>() {
                    arr.get(index)
                        .is_some_and(|v| Self::matches_path(v, rest, query_lower))
                } else {
                    arr.iter()
                        .any(|v| Self::matches_path(v, path, query_lower))
                }
            }
            _ => false,
        }
    }

    fn value_contains(value: &Value, search_lower: &str) -> bool {
        match value {
            Value::String(s) => {
                // Fast-path: zero-allocation check when string is ASCII
                if s.is_ascii() {
                    s.as_bytes()
                        .windows(search_lower.len())
                        .any(|w| w.eq_ignore_ascii_case(search_lower.as_bytes()))
                } else {
                    s.to_lowercase().contains(search_lower)
                }
            }
            Value::Array(arr) => arr.iter().any(|v| Self::value_contains(v, search_lower)),
            Value::Object(map) => map.values().any(|v| Self::value_contains(v, search_lower)),
            // Numbers (including scientific notation like 1e5) and booleans ("true"/"false")
            // are strictly ASCII or already lowercase.
            Value::Number(n) => n
                .to_string()
                .as_bytes()
                .windows(search_lower.len())
                .any(|w| w.eq_ignore_ascii_case(search_lower.as_bytes())),
            Value::Bool(b) => {
                let s = if *b { "true" } else { "false" };
                s.contains(search_lower)
            }
            Value::Null => false,
        }
    }

    /// Shared helper to deduplicate filtering logic between batch and standard searches.
    fn filter_matching<'a, 'b>(
        &'a self,
        resource_name: &ResourceName,
        field_name: &'b str,
        search_query: &str,
    ) -> Box<dyn Iterator<Item = &'a DatabaseResource> + 'b>
    where
        'a: 'b,
    {
        let items = self
            .cache
            .get(resource_name)
            .into_iter()
            .flat_map(|resources| resources.values());

        // Fast-path: if search query is empty, return all cached items
        if search_query.trim().is_empty() {
            return Box::new(items);
        }

        let query_lower = search_query.to_lowercase();
        let path: Vec<&'b str> = field_name.split('.').collect();

        Box::new(items.filter(move |resource| {
            resource
                .inner_content
                .as_ref()
                .is_some_and(|inner| Self::matches_path(inner, &path, &query_lower))
        }))
    }
    pub fn search_innercontent_batch(
        &self,
        resource_name: ResourceName,
        field_name: &str,
        search_query: &str,
        limit: usize,
        starting_from: usize,
    ) -> Vec<DatabaseResource> {
        if search_query.is_empty() {
            return self
                .cache
                .get(&resource_name)
                .into_iter()
                .flat_map(|r| r.values())
                .skip(starting_from)
                .take(limit)
                .cloned()
                .collect();
        }

        self.filter_matching(&resource_name, field_name, search_query)
            .skip(starting_from)
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn search_innercontent(
        &self,
        resource_name: ResourceName,
        field_name: &str,
        search_query: &str,
        limit: usize,
    ) -> (Vec<DatabaseResource>, usize) {
        let total_available = self
            .cache
            .get(&resource_name)
            .map_or(0, |m| m.len());

        // Cap initial allocation to avoid overflow when limit is usize::MAX / very large
        let initial_capacity = limit.min(total_available);
        let mut matches = Vec::with_capacity(initial_capacity);
        let mut found_count = 0;

        for resource in self.filter_matching(&resource_name, field_name, search_query) {
            found_count += 1;
            if matches.len() < limit {
                matches.push(resource.clone());
            }
        }

        (matches, found_count)
    }

    pub fn write(&mut self, resource_name: ResourceName, id: i32, resource: DatabaseResource) {
        self.cache.entry(resource_name).or_default().insert(id, resource);
    }

    pub async fn load_from_db(&mut self, db: &crate::db::Database) {
        let resource_names: Vec<_> = db
            .resource_name
            .iter()
            .filter(|&&ref r| *r != ResourceName::UserResource)
            .cloned()
            .collect();

        let total = resource_names.len();
        println!("Loading cache {total}...");

        for (i, resource_name) in resource_names.into_iter().enumerate() {
            match db.read_all_resource(resource_name.clone()).await {
                Ok(resources) => {
                    // Single outer HashMap lookup and pre-allocation per resource type
                    let entry = self
                        .cache
                        .entry(resource_name.clone())
                        .or_insert_with(|| HashMap::with_capacity(resources.len()));

                    for resource in resources {
                        if let Some(id) = resource.id {
                            entry.insert(id, resource);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error loading cache for resource {:?}: {e}", resource_name);
                }
            }
            println!(
                "Loaded cache for resource {:?}. Remaining: {}",
                resource_name,
                total - 1 - i
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_sample_vehicle(id: i32, number: &str, owner_name: &str, vehicle_id: i64) -> DatabaseResource {
        DatabaseResource {
            id: Some(id),
            resource_name: Some(ResourceName::VehicleResource),
            inner_content: Some(serde_json::json!({
                "VehicleNumber": number,
                "VehicleId": vehicle_id,
                "Registration": {
                    "Status": "ACTIVE"
                },
                "Owners": [
                    {
                        "Name": owner_name,
                        "Type": "Individual",
                        "IsActive": true
                    }
                ]
            })),
            external_content: None,
        }
    }

    #[test]
    fn test_cache_new_and_empty() {
        let cache = Cache::new();
        assert_eq!(cache.read(ResourceName::VehicleResource, 1), None);
        assert_eq!(cache.read_all(ResourceName::VehicleResource), Err(()));
    }

    #[test]
    fn test_cache_write_and_read() {
        let mut cache = Cache::new();
        let res = make_sample_vehicle(1, "BA-01-PA-1234", "Hari Bahadur", 1001);

        cache.write(ResourceName::VehicleResource, 1, res.clone());
        let fetched = cache.read(ResourceName::VehicleResource, 1);
        assert_eq!(fetched, Some(res));

        // Non-existent id
        assert_eq!(cache.read(ResourceName::VehicleResource, 999), None);
        // Non-existent resource name
        assert_eq!(cache.read(ResourceName::BillResource, 1), None);
    }

    #[test]
    fn test_cache_write_overwrite() {
        let mut cache = Cache::new();
        let res1 = make_sample_vehicle(1, "OLD-NUMBER", "Hari", 1001);
        let res2 = make_sample_vehicle(1, "NEW-NUMBER", "Hari Bahadur", 1001);

        cache.write(ResourceName::VehicleResource, 1, res1);
        cache.write(ResourceName::VehicleResource, 1, res2.clone());

        assert_eq!(cache.read(ResourceName::VehicleResource, 1), Some(res2));
    }

    #[test]
    fn test_cache_read_all() {
        let mut cache = Cache::new();
        let v1 = make_sample_vehicle(1, "V1", "Owner 1", 101);
        let v2 = make_sample_vehicle(2, "V2", "Owner 2", 102);
        cache.write(ResourceName::VehicleResource, 1, v1);
        cache.write(ResourceName::VehicleResource, 2, v2);

        let all = cache.read_all(ResourceName::VehicleResource).expect("read_all failed");
        assert_eq!(all.len(), 2);

        // Another resource category is still empty
        assert_eq!(cache.read_all(ResourceName::UserResource), Err(()));
    }

    #[test]
    fn test_cache_search_flat_field() {
        let mut cache = Cache::new();
        cache.write(ResourceName::VehicleResource, 1, make_sample_vehicle(1, "BA-01-PA-1111", "Ram", 101));
        cache.write(ResourceName::VehicleResource, 2, make_sample_vehicle(2, "BA-02-PA-2222", "Shyam", 102));

        let (matches, count) = cache.search_innercontent(ResourceName::VehicleResource, "VehicleNumber", "1111", 10);
        assert_eq!(count, 1);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].id, Some(1));
    }

    #[test]
    fn test_cache_search_nested_field() {
        let mut cache = Cache::new();
        cache.write(ResourceName::VehicleResource, 1, make_sample_vehicle(1, "V1", "Ram", 101));

        let (matches, count) = cache.search_innercontent(ResourceName::VehicleResource, "Registration.Status", "active", 10);
        assert_eq!(count, 1);
        assert_eq!(matches.len(), 1);

        let (matches_none, count_none) = cache.search_innercontent(ResourceName::VehicleResource, "Registration.Status", "expired", 10);
        assert_eq!(count_none, 0);
        assert_eq!(matches_none.len(), 0);
    }

    #[test]
    fn test_cache_search_array_of_objects() {
        let mut cache = Cache::new();
        cache.write(ResourceName::VehicleResource, 1, make_sample_vehicle(1, "V1", "Kiran Shrestha", 101));
        cache.write(ResourceName::VehicleResource, 2, make_sample_vehicle(2, "V2", "Sita Sharma", 102));

        let (matches, count) = cache.search_innercontent(ResourceName::VehicleResource, "Owners.Name", "shrestha", 10);
        assert_eq!(count, 1);
        assert_eq!(matches[0].id, Some(1));
    }

    #[test]
    fn test_cache_search_array_indexed_path() {
        let mut cache = Cache::new();
        cache.write(ResourceName::VehicleResource, 1, make_sample_vehicle(1, "V1", "Kiran Shrestha", 101));

        let (matches, count) = cache.search_innercontent(ResourceName::VehicleResource, "Owners.0.Name", "Kiran", 10);
        assert_eq!(count, 1);
        assert_eq!(matches.len(), 1);

        // Index 1 does not exist
        let (matches_idx1, count_idx1) = cache.search_innercontent(ResourceName::VehicleResource, "Owners.1.Name", "Kiran", 10);
        assert_eq!(count_idx1, 0);
        assert_eq!(matches_idx1.len(), 0);
    }

    #[test]
    fn test_cache_search_number_and_boolean() {
        let mut cache = Cache::new();
        cache.write(ResourceName::VehicleResource, 1, make_sample_vehicle(1, "V1", "Ram", 554433));

        // Search number
        let (matches_num, count_num) = cache.search_innercontent(ResourceName::VehicleResource, "VehicleId", "554433", 10);
        assert_eq!(count_num, 1);
        assert_eq!(matches_num.len(), 1);

        // Search boolean
        let (matches_bool, count_bool) = cache.search_innercontent(ResourceName::VehicleResource, "Owners.IsActive", "true", 10);
        assert_eq!(count_bool, 1);
        assert_eq!(matches_bool.len(), 1);
    }

    #[test]
    fn test_cache_search_empty_query_returns_all() {
        let mut cache = Cache::new();
        cache.write(ResourceName::VehicleResource, 1, make_sample_vehicle(1, "V1", "Ram", 101));
        cache.write(ResourceName::VehicleResource, 2, make_sample_vehicle(2, "V2", "Shyam", 102));

        let (matches, count) = cache.search_innercontent(ResourceName::VehicleResource, "VehicleNumber", "", 10);
        assert_eq!(count, 2);
        assert_eq!(matches.len(), 2);

        let (matches_whitespace, count_ws) = cache.search_innercontent(ResourceName::VehicleResource, "VehicleNumber", "   ", 10);
        assert_eq!(count_ws, 2);
        assert_eq!(matches_whitespace.len(), 2);
    }

    #[test]
    fn test_cache_search_limit_capping() {
        let mut cache = Cache::new();
        for i in 1..=5 {
            cache.write(ResourceName::VehicleResource, i, make_sample_vehicle(i, &format!("VEHICLE-{i}"), "Same Owner", 100 + i as i64));
        }

        // Limit = 2, total matches = 5
        let (matches, count) = cache.search_innercontent(ResourceName::VehicleResource, "Owners.Name", "Same Owner", 2);
        assert_eq!(count, 5);
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn test_cache_search_batch_pagination() {
        let mut cache = Cache::new();
        for i in 1..=10 {
            cache.write(ResourceName::VehicleResource, i, make_sample_vehicle(i, &format!("BA-TEST-{i}"), "Common", 200 + i as i64));
        }

        // Page 1: limit 3, starting_from 0
        let page1 = cache.search_innercontent_batch(ResourceName::VehicleResource, "Owners.Name", "Common", 3, 0);
        assert_eq!(page1.len(), 3);

        // Page 2: limit 3, starting_from 3
        let page2 = cache.search_innercontent_batch(ResourceName::VehicleResource, "Owners.Name", "Common", 3, 3);
        assert_eq!(page2.len(), 3);

        // Beyond items: starting_from 15
        let beyond = cache.search_innercontent_batch(ResourceName::VehicleResource, "Owners.Name", "Common", 3, 15);
        assert_eq!(beyond.len(), 0);

        // Empty query batch pagination
        let page_empty = cache.search_innercontent_batch(ResourceName::VehicleResource, "Owners.Name", "", 4, 0);
        assert_eq!(page_empty.len(), 4);
    }
}
