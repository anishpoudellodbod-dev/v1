use chrono::{DateTime, Datelike, Utc};
pub use narayani_core::{extract_bill_vehicle, extract_string_or_int, is_bill_ignored, normalize_vehicle_number};
use narayani_core::resource::{DatabaseResource, ResourceName};
use narayani_date::{Date, DateParams, resolve_bill_fiscal_year, FiscalYearConfig};
use narayani_settings::is_displaced_tagging_enabled_in_resources;
use narayani_storage::Cache;
use narayani_vehicle_history::extract_all_identifiers_from_history;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

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
    pub fn requested_fiscal_year(&self) -> Option<&str> {
        self.fiscal_year
            .as_deref()
            .or(self.fiscal_year_camel.as_deref())
            .or(self.fiscal_year_id.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum VehicleKey {
    Canonical(usize),
    External(String),
}

pub struct VehicleIndex<'a> {
    #[allow(dead_code)]
    pub canonical_count: usize,
    pub canonical_vehicles: Vec<&'a DatabaseResource>,
    by_db_id: HashMap<i32, usize>,
    by_vehicle_id: HashMap<String, usize>,
    by_vehicle_number: HashMap<String, usize>,
    by_external_id: HashMap<String, String>,
    by_external_number: HashMap<String, String>,
}

fn extract_vehicle_fields(
    res: &DatabaseResource,
) -> (Option<i32>, Option<String>, Option<String>) {
    let v_id = res
        .inner_content
        .as_ref()
        .and_then(|inner| extract_string_or_int(inner.get("VehicleId")));
    let v_num = res
        .inner_content
        .as_ref()
        .and_then(extract_normalized_number_from_vehicle);
    (res.id, v_id, v_num)
}

impl<'a> VehicleIndex<'a> {
    #[allow(dead_code)]
    pub fn build(
        cached_vehicles: &[&'a DatabaseResource],
        db_vehicles: &[DatabaseResource],
    ) -> Self {
        Self::build_with_history(cached_vehicles, db_vehicles, &[])
    }

    pub fn build_with_history(
        cached_vehicles: &[&'a DatabaseResource],
        db_vehicles: &[DatabaseResource],
        vehicle_histories: &[&DatabaseResource],
    ) -> Self {
        let cap = cached_vehicles.len();
        let mut canonical_vehicles = Vec::with_capacity(cap);
        let mut by_db_id = HashMap::with_capacity(cap);
        let mut by_vehicle_id = HashMap::with_capacity(cap);
        let mut by_vehicle_number = HashMap::with_capacity(cap);
        let mut has_duplicates = false;

        // Fast linear pass: when cache already contains unique vehicles, avoid parsing timestamps or sorting.
        for &res in cached_vehicles {
            let (db_id, v_id, v_num) = extract_vehicle_fields(res);

            let is_duplicate = v_id
                .as_ref()
                .is_some_and(|id| by_vehicle_id.contains_key(id))
                || v_num
                    .as_ref()
                    .is_some_and(|num| by_vehicle_number.contains_key(num))
                || db_id.is_some_and(|id| by_db_id.contains_key(&id));

            if is_duplicate {
                has_duplicates = true;
                break;
            }

            let idx = canonical_vehicles.len();
            canonical_vehicles.push(res);

            if let Some(id) = db_id {
                by_db_id.insert(id, idx);
            }
            if let Some(id) = v_id {
                by_vehicle_id.insert(id, idx);
            }
            if let Some(num) = v_num {
                by_vehicle_number.insert(num, idx);
            }
        }

        if has_duplicates {
            canonical_vehicles.clear();
            by_db_id.clear();
            by_vehicle_id.clear();
            by_vehicle_number.clear();

            let mut ranked: Vec<(Option<DateTime<Utc>>, Option<i32>, &'a DatabaseResource)> =
                cached_vehicles
                    .iter()
                    .map(|&res| (extract_vehicle_created_time(res), res.id, res))
                    .collect();
            ranked.sort_unstable_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));

            for (_, _, res) in ranked {
                let (db_id, v_id, v_num) = extract_vehicle_fields(res);

                let existing_idx = v_id
                    .as_ref()
                    .and_then(|id| by_vehicle_id.get(id).copied())
                    .or_else(|| v_num.as_ref().and_then(|num| by_vehicle_number.get(num).copied()))
                    .or_else(|| db_id.and_then(|id| by_db_id.get(&id).copied()));

                let idx = if let Some(idx) = existing_idx {
                    idx
                } else {
                    let new_idx = canonical_vehicles.len();
                    canonical_vehicles.push(res);
                    new_idx
                };

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

        // Link previous DB vehicle configurations back to their canonical latest vehicle.
        for db_v in db_vehicles {
            let (db_id, v_id, v_num) = extract_vehicle_fields(db_v);

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

        let mut by_external_id = HashMap::new();
        let mut by_external_number = HashMap::new();

        // Link all historical IDs/numbers recorded in VehicleHistoryResource linked lists
        // to the same canonical vehicle (or unified external group) even if both ID & number changed.
        for hist_res in vehicle_histories {
            let Some(inner) = hist_res.inner_content.as_ref() else {
                continue;
            };
            let (h_db_ids, h_vids, h_nums) =
                extract_all_identifiers_from_history(inner);
            let matched_idx = h_db_ids
                .iter()
                .find_map(|id| by_db_id.get(id).copied())
                .or_else(|| h_vids.iter().find_map(|vid| by_vehicle_id.get(vid).copied()))
                .or_else(|| h_nums.iter().find_map(|num| by_vehicle_number.get(num).copied()));

            if let Some(idx) = matched_idx {
                for id in h_db_ids {
                    by_db_id.entry(id).or_insert(idx);
                }
                for vid in h_vids {
                    by_vehicle_id.entry(vid).or_insert(idx);
                }
                for num in h_nums {
                    by_vehicle_number.entry(num).or_insert(idx);
                }
            } else {
                let ext_group_key = inner
                    .get("vehicleUid")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|uid| format!("hist:{}", uid))
                    .or_else(|| h_vids.first().map(|id| format!("id:{}", id)))
                    .or_else(|| h_nums.first().map(|n| format!("num:{}", n)));
                if let Some(key) = ext_group_key {
                    for vid in h_vids {
                        by_external_id.entry(vid).or_insert_with(|| key.clone());
                    }
                    for num in h_nums {
                        by_external_number.entry(num).or_insert_with(|| key.clone());
                    }
                }
            }
        }

        Self {
            canonical_count: canonical_vehicles.len(),
            canonical_vehicles,
            by_db_id,
            by_vehicle_id,
            by_vehicle_number,
            by_external_id,
            by_external_number,
        }
    }

    pub fn register_bill_vehicle_with_norm(
        &mut self,
        vid_trimmed: Option<&str>,
        norm: Option<&str>,
    ) {
        let canonical_idx = norm
            .and_then(|n| self.by_vehicle_number.get(n).copied())
            .or_else(|| {
                vid_trimmed.and_then(|vid| {
                    self.by_vehicle_id
                        .get(vid)
                        .copied()
                        .or_else(|| vid.parse::<i32>().ok().and_then(|id| self.by_db_id.get(&id).copied()))
                })
            });

        if let Some(idx) = canonical_idx {
            if let Some(vid) = vid_trimmed {
                if !self.by_vehicle_id.contains_key(vid) {
                    self.by_vehicle_id.insert(vid.to_string(), idx);
                }
            }
            if let Some(n) = norm {
                if !self.by_vehicle_number.contains_key(n) {
                    self.by_vehicle_number.insert(n.to_string(), idx);
                }
            }
            return;
        }

        let key_from_num = norm.and_then(|n| self.by_external_number.get(n).cloned());
        let key_from_id = vid_trimmed.and_then(|vid| self.by_external_id.get(vid).cloned());

        let ext_key = match (key_from_num, key_from_id) {
            (Some(kn), Some(ki)) if kn != ki => {
                for v in self.by_external_number.values_mut() {
                    if *v == ki {
                        *v = kn.clone();
                    }
                }
                for v in self.by_external_id.values_mut() {
                    if *v == ki {
                        *v = kn.clone();
                    }
                }
                Some(kn)
            }
            (Some(kn), _) => Some(kn),
            (None, Some(ki)) => Some(ki),
            (None, None) => norm
                .map(|n| format!("num:{n}"))
                .or_else(|| vid_trimmed.map(|vid| format!("id:{vid}"))),
        };

        if let Some(key) = ext_key {
            if let Some(vid) = vid_trimmed {
                if !self.by_external_id.contains_key(vid) {
                    self.by_external_id.insert(vid.to_string(), key.clone());
                }
            }
            if let Some(n) = norm {
                if !self.by_external_number.contains_key(n) {
                    self.by_external_number.insert(n.to_string(), key);
                }
            }
        }
    }

    pub fn resolve(
        &self,
        vehicle_id: Option<&str>,
        vehicle_number: Option<&str>,
    ) -> Option<VehicleKey> {
        let vid_trimmed = vehicle_id.map(str::trim).filter(|s| !s.is_empty());
        let norm = vehicle_number.and_then(normalize_vehicle_number);
        self.resolve_with_norm(vid_trimmed, norm.as_deref())
    }

    pub fn resolve_with_norm(
        &self,
        vid_trimmed: Option<&str>,
        norm: Option<&str>,
    ) -> Option<VehicleKey> {
        if let Some(n) = norm {
            if let Some(&idx) = self.by_vehicle_number.get(n) {
                return Some(VehicleKey::Canonical(idx));
            }
        }

        if let Some(vid) = vid_trimmed {
            if let Some(&idx) = self.by_vehicle_id.get(vid) {
                return Some(VehicleKey::Canonical(idx));
            }
            if let Ok(db_id) = vid.parse::<i32>() {
                if let Some(&idx) = self.by_db_id.get(&db_id) {
                    return Some(VehicleKey::Canonical(idx));
                }
            }
        }

        if let Some(n) = norm {
            if let Some(ext) = self.by_external_number.get(n) {
                return Some(VehicleKey::External(ext.clone()));
            }
        }

        if let Some(vid) = vid_trimmed {
            if let Some(ext) = self.by_external_id.get(vid) {
                return Some(VehicleKey::External(ext.clone()));
            }
        }

        if let Some(n) = norm {
            return Some(VehicleKey::External(format!("num:{n}")));
        }

        vid_trimmed.map(|vid| VehicleKey::External(format!("id:{vid}")))
    }
}

pub fn is_member_record_within_fiscal_year(
    member: &DatabaseResource,
    target_group_id: usize,
    target_rank: Option<i64>,
    fy_config: &FiscalYearConfig,
) -> bool {
    let member_fy = member
        .inner_content
        .as_ref()
        .and_then(|inner| {
            extract_string_or_int(inner.get("fiscalYear"))
                .or_else(|| extract_string_or_int(inner.get("fiscalYearId")))
        })
        .or_else(|| {
            member
                .external_content
                .as_ref()
                .and_then(|ext| ext.get("createdTime"))
                .and_then(Value::as_str)
                .or_else(|| {
                    member
                        .inner_content
                        .as_ref()
                        .and_then(|inner| inner.get("taggedAt"))
                        .and_then(Value::as_str)
                })
                .and_then(narayani_date::infer_bs_fiscal_year_from_date_str)
        });

    let Some(member_fy) = member_fy else {
        return true;
    };

    let (member_gid, member_rank) = fy_config.lookup_group_and_rank(&member_fy);
    if member_gid == Some(target_group_id) {
        return true;
    }

    if let (Some(mr), Some(tr)) = (member_rank, target_rank) {
        if mr > tr {
            return false;
        }
    }

    true
}

pub fn extract_normalized_number_from_vehicle(inner: &Value) -> Option<String> {
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

fn extract_vehicle_created_time(resource: &DatabaseResource) -> Option<DateTime<Utc>> {
    let raw = resource
        .external_content
        .as_ref()
        .and_then(|ext| ext.get("createdTime"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

pub fn extract_member_timestamp(resource: &DatabaseResource) -> Option<DateTime<Utc>> {
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

#[inline]
pub fn is_member_displaced(inner: &Value) -> bool {
    inner
        .get("isDisplaced")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || inner
            .get("status")
            .and_then(Value::as_str)
            .is_some_and(|s| s.trim().eq_ignore_ascii_case("DISPLACED"))
}

fn count_displaced_vehicles(
    members: Vec<&DatabaseResource>,
    index: &VehicleIndex<'_>,
    target_group_id: usize,
    target_rank: Option<i64>,
    fy_config: &FiscalYearConfig,
) -> usize {
    if members.is_empty() {
        return 0;
    }

    let mut latest_by_vehicle: HashMap<
        VehicleKey,
        (&DatabaseResource, Option<Option<DateTime<Utc>>>, bool),
    > = HashMap::with_capacity(members.len());
    let mut displaced_count = 0usize;

    for member in members {
        if !is_member_record_within_fiscal_year(member, target_group_id, target_rank, fy_config) {
            continue;
        }
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

        let displaced = is_member_displaced(inner);
        match latest_by_vehicle.get_mut(&key) {
            None => {
                if displaced {
                    displaced_count += 1;
                }
                latest_by_vehicle.insert(key, (member, None, displaced));
            }
            Some((prev_member, prev_ts_opt, prev_displaced)) => {
                let prev_ts =
                    *prev_ts_opt.get_or_insert_with(|| extract_member_timestamp(prev_member));
                let new_ts = extract_member_timestamp(member);
                if (new_ts, member.id) > (prev_ts, prev_member.id) {
                    if displaced != *prev_displaced {
                        if displaced {
                            displaced_count += 1;
                        } else {
                            displaced_count -= 1;
                        }
                    }
                    *prev_member = member;
                    *prev_ts_opt = Some(new_ts);
                    *prev_displaced = displaced;
                }
            }
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
    let cached_vehicles: Vec<&DatabaseResource> = cache
        .read_all(ResourceName::VehicleResource)
        .unwrap_or_default();
    let vehicle_histories: Vec<&DatabaseResource> = cache
        .read_all(ResourceName::VehicleHistoryResource)
        .unwrap_or_default();

    let mut index =
        VehicleIndex::build_with_history(&cached_vehicles, db_vehicles, &vehicle_histories);

    let settings: Vec<&DatabaseResource> = cache
        .read_all(ResourceName::SettingResource)
        .unwrap_or_default();
    let mut fy_config = FiscalYearConfig::from_settings(&settings);

    let billing_rules: Vec<&DatabaseResource> = cache
        .read_all(ResourceName::BillingRuleResource)
        .unwrap_or_default();
    fy_config.apply_billing_rules(&billing_rules);

    let bills: Vec<&DatabaseResource> = cache
        .read_all(ResourceName::BillResource)
        .unwrap_or_default();

    // Single pass over bills: filter ignored entries once, normalize vehicle keys once, resolve
    // fiscal year groupings, and prepare compact tuples for metric counting.
    let mut prepared_bills: Vec<(Option<String>, Option<String>, String)> =
        Vec::with_capacity(bills.len());

    for bill in bills {
        let Some(inner) = &bill.inner_content else {
            continue;
        };
        if is_bill_ignored(inner) {
            continue;
        }

        let (v_id, v_num) = extract_bill_vehicle(inner);
        let norm_num = v_num.and_then(normalize_vehicle_number);
        if v_id.is_some() || norm_num.is_some() {
            index.register_bill_vehicle_with_norm(v_id.as_deref(), norm_num.as_deref());
        }

        let Some(resolved_bill_fy) = resolve_bill_fiscal_year(inner, &mut fy_config) else {
            continue;
        };

        if v_id.is_some() || norm_num.is_some() {
            prepared_bills.push((v_id, norm_num, resolved_bill_fy));
        }
    }

    let target_fiscal_year = fy_config.resolve_target_fiscal_year(fiscal_year);
    let (target_group_id, target_rank) = fy_config.canonicalize(&target_fiscal_year);

    let mut member_keys: HashSet<VehicleKey> = HashSet::with_capacity(prepared_bills.len());
    let mut active_in_target: HashSet<VehicleKey> = HashSet::with_capacity(prepared_bills.len());
    let mut billed_in_prior_fy: HashSet<VehicleKey> = HashSet::new();

    for (v_id, norm_num, bill_fy) in prepared_bills {
        let Some(vehicle_key) = index.resolve_with_norm(v_id.as_deref(), norm_num.as_deref()) else {
            continue;
        };

        let (bill_group_id, bill_rank) = fy_config.canonicalize(&bill_fy);
        if bill_group_id == target_group_id {
            member_keys.insert(vehicle_key.clone());
            active_in_target.insert(vehicle_key);
        } else {
            let is_prior = match (bill_rank, target_rank) {
                (Some(br), Some(tr)) => br < tr,
                _ => true,
            };
            if is_prior {
                member_keys.insert(vehicle_key.clone());
                billed_in_prior_fy.insert(vehicle_key);
            }
        }
    }

    let total_users_year = member_keys.len();
    let total_active_users_year = active_in_target.len();
    let new_users_year = active_in_target
        .iter()
        .filter(|key| !billed_in_prior_fy.contains(*key))
        .count();

    let total_inactive_users_year = member_keys
        .iter()
        .filter(|key| !active_in_target.contains(*key))
        .count();

    let members: Vec<&DatabaseResource> = cache
        .read_all(ResourceName::MemberResource)
        .unwrap_or_default();
    let displaced_users_year =
        if is_displaced_tagging_enabled_in_resources(settings.iter().copied()) {
            count_displaced_vehicles(
                members,
                &index,
                target_group_id,
                target_rank,
                &fy_config,
            )
        } else {
            0
        };

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

