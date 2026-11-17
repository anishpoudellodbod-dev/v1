use crate::cache::Cache;
use crate::dashboard_statistics::{
    current_date, extract_bill_inferred_fiscal_year, extract_bill_period_start_fiscal_year,
    extract_bill_vehicle, extract_member_timestamp, extract_string_or_int, get_cached_db_vehicles,
    is_bill_ignored, is_member_displaced, normalize_vehicle_number, parse_fiscal_year_start,
    DashboardQuery, FiscalYearConfig, VehicleIndex, VehicleKey,
};
use crate::date::Date;
use crate::resource::{DatabaseResource, ResourceName};
use crate::ServerData;
use actix_web::{web, HttpResponse};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

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
) -> Vec<DatabaseResource> {
    if members.is_empty() {
        return Vec::new();
    }

    let mut latest_by_vehicle: HashMap<
        VehicleKey,
        (&DatabaseResource, Option<Option<DateTime<Utc>>>, bool),
    > = HashMap::with_capacity(members.len());

    for member in members {
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

    let mut index = VehicleIndex::build(&cached_vehicles, db_vehicles);

    let settings: Vec<&DatabaseResource> = cache
        .read_all(ResourceName::SettingResource)
        .unwrap_or_default();
    let mut fy_config = FiscalYearConfig::from_settings(&settings);

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
        index.register_bill_vehicle_with_norm(v_id.as_deref(), norm_num.as_deref());

        let bill_fy_raw = extract_string_or_int(inner.get("fiscalYearId"))
            .or_else(|| extract_string_or_int(inner.get("fiscalYear")));

        let resolved_bill_fy = if let Some(period_fy) = extract_bill_period_start_fiscal_year(inner) {
            if let Some(fy) = bill_fy_raw {
                let conflicting_setting = parse_fiscal_year_start(&period_fy).is_some_and(|y| {
                    fy_config
                        .setting_year_to_id
                        .get(&y)
                        .is_some_and(|sid| !sid.eq_ignore_ascii_case(&fy))
                });
                if !conflicting_setting {
                    fy_config.register_group(&period_fy, &[fy, period_fy.clone()]);
                } else {
                    fy_config.register_group(&period_fy, &[period_fy.clone()]);
                }
            } else {
                fy_config.register_group(&period_fy, &[period_fy.clone()]);
            }
            period_fy
        } else if let Some(bill_fy) = bill_fy_raw {
            if parse_fiscal_year_start(&bill_fy).is_none() && !fy_config.has_ranked_group(&bill_fy) {
                if let Some(inferred_fy) = extract_bill_inferred_fiscal_year(inner) {
                    let conflicting_setting = parse_fiscal_year_start(&inferred_fy).is_some_and(|y| {
                        fy_config
                            .setting_year_to_id
                            .get(&y)
                            .is_some_and(|sid| !sid.eq_ignore_ascii_case(&bill_fy))
                    });
                    if !conflicting_setting {
                        fy_config.register_group(&inferred_fy, &[bill_fy.clone(), inferred_fy.clone()]);
                    }
                }
            }
            bill_fy
        } else {
            let Some(inferred_fy) = extract_bill_inferred_fiscal_year(inner) else {
                continue;
            };
            inferred_fy
        };

        if v_id.is_some() || norm_num.is_some() {
            prepared_bills.push((v_id, norm_num, resolved_bill_fy, inner));
        }
    }

    let target_fiscal_year = fy_config.resolve_target_fiscal_year(fiscal_year);
    let (target_group_id, target_rank) = fy_config.canonicalize(&target_fiscal_year);

    let mut active_entries: HashMap<VehicleKey, DatabaseResource> =
        HashMap::with_capacity(prepared_bills.len());
    let mut billed_in_prior_fy: HashSet<VehicleKey> = HashSet::new();

    for (v_id, norm_num, bill_fy, bill_inner) in prepared_bills {
        let Some(vehicle_key) = index.resolve_with_norm(v_id.as_deref(), norm_num.as_deref()) else {
            continue;
        };

        let (bill_group_id, bill_rank) = fy_config.canonicalize(&bill_fy);
        if bill_group_id == target_group_id {
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
                billed_in_prior_fy.insert(vehicle_key);
            }
        }
    }

    let total_users: Vec<DatabaseResource> = index
        .canonical_vehicles
        .iter()
        .map(|&res| res.clone())
        .collect();

    let mut active_pairs: Vec<(VehicleKey, DatabaseResource)> = active_entries.into_iter().collect();
    active_pairs.sort_by(|a, b| a.0.cmp(&b.0));

    let active_canonical_indices: HashSet<usize> = active_pairs
        .iter()
        .filter_map(|(key, _)| match key {
            VehicleKey::Canonical(idx) => Some(*idx),
            VehicleKey::External(_) => None,
        })
        .collect();

    let inactive_year: Vec<DatabaseResource> = index
        .canonical_vehicles
        .iter()
        .enumerate()
        .filter_map(|(idx, &res)| {
            if !active_canonical_indices.contains(&idx) {
                Some(res.clone())
            } else {
                None
            }
        })
        .collect();

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
    let displaced_users = collect_displaced_vehicles(members, &index);
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

async fn load_members_for_fy(
    requested_fy: Option<&str>,
    s_data: &Arc<ServerData>,
) -> DashboardMembers {
    let db_vehicles = get_cached_db_vehicles(s_data).await;
    let cache = s_data.cache.read().await;
    compute_members(requested_fy, &cache, &db_vehicles, current_date())
}

fn normalize_path_fy(path: web::Path<String>) -> Option<String> {
    let trimmed = path.into_inner().trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

async fn respond_member_list(
    requested_fy: Option<&str>,
    s_data: &Arc<ServerData>,
    select: fn(&DashboardMembers) -> &[DatabaseResource],
) -> HttpResponse {
    let members = load_members_for_fy(requested_fy, s_data).await;
    HttpResponse::Ok().json(select(&members))
}

pub async fn get_members(
    query: web::Query<DashboardQuery>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let members = load_members_for_fy(query.requested_fiscal_year(), &s_data).await;
    HttpResponse::Ok().json(members)
}

pub async fn get_members_by_fy(
    path: web::Path<String>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let fy = normalize_path_fy(path);
    let members = load_members_for_fy(fy.as_deref(), &s_data).await;
    HttpResponse::Ok().json(members)
}

pub async fn get_total_users(
    query: web::Query<DashboardQuery>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    respond_member_list(query.requested_fiscal_year(), &s_data, |m| &m.total_users).await
}

pub async fn get_total_users_by_fy(
    path: web::Path<String>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let fy = normalize_path_fy(path);
    respond_member_list(fy.as_deref(), &s_data, |m| &m.total_users).await
}

pub async fn get_total_active_users(
    query: web::Query<DashboardQuery>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    respond_member_list(query.requested_fiscal_year(), &s_data, |m| &m.total_active_users).await
}

pub async fn get_total_active_users_by_fy(
    path: web::Path<String>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let fy = normalize_path_fy(path);
    respond_member_list(fy.as_deref(), &s_data, |m| &m.total_active_users).await
}

pub async fn get_new_users(
    query: web::Query<DashboardQuery>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    respond_member_list(query.requested_fiscal_year(), &s_data, |m| &m.new_users).await
}

pub async fn get_new_users_by_fy(
    path: web::Path<String>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let fy = normalize_path_fy(path);
    respond_member_list(fy.as_deref(), &s_data, |m| &m.new_users).await
}

pub async fn get_displaced_users(
    query: web::Query<DashboardQuery>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    respond_member_list(query.requested_fiscal_year(), &s_data, |m| &m.displaced_users).await
}

pub async fn get_displaced_users_by_fy(
    path: web::Path<String>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let fy = normalize_path_fy(path);
    respond_member_list(fy.as_deref(), &s_data, |m| &m.displaced_users).await
}

pub async fn get_inactive_year(
    query: web::Query<DashboardQuery>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    respond_member_list(query.requested_fiscal_year(), &s_data, |m| &m.inactive_year).await
}

pub async fn get_inactive_year_by_fy(
    path: web::Path<String>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let fy = normalize_path_fy(path);
    respond_member_list(fy.as_deref(), &s_data, |m| &m.inactive_year).await
}

pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.route("", web::get().to(get_members))
        .route("/total_users", web::get().to(get_total_users))
        .route("/total_users/{fiscal_year}", web::get().to(get_total_users_by_fy))
        .route("/total", web::get().to(get_total_users))
        .route("/total/{fiscal_year}", web::get().to(get_total_users_by_fy))
        .route("/total_active_users", web::get().to(get_total_active_users))
        .route(
            "/total_active_users/{fiscal_year}",
            web::get().to(get_total_active_users_by_fy),
        )
        .route("/active", web::get().to(get_total_active_users))
        .route("/active/{fiscal_year}", web::get().to(get_total_active_users_by_fy))
        .route("/new_users", web::get().to(get_new_users))
        .route("/new_users/{fiscal_year}", web::get().to(get_new_users_by_fy))
        .route("/new", web::get().to(get_new_users))
        .route("/new/{fiscal_year}", web::get().to(get_new_users_by_fy))
        .route("/displaced_users", web::get().to(get_displaced_users))
        .route(
            "/displaced_users/{fiscal_year}",
            web::get().to(get_displaced_users_by_fy),
        )
        .route("/displaced", web::get().to(get_displaced_users))
        .route("/displaced/{fiscal_year}", web::get().to(get_displaced_users_by_fy))
        .route("/inactive_year", web::get().to(get_inactive_year))
        .route("/inactive_year/{fiscal_year}", web::get().to(get_inactive_year_by_fy))
        .route("/total_inactive_users", web::get().to(get_inactive_year))
        .route(
            "/total_inactive_users/{fiscal_year}",
            web::get().to(get_inactive_year_by_fy),
        )
        .route("/inactive", web::get().to(get_inactive_year))
        .route("/inactive/{fiscal_year}", web::get().to(get_inactive_year_by_fy))
        .route("/{fiscal_year}", web::get().to(get_members_by_fy));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dashboard_statistics::compute_statistics;
    use crate::date::DateParams;
    use actix_web::{http::StatusCode, test as actix_test, App};
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
                "VehicleType": "Container",
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
        period_start: Option<&str>,
        status: Option<&str>,
    ) -> DatabaseResource {
        let mut inner = json!({
            "items": [{ "title": "Membership", "amount": 1000, "feeHeadId": "3" }],
            "total": 1000,
            "vehicle": { "id": v_id, "vehicleNumber": v_num },
            "billingDate": "2026-11-16",
            "fiscalYearId": fiscal_year_id
        });
        if let Some(ps) = period_start {
            inner["periodStart"] = json!(ps);
        }
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
                "vehicleId": v_id,
                "vehicleNumber": v_num,
                "isDisplaced": is_displaced,
                "status": status
            })),
            external_content: created_time.map(|ct| json!({ "createdTime": ct })),
        }
    }

    fn build_sample_cache() -> Cache {
        let mut cache = Cache::new();
        cache.write(
            ResourceName::SettingResource,
            2,
            DatabaseResource {
                id: Some(2),
                resource_name: Some(ResourceName::SettingResource),
                inner_content: Some(json!({
                    "key": "fiscal-year-management",
                    "fiscalYears": [
                        {
                            "id": "15f21721-50e8-41af-9596-9d8cdb8d6a16",
                            "code": "2082/83",
                            "name": "2082/83",
                            "status": "ACTIVE",
                            "startDate": "2025-07-17",
                            "endDate": "2026-07-16"
                        }
                    ]
                })),
                external_content: None,
            },
        );

        // Vehicle 101: billed in 2081/82 AND 2083/84 -> active, NOT new, NOT inactive
        cache.write(
            ResourceName::VehicleResource,
            1,
            make_vehicle(1, 101, "BA-01-PA-1001", Some("2026-11-01T00:00:00Z")),
        );
        // Duplicate stale version of Vehicle 101 (no createdTime -> oldest) must be deduplicated
        cache.write(
            ResourceName::VehicleResource,
            10,
            make_vehicle(10, 101, "OLD-BA-1001", None),
        );

        // Vehicle 102: billed ONLY in 2083/84 via UUID + periodStart -> active AND new
        cache.write(
            ResourceName::VehicleResource,
            2,
            make_vehicle(2, 102, "BA-01-PA-1002", Some("2026-11-02T00:00:00Z")),
        );

        // Vehicle 103: only cancelled bill in 2083/84 -> inactive
        cache.write(
            ResourceName::VehicleResource,
            3,
            make_vehicle(3, 103, "BA-01-PA-1003", Some("2026-11-03T00:00:00Z")),
        );

        // Vehicle 104: no bills, latest MemberResource says DISPLACED -> inactive AND displaced
        cache.write(
            ResourceName::VehicleResource,
            4,
            make_vehicle(4, 104, "BA-01-PA-1004", Some("2026-11-04T00:00:00Z")),
        );

        // Bills
        cache.write(
            ResourceName::BillResource,
            1,
            make_bill(1, "101", "BA-01-PA-1001", "2081/82", Some("2024-11-01"), None),
        );
        cache.write(
            ResourceName::BillResource,
            2,
            make_bill(
                2,
                "101",
                "BA-01-PA-1001",
                "15f21721-50e8-41af-9596-9d8cdb8d6a16",
                Some("2026-11-16"),
                Some("PAID"),
            ),
        );
        cache.write(
            ResourceName::BillResource,
            3,
            make_bill(
                3,
                "102",
                "BA-01-PA-1002",
                "15f21721-50e8-41af-9596-9d8cdb8d6a16",
                Some("2026-11-16"),
                Some("PAID"),
            ),
        );
        // External uncached vehicle 999 billed in 2083/84 -> active AND new
        cache.write(
            ResourceName::BillResource,
            4,
            make_bill(4, "999", "NA-09-KHA-9999", "2083/84", Some("2026-11-16"), Some("PAID")),
        );
        // Ignored cancelled bill for Vehicle 103
        cache.write(
            ResourceName::BillResource,
            5,
            make_bill(
                5,
                "103",
                "BA-01-PA-1003",
                "2083/84",
                Some("2026-11-16"),
                Some("CANCELLED"),
            ),
        );

        // MemberResource records
        // Vehicle 104: old ACTIVE, newer DISPLACED -> counted in displaced_users
        cache.write(
            ResourceName::MemberResource,
            1,
            make_member(1, "104", "BA-01-PA-1004", false, "ACTIVE", Some("2026-11-01T00:00:00Z")),
        );
        cache.write(
            ResourceName::MemberResource,
            2,
            make_member(2, "104", "BA-01-PA-1004", true, "DISPLACED", Some("2026-11-10T00:00:00Z")),
        );
        // Vehicle 103: old DISPLACED, newer ACTIVE -> NOT displaced
        cache.write(
            ResourceName::MemberResource,
            3,
            make_member(3, "103", "BA-01-PA-1003", true, "DISPLACED", Some("2026-11-01T00:00:00Z")),
        );
        cache.write(
            ResourceName::MemberResource,
            4,
            make_member(4, "103", "BA-01-PA-1003", false, "ACTIVE", Some("2026-11-12T00:00:00Z")),
        );

        cache
    }

    async fn create_mock_server_data(cache: Cache) -> web::Data<Arc<ServerData>> {
        let (client_io, mut server_io) = tokio::io::duplex(65536);
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buf = [0u8; 1024];
            let _ = server_io.read(&mut buf).await;
            let _ = server_io
                .write_all(b"R\x00\x00\x00\x08\x00\x00\x00\x00K\x00\x00\x00\x0c\x00\x00\x04\xd2\x12\x34\x56\x78Z\x00\x00\x00\x05I")
                .await;
            let _ = server_io.flush().await;
            while server_io.read(&mut buf).await.unwrap_or(0) > 0 {
                let _ = server_io
                    .write_all(b"E\x00\x00\x00\x16SERROR\x00Mmock\x00\x00Z\x00\x00\x00\x05I")
                    .await;
                let _ = server_io.flush().await;
            }
        });
        let (client, connection) = tokio_postgres::Config::new()
            .connect_raw(client_io, tokio_postgres::NoTls)
            .await
            .unwrap();
        tokio::spawn(async move {
            let _ = connection.await;
        });

        web::Data::new(Arc::new(ServerData {
            db: Arc::new(crate::db::Database::new(Arc::new(client))),
            tqms_client: Arc::new(crate::tqms_integration::TQMSClient::new().await),
            cache: Arc::new(tokio::sync::RwLock::new(cache)),
        }))
    }

    #[test]
    fn test_compute_members_matches_dashboard_statistics_counts() {
        let cache = build_sample_cache();
        let date: Date = DateParams::from("2026-11-17").into();

        for requested in [None, Some("2082/83"), Some("2083/84"), Some("15f21721-50e8-41af-9596-9d8cdb8d6a16")] {
            let stats = compute_statistics(requested, &cache, &[], date.clone());
            let members = compute_members(requested, &cache, &[], date.clone());

            assert_eq!(members.fiscal_year, stats.fiscal_year);
            assert_eq!(members.total_users.len(), stats.total_users_year);
            assert_eq!(members.total_active_users.len(), stats.total_active_users_year);
            assert_eq!(members.new_users.len(), stats.new_users_year);
            assert_eq!(members.displaced_users.len(), stats.displaced_users_year);
            assert_eq!(members.inactive_year.len(), stats.total_inactive_users_year);
            assert_eq!(members.total_inactive_users, members.inactive_year);

            assert_eq!(members.total_users.len(), 4);
            assert_eq!(members.total_active_users.len(), 3); // 101, 102, external 999
            assert_eq!(members.new_users.len(), 2); // 102, external 999
            assert_eq!(members.displaced_users.len(), 1); // 104
            assert_eq!(members.inactive_year.len(), 2); // 103, 104
        }
    }

    #[actix_web::test]
    async fn test_members_routes_return_combined_and_individual_json_arrays() {
        let cache = build_sample_cache();
        let s_data = create_mock_server_data(cache).await;

        let app = actix_test::init_service(
            App::new()
                .app_data(s_data)
                .service(web::scope("/api/v1/members").configure(configure_routes))
                .service(web::scope("/api/v1/dashboard/members").configure(configure_routes)),
        )
        .await;

        // 1. Combined endpoint returns DashboardMembers JSON object
        let resp_all = actix_test::call_service(
            &app,
            actix_test::TestRequest::get()
                .uri("/api/v1/members?fiscal_year=2083/84")
                .to_request(),
        )
        .await;
        assert_eq!(resp_all.status(), StatusCode::OK);
        let parsed_all: DashboardMembers = actix_test::read_body_json(resp_all).await;
        assert_eq!(parsed_all.fiscal_year, "2083/84");
        assert_eq!(parsed_all.total_users.len(), 4);
        assert_eq!(parsed_all.total_active_users.len(), 3);
        assert_eq!(parsed_all.new_users.len(), 2);
        assert_eq!(parsed_all.displaced_users.len(), 1);
        assert_eq!(parsed_all.inactive_year.len(), 2);

        // 2. Each individual array route returns its JSON array (`Vec<DatabaseResource>`)
        for (path, expected_len) in [
            ("/api/v1/members/total_users", 4usize),
            ("/api/v1/members/total_users/2083%2F84", 4),
            ("/api/v1/members/total_active_users", 3),
            ("/api/v1/members/total_active_users?fiscalYear=2083/84", 3),
            ("/api/v1/members/new_users", 2),
            ("/api/v1/members/new_users?fiscalYearId=15f21721-50e8-41af-9596-9d8cdb8d6a16", 2),
            ("/api/v1/members/displaced_users", 1),
            ("/api/v1/members/inactive_year", 2),
            ("/api/v1/dashboard/members/inactive_year?fiscal_year=2083/84", 2),
        ] {
            let resp =
                actix_test::call_service(&app, actix_test::TestRequest::get().uri(path).to_request())
                    .await;
            assert_eq!(resp.status(), StatusCode::OK, "failed route: {}", path);
            let list: Vec<DatabaseResource> = actix_test::read_body_json(resp).await;
            assert_eq!(list.len(), expected_len, "length mismatch for route: {}", path);
        }

        // 3. Past fiscal year without active FY bills shows only 2081/82 active vehicle
        let resp_2081 = actix_test::call_service(
            &app,
            actix_test::TestRequest::get()
                .uri("/api/v1/members/total_active_users/2081%2F82")
                .to_request(),
        )
        .await;
        assert_eq!(resp_2081.status(), StatusCode::OK);
        let list_2081: Vec<DatabaseResource> = actix_test::read_body_json(resp_2081).await;
        assert_eq!(list_2081.len(), 1);
        assert_eq!(list_2081[0].id, Some(1));
    }
}
