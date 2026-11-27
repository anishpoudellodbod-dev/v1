use crate::dashboard::*;
use chrono::{DateTime, Utc};
use narayani_core::resource::{DatabaseResource, ResourceName};
use narayani_date::{resolve_bill_fiscal_year, Date, FiscalYearConfig};
use narayani_settings::is_displaced_tagging_enabled_in_resources;
use narayani_storage::Cache;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DashboardMembers {
    pub fiscal_year: String,
    pub total_users: Vec<DatabaseResource>,
    pub total_active_users: Vec<DatabaseResource>,
    pub new_users: Vec<DatabaseResource>,
    pub displaced_users: Vec<DatabaseResource>,
    pub inactive_year: Vec<DatabaseResource>,
    pub total_inactive_users: Vec<DatabaseResource>,
    pub date: Date,
}

fn synthesize_external_vehicle_resource(inner: &Value) -> DatabaseResource {
    let vehicle_inner = inner.get("vehicle").cloned().unwrap_or_else(|| {
        let (v_id, v_num) = extract_bill_vehicle(inner);
        serde_json::json!({
            "id": v_id,
            "vehicleNumber": v_num,
        })
    });

    DatabaseResource {
        id: None,
        resource_name: Some(ResourceName::VehicleResource),
        inner_content: Some(vehicle_inner),
        external_content: None,
    }
}

fn collect_displaced_vehicles(
    members: Vec<&DatabaseResource>,
    index: &VehicleIndex<'_>,
    target_group_id: usize,
    target_rank: Option<i64>,
    fy_config: &FiscalYearConfig,
) -> Vec<DatabaseResource> {
    if members.is_empty() {
        return Vec::new();
    }

    let mut latest_by_vehicle: HashMap<
        VehicleKey,
        (&DatabaseResource, Option<Option<DateTime<Utc>>>, bool),
    > = HashMap::with_capacity(members.len());

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
                latest_by_vehicle.insert(key, (member, None, displaced));
            }
            Some((prev_member, prev_ts_opt, prev_displaced)) => {
                let prev_ts =
                    *prev_ts_opt.get_or_insert_with(|| extract_member_timestamp(prev_member));
                let new_ts = extract_member_timestamp(member);
                if (new_ts, member.id) > (prev_ts, prev_member.id) {
                    *prev_member = member;
                    *prev_ts_opt = Some(new_ts);
                    *prev_displaced = displaced;
                }
            }
        }
    }

    let mut displaced_pairs: Vec<(VehicleKey, DatabaseResource)> = latest_by_vehicle
        .into_iter()
        .filter_map(|(key, (member, _, is_displaced))| {
            if !is_displaced {
                return None;
            }
            let resource = match &key {
                VehicleKey::Canonical(idx) => index.canonical_vehicles[*idx].clone(),
                VehicleKey::External(_) => member.clone(),
            };
            Some((key, resource))
        })
        .collect();

    displaced_pairs.sort_by(|a, b| a.0.cmp(&b.0));
    displaced_pairs.into_iter().map(|(_, res)| res).collect()
}

/// Computes the concrete vehicle/member lists corresponding to every bucket in `DashBoardStatistics`
/// for the requested (or active setting) fiscal year.
pub fn compute_members(
    fiscal_year: Option<&str>,
    cache: &Cache,
    db_vehicles: &[DatabaseResource],
    current_date: Date,
) -> DashboardMembers {
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

    let mut prepared_bills: Vec<(Option<String>, Option<String>, String, &Value)> =
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
            prepared_bills.push((v_id, norm_num, resolved_bill_fy, inner));
        }
    }

    let target_fiscal_year = fy_config.resolve_target_fiscal_year(fiscal_year);
    let (target_group_id, target_rank) = fy_config.canonicalize(&target_fiscal_year);

    let mut member_entries: HashMap<VehicleKey, DatabaseResource> =
        HashMap::with_capacity(prepared_bills.len());
    let mut active_entries: HashMap<VehicleKey, DatabaseResource> =
        HashMap::with_capacity(prepared_bills.len());
    let mut billed_in_prior_fy: HashSet<VehicleKey> = HashSet::new();

    for (v_id, norm_num, bill_fy, bill_inner) in prepared_bills {
        let Some(vehicle_key) = index.resolve_with_norm(v_id.as_deref(), norm_num.as_deref()) else {
            continue;
        };

        let (bill_group_id, bill_rank) = fy_config.canonicalize(&bill_fy);
        if bill_group_id == target_group_id {
            member_entries.entry(vehicle_key.clone()).or_insert_with(|| {
                match vehicle_key {
                    VehicleKey::Canonical(idx) => index.canonical_vehicles[idx].clone(),
                    VehicleKey::External(_) => synthesize_external_vehicle_resource(bill_inner),
                }
            });
            active_entries.entry(vehicle_key.clone()).or_insert_with(|| {
                match vehicle_key {
                    VehicleKey::Canonical(idx) => index.canonical_vehicles[idx].clone(),
                    VehicleKey::External(_) => synthesize_external_vehicle_resource(bill_inner),
                }
            });
        } else {
            let is_prior = match (bill_rank, target_rank) {
                (Some(br), Some(tr)) => br < tr,
                _ => true,
            };
            if is_prior {
                member_entries.entry(vehicle_key.clone()).or_insert_with(|| {
                    match vehicle_key {
                        VehicleKey::Canonical(idx) => index.canonical_vehicles[idx].clone(),
                        VehicleKey::External(_) => synthesize_external_vehicle_resource(bill_inner),
                    }
                });
                billed_in_prior_fy.insert(vehicle_key);
            }
        }
    }

    let mut member_pairs: Vec<(VehicleKey, DatabaseResource)> = member_entries.into_iter().collect();
    member_pairs.sort_by(|a, b| a.0.cmp(&b.0));

    let mut active_pairs: Vec<(VehicleKey, DatabaseResource)> = active_entries.into_iter().collect();
    active_pairs.sort_by(|a, b| a.0.cmp(&b.0));

    let active_keys: HashSet<VehicleKey> = active_pairs.iter().map(|(k, _)| k.clone()).collect();

    let inactive_year: Vec<DatabaseResource> = member_pairs
        .iter()
        .filter_map(|(key, res)| {
            if !active_keys.contains(key) {
                Some(res.clone())
            } else {
                None
            }
        })
        .collect();

    let total_users: Vec<DatabaseResource> =
        member_pairs.into_iter().map(|(_, res)| res).collect();

    let new_users: Vec<DatabaseResource> = active_pairs
        .iter()
        .filter_map(|(key, res)| {
            if !billed_in_prior_fy.contains(key) {
                Some(res.clone())
            } else {
                None
            }
        })
        .collect();

    let total_active_users: Vec<DatabaseResource> =
        active_pairs.into_iter().map(|(_, res)| res).collect();

    let members: Vec<&DatabaseResource> = cache
        .read_all(ResourceName::MemberResource)
        .unwrap_or_default();
    let displaced_users =
        if is_displaced_tagging_enabled_in_resources(settings.iter().copied()) {
            collect_displaced_vehicles(
                members,
                &index,
                target_group_id,
                target_rank,
                &fy_config,
            )
        } else {
            Vec::new()
        };
    let total_inactive_users = inactive_year.clone();

    DashboardMembers {
        fiscal_year: target_fiscal_year,
        total_users,
        total_active_users,
        new_users,
        displaced_users,
        inactive_year,
        total_inactive_users,
        date: current_date,
    }
}

