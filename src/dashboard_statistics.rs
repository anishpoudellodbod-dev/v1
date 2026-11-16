use crate::cache::Cache;
use crate::date::{Date, DateParams};
use crate::resource::{DatabaseResource, ResourceName};
use crate::ServerData;
use actix_web::{web, HttpResponse};
use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashBoardStatistics {
    pub fiscal_year: String,
    pub total_users_year: usize,
    pub total_active_users_year: usize,
    pub new_users_year: usize,
    pub displaced_users_year: usize,
    pub total_inactive_users_year: usize,
    pub date: Date,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DashboardQuery {
    pub fiscal_year: Option<String>,
    #[serde(rename = "fiscalYear")]
    pub fiscal_year_camel: Option<String>,
    #[serde(rename = "fiscalYearId")]
    pub fiscal_year_id: Option<String>,
}

impl DashboardQuery {
    fn requested_fiscal_year(&self) -> Option<&str> {
        self.fiscal_year
            .as_deref()
            .or(self.fiscal_year_camel.as_deref())
            .or(self.fiscal_year_id.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum VehicleKey {
    Canonical(usize),
    External(String),
}

struct VehicleIndex {
    canonical_count: usize,
    by_db_id: HashMap<i32, usize>,
    by_vehicle_id: HashMap<String, usize>,
    by_vehicle_number: HashMap<String, usize>,
}

impl VehicleIndex {
    fn build(cached_vehicles: Vec<DatabaseResource>, db_vehicles: &[DatabaseResource]) -> Self {
        let canonical = Cache::deduplicate_vehicle_resources(cached_vehicles);
        let canonical_count = canonical.len();
        let mut by_db_id = HashMap::with_capacity(canonical_count);
        let mut by_vehicle_id = HashMap::with_capacity(canonical_count);
        let mut by_vehicle_number = HashMap::with_capacity(canonical_count);

        for (idx, res) in canonical.iter().enumerate() {
            if let Some(db_id) = res.id {
                by_db_id.entry(db_id).or_insert(idx);
            }
            if let Some(inner) = &res.inner_content {
                if let Some(v_id) = extract_string_or_int(inner.get("VehicleId")) {
                    by_vehicle_id.entry(v_id).or_insert(idx);
                }
                if let Some(v_num) = extract_normalized_number_from_vehicle(inner) {
                    by_vehicle_number.entry(v_num).or_insert(idx);
                }
            }
        }

        // Link previous DB vehicle configurations back to their canonical latest vehicle.
        for db_v in db_vehicles {
            let db_id = db_v.id;
            let v_id = db_v
                .inner_content
                .as_ref()
                .and_then(|inner| extract_string_or_int(inner.get("VehicleId")));
            let v_num = db_v
                .inner_content
                .as_ref()
                .and_then(extract_normalized_number_from_vehicle);

            let matched_idx = db_id
                .and_then(|id| by_db_id.get(&id).copied())
                .or_else(|| v_id.as_ref().and_then(|id| by_vehicle_id.get(id).copied()))
                .or_else(|| v_num.as_ref().and_then(|num| by_vehicle_number.get(num).copied()));

            if let Some(idx) = matched_idx {
                if let Some(id) = db_id {
                    by_db_id.entry(id).or_insert(idx);
                }
                if let Some(id) = v_id {
                    by_vehicle_id.entry(id).or_insert(idx);
                }
                if let Some(num) = v_num {
                    by_vehicle_number.entry(num).or_insert(idx);
                }
            }
        }

        Self {
            canonical_count,
            by_db_id,
            by_vehicle_id,
            by_vehicle_number,
        }
    }

    fn resolve(&self, vehicle_id: Option<&str>, vehicle_number: Option<&str>) -> Option<VehicleKey> {
        if let Some(vid) = vehicle_id.map(str::trim).filter(|s| !s.is_empty()) {
            if let Some(&idx) = self.by_vehicle_id.get(vid) {
                return Some(VehicleKey::Canonical(idx));
            }
            if let Ok(db_id) = vid.parse::<i32>() {
                if let Some(&idx) = self.by_db_id.get(&db_id) {
                    return Some(VehicleKey::Canonical(idx));
                }
            }
        }

        if let Some(norm) = vehicle_number.and_then(normalize_vehicle_number) {
            if let Some(&idx) = self.by_vehicle_number.get(&norm) {
                return Some(VehicleKey::Canonical(idx));
            }
            return Some(VehicleKey::External(format!("num:{norm}")));
        }

        vehicle_id
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|vid| VehicleKey::External(format!("id:{vid}")))
    }
}

struct FiscalYearConfig {
    default_fiscal_year: Option<String>,
    /// Maps any normalized fiscal year token (UUID, short label, full label) to a canonical group id.
    alias_to_group: HashMap<String, usize>,
    /// Maps a canonical group id to its display string and optional chronological order rank.
    group_display: Vec<String>,
    group_rank: Vec<Option<i64>>,
}

impl FiscalYearConfig {
    fn from_settings(settings: &[&DatabaseResource]) -> Self {
        let mut config = Self {
            default_fiscal_year: None,
            alias_to_group: HashMap::new(),
            group_display: Vec::new(),
            group_rank: Vec::new(),
        };

        for setting in settings {
            if let Some(inner) = &setting.inner_content {
                config.collect_from_value(inner);
            }
        }

        config
    }

    fn collect_from_value(&mut self, value: &Value) {
        match value {
            Value::Object(map) => {
                for key in ["currentFiscalYear", "currentFiscalYearId", "activeFiscalYear"] {
                    if let Some(val) = map.get(key).and_then(Value::as_str).map(str::trim) {
                        if !val.is_empty() && self.default_fiscal_year.is_none() {
                            self.default_fiscal_year = Some(val.to_string());
                        }
                    }
                }

                // Detect a fiscal year entry object containing id and/or label fields.
                let id_str = extract_string_or_int(map.get("id"))
                    .or_else(|| extract_string_or_int(map.get("fiscalYearId")));
                let label_str = extract_string_or_int(map.get("name"))
                    .or_else(|| extract_string_or_int(map.get("label")))
                    .or_else(|| extract_string_or_int(map.get("title")))
                    .or_else(|| extract_string_or_int(map.get("fiscalYear")))
                    .or_else(|| extract_string_or_int(map.get("year")));

                let is_fy_object = label_str
                    .as_deref()
                    .is_some_and(|l| parse_fiscal_year_start(l).is_some())
                    || map
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|t| t.eq_ignore_ascii_case("FISCAL_YEAR"))
                    || (id_str.is_some()
                        && (map.contains_key("startDate") || map.contains_key("endDate")));

                if is_fy_object {
                    let mut tokens = Vec::new();
                    if let Some(ref id) = id_str {
                        tokens.push(id.clone());
                    }
                    if let Some(ref label) = label_str {
                        tokens.push(label.clone());
                    }
                    let display = label_str.clone().or(id_str.clone());
                    if let Some(display_val) = display {
                        let is_current = map
                            .get("isCurrent")
                            .or_else(|| map.get("isActive"))
                            .and_then(Value::as_bool)
                            .unwrap_or(false);

                        if is_current && self.default_fiscal_year.is_none() {
                            self.default_fiscal_year = Some(display_val.clone());
                        }

                        self.register_group(&display_val, &tokens);
                    }
                }

                for (k, v) in map {
                    if k == "fiscalYears" || k == "fiscal_years" || v.is_array() || v.is_object() {
                        self.collect_from_value(v);
                    }
                }
            }
            Value::Array(arr) => {
                for item in arr {
                    self.collect_from_value(item);
                }
            }
            _ => {}
        }
    }

    fn register_group(&mut self, display: &str, tokens: &[String]) {
        let mut all_aliases = Vec::new();
        for token in tokens {
            all_aliases.extend(expand_fiscal_year_aliases(token));
        }

        let existing_group = all_aliases
            .iter()
            .find_map(|a| self.alias_to_group.get(a).copied());

        let group_id = match existing_group {
            Some(gid) => gid,
            None => {
                let gid = self.group_display.len();
                let rank = tokens
                    .iter()
                    .find_map(|t| parse_fiscal_year_start(t))
                    .map(|y| (y as i64) * 1000 + gid as i64);
                self.group_display.push(display.to_string());
                self.group_rank.push(rank);
                gid
            }
        };

        for alias in all_aliases {
            self.alias_to_group.insert(alias, group_id);
        }
    }

    fn canonicalize(&mut self, raw: &str) -> (usize, Option<i64>) {
        let trimmed = raw.trim();
        let aliases = expand_fiscal_year_aliases(trimmed);
        if let Some(&gid) = aliases.iter().find_map(|a| self.alias_to_group.get(a)) {
            return (gid, self.group_rank[gid]);
        }

        let gid = self.group_display.len();
        let rank = parse_fiscal_year_start(trimmed).map(|y| (y as i64) * 1000);
        self.group_display.push(trimmed.to_string());
        self.group_rank.push(rank);
        for alias in aliases {
            self.alias_to_group.insert(alias, gid);
        }
        (gid, rank)
    }

    fn resolve_target_fiscal_year(&self, requested: Option<&str>) -> String {
        if let Some(req) = requested.map(str::trim).filter(|s| !s.is_empty()) {
            let aliases = expand_fiscal_year_aliases(req);
            if let Some(&gid) = aliases.iter().find_map(|a| self.alias_to_group.get(a)) {
                if let Some(label) = self.group_display.get(gid) {
                    if parse_fiscal_year_start(req).is_some() {
                        return req.to_string();
                    }
                    return label.clone();
                }
            }
            return req.to_string();
        }

        if let Some(ref def) = self.default_fiscal_year {
            let aliases = expand_fiscal_year_aliases(def);
            if let Some(&gid) = aliases.iter().find_map(|a| self.alias_to_group.get(a)) {
                if let Some(label) = self.group_display.get(gid) {
                    return label.clone();
                }
            }
            return def.clone();
        }

        "2083/2084".to_string()
    }
}

fn parse_fiscal_year_start(raw: &str) -> Option<i32> {
    let first = raw.trim().split('/').next()?.trim();
    if first.len() == 4 && first.chars().all(|c| c.is_ascii_digit()) {
        first.parse::<i32>().ok()
    } else {
        None
    }
}

fn expand_fiscal_year_aliases(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let mut aliases = vec![trimmed.to_lowercase()];
    if let Some((start_str, end_str)) = trimmed.split_once('/') {
        let start = start_str.trim();
        let end = end_str.trim();
        if start.len() == 4 && start.chars().all(|c| c.is_ascii_digit()) {
            if let Ok(start_num) = start.parse::<i32>() {
                let next_year = start_num + 1;
                let expected_short = format!("{:02}", next_year % 100);
                let expected_full = format!("{:04}", next_year);
                if end == expected_short || end == expected_full {
                    aliases.push(format!("{start}/{expected_short}"));
                    aliases.push(format!("{start}/{expected_full}"));
                }
            }
        }
    }

    aliases.sort();
    aliases.dedup();
    aliases
}

fn extract_string_or_int(val: Option<&Value>) -> Option<String> {
    let val = val?;
    val.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .or_else(|| val.as_i64().map(|n| n.to_string()))
        .or_else(|| val.as_u64().map(|n| n.to_string()))
}

fn normalize_vehicle_number(raw: &str) -> Option<String> {
    let normalized: String = raw
        .chars()
        .filter(|c| !c.is_whitespace() && !c.is_ascii_punctuation())
        .flat_map(char::to_uppercase)
        .collect();
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn extract_normalized_number_from_vehicle(inner: &Value) -> Option<String> {
    inner
        .get("VehicleNumber")
        .and_then(Value::as_str)
        .and_then(normalize_vehicle_number)
        .or_else(|| {
            inner
                .get("NormalizedVehicleNumber")
                .and_then(Value::as_str)
                .and_then(normalize_vehicle_number)
        })
}

fn extract_member_timestamp(resource: &DatabaseResource) -> Option<DateTime<Utc>> {
    let ext_raw = resource
        .external_content
        .as_ref()
        .and_then(|ext| ext.get("createdTime"))
        .and_then(Value::as_str)
        .map(str::trim);

    let fallback_raw = resource
        .inner_content
        .as_ref()
        .and_then(|inner| inner.get("taggedAt"))
        .and_then(Value::as_str)
        .map(str::trim);

    let raw = ext_raw.or(fallback_raw)?;
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

fn is_bill_ignored(inner: &Value) -> bool {
    let Some(status) = inner.get("status").and_then(Value::as_str) else {
        return false;
    };
    let s = status.trim();
    s.eq_ignore_ascii_case("DELETED")
        || s.eq_ignore_ascii_case("AMENDED")
        || s.eq_ignore_ascii_case("ROLLED_BACK")
}

fn extract_bill_vehicle<'a>(inner: &'a Value) -> (Option<String>, Option<&'a str>) {
    let vehicle_obj = inner.get("vehicle");
    let v_id = vehicle_obj
        .and_then(|v| {
            extract_string_or_int(v.get("id"))
                .or_else(|| extract_string_or_int(v.get("vehicleId")))
                .or_else(|| extract_string_or_int(v.get("VehicleId")))
        })
        .or_else(|| extract_string_or_int(inner.get("vehicleId")));

    let v_num = vehicle_obj
        .and_then(|v| {
            v.get("vehicleNumber")
                .or_else(|| v.get("VehicleNumber"))
                .and_then(Value::as_str)
        })
        .or_else(|| inner.get("vehicleNumber").and_then(Value::as_str));

    (v_id, v_num)
}

fn count_displaced_vehicles(members: Vec<&DatabaseResource>, index: &VehicleIndex) -> usize {
    let mut member_records: Vec<&DatabaseResource> = members;
    member_records.sort_by(|a, b| {
        let time_a = extract_member_timestamp(a);
        let time_b = extract_member_timestamp(b);
        time_b.cmp(&time_a).then_with(|| b.id.cmp(&a.id))
    });

    let mut seen_vehicles: HashSet<VehicleKey> = HashSet::new();
    let mut displaced_count = 0usize;

    for member in member_records {
        let Some(inner) = &member.inner_content else {
            continue;
        };

        let v_id = extract_string_or_int(inner.get("vehicleId"))
            .or_else(|| extract_string_or_int(inner.get("VehicleId")));
        let v_num = inner
            .get("vehicleNumber")
            .or_else(|| inner.get("VehicleNumber"))
            .and_then(Value::as_str);

        let Some(key) = index.resolve(v_id.as_deref(), v_num) else {
            continue;
        };

        if !seen_vehicles.insert(key) {
            continue;
        }

        let is_displaced_flag = inner
            .get("isDisplaced")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let is_displaced_status = inner
            .get("status")
            .and_then(Value::as_str)
            .is_some_and(|s| s.trim().eq_ignore_ascii_case("DISPLACED"));

        if is_displaced_flag || is_displaced_status {
            displaced_count += 1;
        }
    }

    displaced_count
}

pub fn current_date() -> Date {
    let today = chrono::Local::now().date_naive();
    DateParams {
        year: today.year(),
        month: today.month() as i32,
        day: today.day() as i32,
    }
    .into()
}

pub fn compute_statistics(
    fiscal_year: Option<&str>,
    cache: &Cache,
    db_vehicles: &[DatabaseResource],
    current_date: Date,
) -> DashBoardStatistics {
    let cached_vehicles: Vec<DatabaseResource> = cache
        .read_all(ResourceName::VehicleResource)
        .map(|v| v.into_iter().cloned().collect())
        .unwrap_or_default();

    let index = VehicleIndex::build(cached_vehicles, db_vehicles);

    let settings: Vec<&DatabaseResource> = cache
        .read_all(ResourceName::SettingResource)
        .unwrap_or_default();
    let mut fy_config = FiscalYearConfig::from_settings(&settings);

    let target_fiscal_year = fy_config.resolve_target_fiscal_year(fiscal_year);
    let (target_group_id, target_rank) = fy_config.canonicalize(&target_fiscal_year);

    let bills: Vec<&DatabaseResource> = cache
        .read_all(ResourceName::BillResource)
        .unwrap_or_default();

    let mut active_in_target: HashSet<VehicleKey> = HashSet::new();
    let mut billed_in_prior_fy: HashSet<VehicleKey> = HashSet::new();

    for bill in bills {
        let Some(inner) = &bill.inner_content else {
            continue;
        };
        if is_bill_ignored(inner) {
            continue;
        }

        let Some(bill_fy) = extract_string_or_int(inner.get("fiscalYearId"))
            .or_else(|| extract_string_or_int(inner.get("fiscalYear")))
        else {
            continue;
        };

        let (v_id, v_num) = extract_bill_vehicle(inner);
        let Some(vehicle_key) = index.resolve(v_id.as_deref(), v_num) else {
            continue;
        };

        let (bill_group_id, bill_rank) = fy_config.canonicalize(&bill_fy);
        if bill_group_id == target_group_id {
            active_in_target.insert(vehicle_key);
        } else {
            let is_prior = match (bill_rank, target_rank) {
                (Some(br), Some(tr)) => br < tr,
                _ => true,
            };
            if is_prior {
                billed_in_prior_fy.insert(vehicle_key);
            }
        }
    }

    let total_users_year = index.canonical_count;
    let total_active_users_year = active_in_target.len();
    let new_users_year = active_in_target
        .iter()
        .filter(|key| !billed_in_prior_fy.contains(*key))
        .count();

    let active_canonical_count = active_in_target
        .iter()
        .filter(|key| matches!(key, VehicleKey::Canonical(_)))
        .count();
    let total_inactive_users_year = total_users_year.saturating_sub(active_canonical_count);

    let members: Vec<&DatabaseResource> = cache
        .read_all(ResourceName::MemberResource)
        .unwrap_or_default();
    let displaced_users_year = count_displaced_vehicles(members, &index);

    DashBoardStatistics {
        fiscal_year: target_fiscal_year,
        total_users_year,
        total_active_users_year,
        new_users_year,
        displaced_users_year,
        total_inactive_users_year,
        date: current_date,
    }
}

pub async fn get_dashboard_statistics(
    query: web::Query<DashboardQuery>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let db_vehicles = match s_data
        .db
        .read_all_resource(ResourceName::VehicleResource)
        .await
    {
        Ok(vehicles) => vehicles,
        Err(err) => {
            eprintln!("Warning: failed to read historical vehicles from DB: {err}");
            Vec::new()
        }
    };

    let cache = s_data.cache.read().await;
    let stats = compute_statistics(
        query.requested_fiscal_year(),
        &cache,
        &db_vehicles,
        current_date(),
    );
    HttpResponse::Ok().json(stats)
}

pub async fn get_dashboard_statistics_by_fy(
    path: web::Path<String>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let fy = path.into_inner();
    let requested = if fy.trim().is_empty() {
        None
    } else {
        Some(fy.as_str())
    };

    let db_vehicles = match s_data
        .db
        .read_all_resource(ResourceName::VehicleResource)
        .await
    {
        Ok(vehicles) => vehicles,
        Err(err) => {
            eprintln!("Warning: failed to read historical vehicles from DB: {err}");
            Vec::new()
        }
    };

    let cache = s_data.cache.read().await;
    let stats = compute_statistics(requested, &cache, &db_vehicles, current_date());
    HttpResponse::Ok().json(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::http::StatusCode;
    use serde_json::json;

    fn make_vehicle(
        db_id: i32,
        vehicle_id: i64,
        number: &str,
        created_time: Option<&str>,
    ) -> DatabaseResource {
        DatabaseResource {
            id: Some(db_id),
            resource_name: Some(ResourceName::VehicleResource),
            inner_content: Some(json!({
                "VehicleId": vehicle_id,
                "VehicleNumber": number,
                "NormalizedVehicleNumber": number,
                "VehicleType": "Container 10 Wheeler 17 Ton",
                "Registration": { "Status": "ACTIVE" },
                "Owners": [{ "Name": "Test Owner", "Phone": "9800000000" }]
            })),
            external_content: created_time.map(|ct| json!({ "createdTime": ct })),
        }
    }

    fn make_bill(
        db_id: i32,
        v_id: &str,
        v_num: &str,
        fiscal_year_id: &str,
        fee_title: &str,
        fee_head_id: &str,
        status: Option<&str>,
    ) -> DatabaseResource {
        let mut inner = json!({
            "items": [{
                "title": fee_title,
                "amount": 1000,
                "ruleId": fee_head_id,
                "feeHeadId": fee_head_id,
            }],
            "total": 1000,
            "vehicle": {
                "id": v_id,
                "vehicleNumber": v_num,
            },
            "billingDate": "2026-10-29",
            "billingType": "MEMBERSHIP",
            "fiscalYearId": fiscal_year_id,
        });
        if let Some(st) = status {
            inner["status"] = json!(st);
        }
        DatabaseResource {
            id: Some(db_id),
            resource_name: Some(ResourceName::BillResource),
            inner_content: Some(inner),
            external_content: None,
        }
    }

    fn make_member(
        db_id: i32,
        v_id: &str,
        v_num: &str,
        is_displaced: bool,
        status: &str,
        created_time: Option<&str>,
    ) -> DatabaseResource {
        DatabaseResource {
            id: Some(db_id),
            resource_name: Some(ResourceName::MemberResource),
            inner_content: Some(json!({
                "reason": "Bisthapit gariyeko",
                "status": status,
                "vehicleId": v_id,
                "isDisplaced": is_displaced,
                "vehicleNumber": v_num,
                "createdForDisplacement": true
            })),
            external_content: created_time.map(|ct| json!({ "createdTime": ct })),
        }
    }

    fn test_date() -> Date {
        DateParams::from("2026-11-16").into()
    }

    #[test]
    fn test_double_bill_on_same_vehicle_and_other_fee_heads_counts_once() {
        let mut cache = Cache::new();
        cache.write(
            ResourceName::VehicleResource,
            6390,
            make_vehicle(6390, 36017, "LU-548-GA-6236", Some("2026-11-05T11:09:07Z")),
        );
        // Same vehicle, different fee heads and even a slightly formatted vehicleNumber
        cache.write(
            ResourceName::BillResource,
            1,
            make_bill(1, "36017", "LU-548-GA-6236", "2083/84", "सदस्यता नविकरण शूल्क", "1", Some("PAID")),
        );
        cache.write(
            ResourceName::BillResource,
            2,
            make_bill(2, "36017", "lu 548 ga 6236", "2083/2084", "सदहितकोष शुल्क", "2", Some("PAID")),
        );
        // Soft-deleted bill on another vehicle must not count as active
        cache.write(
            ResourceName::VehicleResource,
            6391,
            make_vehicle(6391, 36018, "BA-01-PA-1111", Some("2026-11-05T11:09:07Z")),
        );
        cache.write(
            ResourceName::BillResource,
            3,
            make_bill(3, "36018", "BA-01-PA-1111", "2083/84", "Fee", "1", Some("DELETED")),
        );

        let stats = compute_statistics(Some("2083/84"), &cache, &[], test_date());
        assert_eq!(stats.fiscal_year, "2083/84");
        assert_eq!(stats.total_users_year, 2);
        assert_eq!(stats.total_active_users_year, 1);
        assert_eq!(stats.new_users_year, 1);
        assert_eq!(stats.total_inactive_users_year, 1);
        assert_eq!(stats.displaced_users_year, 0);
    }

    #[test]
    fn test_previous_db_vehicle_config_resolves_to_latest_cached_vehicle() {
        let mut cache = Cache::new();
        // Cache holds only the deduplicated latest vehicle (db_id: 6390, new number)
        let latest_vehicle = make_vehicle(6390, 36017, "LU-548-GA-6236", Some("2026-11-05T11:09:07Z"));
        cache.write(ResourceName::VehicleResource, 6390, latest_vehicle.clone());

        // Historical DB records contain an older config (db_id: 773, old number) with the same VehicleId
        let old_db_vehicle = make_vehicle(773, 36017, "OLD-LU-548-1111", None);
        let db_vehicles = vec![old_db_vehicle, latest_vehicle];

        // Bill 1 references the historical DB id "773" & old vehicle number
        cache.write(
            ResourceName::BillResource,
            10,
            make_bill(10, "773", "OLD-LU-548-1111", "2083/84", "Membership", "1", None),
        );
        // Bill 2 references the current VehicleId "36017" & current vehicle number in the same FY
        cache.write(
            ResourceName::BillResource,
            11,
            make_bill(11, "36017", "LU-548-GA-6236", "2083/84", "Other Fee", "2", None),
        );

        let stats = compute_statistics(Some("2083/84"), &cache, &db_vehicles, test_date());
        assert_eq!(stats.total_users_year, 1);
        assert_eq!(stats.total_active_users_year, 1);
        assert_eq!(stats.new_users_year, 1);
        assert_eq!(stats.total_inactive_users_year, 0);
    }

    #[test]
    fn test_new_users_year_excludes_vehicles_billed_in_previous_fiscal_year() {
        let mut cache = Cache::new();
        // SettingResource maps UUID to "2083/84"
        cache.write(
            ResourceName::SettingResource,
            1,
            DatabaseResource {
                id: Some(1),
                resource_name: Some(ResourceName::SettingResource),
                inner_content: Some(json!({
                    "fiscalYears": [
                        { "id": "fy-prev-uuid", "name": "2082/83", "isCurrent": false },
                        { "id": "15f21721-50e8-41af-9596-9d8cdb8d6a16", "name": "2083/84", "isCurrent": true }
                    ]
                })),
                external_content: None,
            },
        );

        cache.write(
            ResourceName::VehicleResource,
            1,
            make_vehicle(1, 101, "BA-01-PA-1001", Some("2026-11-01T00:00:00Z")),
        );
        cache.write(
            ResourceName::VehicleResource,
            2,
            make_vehicle(2, 102, "BA-01-PA-1002", Some("2026-11-02T00:00:00Z")),
        );
        cache.write(
            ResourceName::VehicleResource,
            3,
            make_vehicle(3, 103, "BA-01-PA-1003", Some("2026-11-03T00:00:00Z")),
        );

        // Vehicle 101 billed in 2082/83 AND 2083/84 (via UUID) -> active, NOT new
        cache.write(
            ResourceName::BillResource,
            1,
            make_bill(1, "101", "BA-01-PA-1001", "fy-prev-uuid", "Membership", "1", None),
        );
        cache.write(
            ResourceName::BillResource,
            2,
            make_bill(
                2,
                "101",
                "BA-01-PA-1001",
                "15f21721-50e8-41af-9596-9d8cdb8d6a16",
                "Membership",
                "1",
                None,
            ),
        );

        // Vehicle 102 billed ONLY in 2083/84 -> active AND new
        cache.write(
            ResourceName::BillResource,
            3,
            make_bill(3, "102", "BA-01-PA-1002", "2083/84", "Membership", "1", None),
        );

        // Vehicle 103 billed only in 2082/83 -> inactive in 2083/84
        cache.write(
            ResourceName::BillResource,
            4,
            make_bill(4, "103", "BA-01-PA-1003", "2082/83", "Membership", "1", None),
        );

        let stats = compute_statistics(None, &cache, &[], test_date());
        assert_eq!(stats.fiscal_year, "2083/84");
        assert_eq!(stats.total_users_year, 3);
        assert_eq!(stats.total_active_users_year, 2);
        assert_eq!(stats.new_users_year, 1);
        assert_eq!(stats.total_inactive_users_year, 1);
    }

    #[test]
    fn test_displaced_users_year_uses_latest_member_resource_and_missing_time_is_oldest() {
        let mut cache = Cache::new();
        cache.write(
            ResourceName::VehicleResource,
            1,
            make_vehicle(1, 36017, "LU-548-GA-6236", Some("2026-11-05T11:09:07Z")),
        );
        cache.write(
            ResourceName::VehicleResource,
            2,
            make_vehicle(2, 36018, "BA-09-KHA-5555", Some("2026-11-05T11:09:07Z")),
        );

        // Vehicle 36017: missing createdTime (oldest) says ACTIVE, latest createdTime says DISPLACED -> counted
        cache.write(
            ResourceName::MemberResource,
            1,
            make_member(1, "36017", "LU-548-GA-6236", false, "ACTIVE", None),
        );
        cache.write(
            ResourceName::MemberResource,
            2,
            make_member(
                2,
                "36017",
                "LU-548-GA-6236",
                true,
                "DISPLACED",
                Some("2026-11-16T07:12:14Z"),
            ),
        );

        // Vehicle 36018: older says DISPLACED, latest says ACTIVE (reinstated) -> NOT counted
        cache.write(
            ResourceName::MemberResource,
            3,
            make_member(
                3,
                "36018",
                "BA-09-KHA-5555",
                true,
                "DISPLACED",
                Some("2026-11-10T07:12:14Z"),
            ),
        );
        cache.write(
            ResourceName::MemberResource,
            4,
            make_member(
                4,
                "36018",
                "BA-09-KHA-5555",
                false,
                "ACTIVE",
                Some("2026-11-15T07:12:14Z"),
            ),
        );

        let stats = compute_statistics(Some("2083/84"), &cache, &[], test_date());
        assert_eq!(stats.displaced_users_year, 1);
    }

    #[actix_web::test]
    async fn test_get_dashboard_statistics_endpoint_returns_ok() {
        let (client_io, mut server_io) = tokio::io::duplex(64 * 1024);
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut startup_buf = vec![0u8; 1024];
            let _ = server_io.read(&mut startup_buf).await;
            let mut resp_buf = Vec::new();
            resp_buf.extend_from_slice(b"R\x00\x00\x00\x08\x00\x00\x00\x00");
            resp_buf.extend_from_slice(b"K\x00\x00\x00\x0c\x00\x00\x04\xd2\x12\x34\x56\x78");
            resp_buf.extend_from_slice(b"Z\x00\x00\x00\x05I");
            let _ = server_io.write_all(&resp_buf).await;
            let _ = server_io.flush().await;
            let mut buf = [0u8; 256];
            while server_io.read(&mut buf).await.unwrap_or(0) > 0 {
                // Close or respond with error so endpoint exercises fallback cleanly
                let _ = server_io
                    .write_all(b"E\x00\x00\x00\x16SERROR\x00Mmock\x00\x00Z\x00\x00\x00\x05I")
                    .await;
                let _ = server_io.flush().await;
            }
        });

        let (client, connection) = tokio_postgres::Config::new()
            .connect_raw(client_io, tokio_postgres::NoTls)
            .await
            .expect("mock postgres connect_raw failed");
        tokio::spawn(async move {
            let _ = connection.await;
        });

        let mut cache = Cache::new();
        cache.write(
            ResourceName::VehicleResource,
            6390,
            make_vehicle(6390, 36017, "LU-548-GA-6236", Some("2026-11-05T11:09:07Z")),
        );
        cache.write(
            ResourceName::BillResource,
            1,
            make_bill(1, "36017", "LU-548-GA-6236", "2083/84", "Membership", "1", None),
        );

        let s_data = web::Data::new(Arc::new(ServerData {
            db: Arc::new(crate::db::Database::new(Arc::new(client))),
            tqms_client: Arc::new(crate::tqms_integration::TQMSClient::new().await),
            cache: Arc::new(tokio::sync::RwLock::new(cache)),
        }));

        let resp = get_dashboard_statistics(
            web::Query(DashboardQuery {
                fiscal_year: Some("2083/84".to_string()),
                ..Default::default()
            }),
            s_data,
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = actix_web::body::to_bytes(resp.into_body()).await.unwrap();
        let parsed: DashBoardStatistics = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.fiscal_year, "2083/84");
        assert_eq!(parsed.total_users_year, 1);
        assert_eq!(parsed.total_active_users_year, 1);
        assert_eq!(parsed.new_users_year, 1);
        assert_eq!(parsed.total_inactive_users_year, 0);
    }
}
