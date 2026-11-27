use crate::resource::{DatabaseResource, ResourceName};
use crate::ServerData;
use actix_web::{web, HttpResponse};
pub use narayani_analytics::dashboard::*;
pub use narayani_date::parse_fiscal_year_start;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

static DB_VEHICLES_CACHE: OnceLock<RwLock<HashMap<usize, Arc<Vec<DatabaseResource>>>>> =
    OnceLock::new();
static DB_VEHICLES_INFLIGHT: OnceLock<Mutex<HashSet<usize>>> = OnceLock::new();

fn compact_db_vehicle(resource: DatabaseResource) -> Option<DatabaseResource> {
    let inner = resource.inner_content.as_ref()?;
    let v_id = extract_string_or_int(inner.get("VehicleId"));
    let v_num = extract_normalized_number_from_vehicle(inner);
    if resource.id.is_none() && v_id.is_none() && v_num.is_none() {
        return None;
    }

    let mut map = serde_json::Map::with_capacity(2);
    if let Some(id_val) = v_id {
        map.insert("VehicleId".to_string(), Value::String(id_val));
    }
    if let Some(num_val) = v_num {
        map.insert("NormalizedVehicleNumber".to_string(), Value::String(num_val));
    }

    Some(DatabaseResource {
        id: resource.id,
        resource_name: resource.resource_name,
        inner_content: Some(Value::Object(map)),
        external_content: None,
    })
}

pub(crate) async fn get_cached_db_vehicles(s_data: &Arc<ServerData>) -> Arc<Vec<DatabaseResource>> {
    let db_key = Arc::as_ptr(&s_data.db) as usize;
    let cache_lock = DB_VEHICLES_CACHE.get_or_init(|| RwLock::new(HashMap::new()));
    if let Some(hit) = cache_lock.read().ok().and_then(|g| g.get(&db_key).cloned()) {
        return hit;
    }

    let inflight = DB_VEHICLES_INFLIGHT.get_or_init(|| Mutex::new(HashSet::new()));
    let should_spawn = inflight
        .lock()
        .map(|mut guard| guard.insert(db_key))
        .unwrap_or(false);

    if !should_spawn {
        return cache_lock
            .read()
            .ok()
            .and_then(|g| g.get(&db_key).cloned())
            .unwrap_or_else(|| Arc::new(Vec::new()));
    }

    let db = Arc::clone(&s_data.db);
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let loaded = match db.read_all_resource(ResourceName::VehicleResource).await {
            Ok(vehicles) => {
                let compact: Vec<DatabaseResource> =
                    vehicles.into_iter().filter_map(compact_db_vehicle).collect();
                let arc = Arc::new(compact);
                if let Ok(mut guard) = DB_VEHICLES_CACHE
                    .get_or_init(|| RwLock::new(HashMap::new()))
                    .write()
                {
                    guard.insert(db_key, Arc::clone(&arc));
                }
                arc
            }
            Err(err) => {
                eprintln!("Warning: failed to read historical vehicles from DB: {err}");
                Arc::new(Vec::new())
            }
        };

        if let Ok(mut guard) = DB_VEHICLES_INFLIGHT
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
        {
            guard.remove(&db_key);
        }

        let _ = tx.send(loaded);
    });

    match tokio::time::timeout(Duration::from_millis(20), rx).await {
        Ok(Ok(arc)) => arc,
        _ => cache_lock
            .read()
            .ok()
            .and_then(|g| g.get(&db_key).cloned())
            .unwrap_or_else(|| Arc::new(Vec::new())),
    }
}

pub async fn get_dashboard_statistics(
    query: web::Query<DashboardQuery>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let db_vehicles = get_cached_db_vehicles(&s_data).await;

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
    let trimmed = fy.trim().trim_matches('/').trim();
    let requested = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    };

    let db_vehicles = get_cached_db_vehicles(&s_data).await;

    let cache = s_data.cache.read().await;
    let stats = compute_statistics(requested, &cache, &db_vehicles, current_date());
    HttpResponse::Ok().json(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::Cache;
    use actix_web::http::StatusCode;
    use narayani_date::{
        ad_date_str_to_bs_ymd, expand_fiscal_year_aliases, infer_bs_fiscal_year_from_date_str,
        Date, DateParams, FiscalYearConfig,
    };
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
        assert_eq!(stats.total_users_year, 1);
        assert_eq!(stats.total_active_users_year, 1);
        assert_eq!(stats.new_users_year, 1);
        assert_eq!(stats.total_inactive_users_year, 0);
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

    async fn create_mock_server_data_for_tests(cache: Cache) -> web::Data<Arc<ServerData>> {
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

        web::Data::new(Arc::new(ServerData {
            db: Arc::new(crate::db::Database::new(Arc::new(client))),
            tqms_client: Arc::new(crate::tqms_integration::TQMSClient::new().await),
            cache: Arc::new(tokio::sync::RwLock::new(cache)),
        }))
    }

    #[actix_web::test]
    async fn test_get_dashboard_statistics_endpoint_returns_ok() {
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

        let s_data = create_mock_server_data_for_tests(cache).await;

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

    #[test]
    fn test_uuid_fiscal_year_bills_without_setting_resource_match_target_fiscal_year_via_bill_dates() {
        let mut cache = Cache::new();

        // 5 vehicles: 2 billed with "2083/84", 3 billed with UUID fiscalYearId and 2026-10-29 billingDate
        for i in 1..=5 {
            cache.write(
                ResourceName::VehicleResource,
                i,
                make_vehicle(
                    i,
                    36000 + i as i64,
                    &format!("BA-01-PA-{:04}", 1000 + i),
                    Some("2026-11-05T11:09:07Z"),
                ),
            );
        }

        // 2 bills with explicit "2083/84"
        cache.write(
            ResourceName::BillResource,
            1,
            make_bill(1, "36001", "BA-01-PA-1001", "2083/84", "Membership", "1", None),
        );
        cache.write(
            ResourceName::BillResource,
            2,
            make_bill(2, "36002", "BA-01-PA-1002", "2083/84", "Membership", "1", None),
        );

        // 3 bills with UUID fiscalYearId and no SettingResource mapping
        let uuid_fy = "15f21721-50e8-41af-9596-9d8cdb8d6a16";
        for i in 3..=5 {
            cache.write(
                ResourceName::BillResource,
                i,
                make_bill(
                    i,
                    &(36000 + i).to_string(),
                    &format!("BA-01-PA-{:04}", 1000 + i),
                    uuid_fy,
                    "Membership",
                    "1",
                    None,
                ),
            );
        }

        // Also ensure DB id collision is avoided: a 6th vehicle has DB id 1 (same as Bill 1's vehicle.id
        // if someone used DB id), and a bill with DB id 6 has vehicle.id="1" + distinct vehicleNumber
        cache.write(
            ResourceName::VehicleResource,
            6,
            make_vehicle(6, 99999, "NA-09-KHA-9999", Some("2026-11-05T11:09:07Z")),
        );
        cache.write(
            ResourceName::BillResource,
            6,
            make_bill(6, "1", "NA-09-KHA-9999", uuid_fy, "Membership", "1", None),
        );

        // Querying with None (defaults to 2083/2084), "2083/84", and UUID must all count all 6
        for requested in [None, Some("2083/84"), Some(uuid_fy)] {
            let stats = compute_statistics(requested, &cache, &[], test_date());
            assert_eq!(stats.total_users_year, 6);
            assert_eq!(
                stats.total_active_users_year, 6,
                "failed for requested fiscal year {:?}",
                requested
            );
            assert_eq!(stats.new_users_year, 6);
            assert_eq!(stats.total_inactive_users_year, 0);
        }
    }

    #[test]
    fn test_infer_bs_fiscal_year_and_devanagari_aliases_edge_cases() {
        // Exact AD -> BS conversion via DateMapper (date_range.json)
        assert_eq!(ad_date_str_to_bs_ymd("2026-11-16"), Some((2083, 8, 1)));
        assert_eq!(ad_date_str_to_bs_ymd("2026-10-29"), Some((2083, 7, 13)));

        // AD boundary around Shrawan 1 (2083-04-01 = 2026-07-17, 2084-04-01 = 2027-07-17)
        assert_eq!(infer_bs_fiscal_year_from_date_str("2026-07-15"), Some("2082/83".to_string()));
        assert_eq!(infer_bs_fiscal_year_from_date_str("2026-07-16"), Some("2082/83".to_string()));
        assert_eq!(infer_bs_fiscal_year_from_date_str("2026-07-17"), Some("2083/84".to_string()));
        assert_eq!(infer_bs_fiscal_year_from_date_str("2027-01-15T10:20:30Z"), Some("2083/84".to_string()));
        assert_eq!(infer_bs_fiscal_year_from_date_str("2027-07-16"), Some("2083/84".to_string()));
        assert_eq!(infer_bs_fiscal_year_from_date_str("2027-07-17"), Some("2084/85".to_string()));

        // BS boundary around Shrawan 1 (month 4)
        assert_eq!(infer_bs_fiscal_year_from_date_str("2083-03-31"), Some("2082/83".to_string()));
        assert_eq!(infer_bs_fiscal_year_from_date_str("2083-04-01"), Some("2083/84".to_string()));

        // Devanagari digits in dates and fiscal year strings
        assert_eq!(infer_bs_fiscal_year_from_date_str("२०८३-०४-०१"), Some("2083/84".to_string()));
        assert_eq!(infer_bs_fiscal_year_from_date_str("२०२६-१०-२९"), Some("2083/84".to_string()));
        assert_eq!(parse_fiscal_year_start("२०८३/८४"), Some(2083));
        assert_eq!(parse_fiscal_year_start("2083-2084"), Some(2083));

        let devanagari_aliases = expand_fiscal_year_aliases("२०८३/८४");
        assert!(devanagari_aliases.contains(&"2083".to_string()));
        assert!(devanagari_aliases.contains(&"2083/84".to_string()));
        assert!(devanagari_aliases.contains(&"2083/084".to_string()));
        assert!(devanagari_aliases.contains(&"2083/2084".to_string()));

        let hyphen_aliases = expand_fiscal_year_aliases("2083-2084");
        assert!(hyphen_aliases.contains(&"2083".to_string()));
        assert!(hyphen_aliases.contains(&"2083/84".to_string()));
        assert!(hyphen_aliases.contains(&"2083/2084".to_string()));

        // Invalid / out-of-range dates
        assert_eq!(infer_bs_fiscal_year_from_date_str("1969-12-31"), None);
        assert_eq!(infer_bs_fiscal_year_from_date_str("2096-01-01"), None);
        assert_eq!(infer_bs_fiscal_year_from_date_str("2026-13-01"), None);
        assert_eq!(infer_bs_fiscal_year_from_date_str("2026-00-10"), None);
        assert_eq!(infer_bs_fiscal_year_from_date_str("2026-10-33"), None);
        assert_eq!(infer_bs_fiscal_year_from_date_str("not-a-date"), None);
        assert_eq!(infer_bs_fiscal_year_from_date_str(""), None);
    }

    #[test]
    fn test_setting_resource_variations_and_future_fy_bills() {
        let mut cache = Cache::new();
        cache.write(
            ResourceName::SettingResource,
            1,
            DatabaseResource {
                id: Some(1),
                resource_name: Some(ResourceName::SettingResource),
                inner_content: Some(json!({
                    "currentFiscalYearId": "fy-uuid-2083",
                    "fiscal_years": [
                        { "id": "fy-uuid-2083", "startDate": "2026-07-17", "isActive": true },
                        { "id": "fy-uuid-2084", "from": "2027-07-17", "isActive": false }
                    ]
                })),
                external_content: None,
            },
        );

        cache.write(
            ResourceName::VehicleResource,
            1,
            make_vehicle(1, 501, "BA-01-PA-5001", Some("2026-11-01T00:00:00Z")),
        );
        cache.write(
            ResourceName::VehicleResource,
            2,
            make_vehicle(2, 502, "BA-01-PA-5002", Some("2026-11-01T00:00:00Z")),
        );

        // Vehicle 501 billed in 2083/84 AND future 2084/85 -> still counts as NEW in 2083/84
        cache.write(
            ResourceName::BillResource,
            1,
            make_bill(1, "501", "BA-01-PA-5001", "fy-uuid-2083", "Membership", "1", None),
        );
        cache.write(
            ResourceName::BillResource,
            2,
            make_bill(2, "501", "BA-01-PA-5001", "fy-uuid-2084", "Advance", "1", None),
        );

        // Vehicle 502 has a bill missing fiscalYearId/fiscalYear, falling back to paidAt/periodStart/periodEnd
        cache.write(
            ResourceName::BillResource,
            3,
            DatabaseResource {
                id: Some(3),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(json!({
                    "vehicle": { "id": "502", "vehicleNumber": "BA-01-PA-5002" },
                    "paidAt": "2026-08-01T12:00:00Z",
                    "items": [{ "title": "Fee", "amount": 500 }]
                })),
                external_content: None,
            },
        );

        let stats = compute_statistics(None, &cache, &[], test_date());
        assert_eq!(stats.fiscal_year, "2083/84");
        assert_eq!(stats.total_users_year, 2);
        assert_eq!(stats.total_active_users_year, 2);
        assert_eq!(stats.new_users_year, 2);
        assert_eq!(stats.total_inactive_users_year, 0);
    }

    #[test]
    fn test_bill_status_variants_and_vehicle_field_fallbacks() {
        // Empty cache & DB returns zeroes
        let empty_cache = Cache::new();
        let empty_stats = compute_statistics(None, &empty_cache, &[], test_date());
        assert_eq!(empty_stats.fiscal_year, "2083/2084");
        assert_eq!(empty_stats.total_users_year, 0);
        assert_eq!(empty_stats.total_active_users_year, 0);
        assert_eq!(empty_stats.new_users_year, 0);
        assert_eq!(empty_stats.displaced_users_year, 0);
        assert_eq!(empty_stats.total_inactive_users_year, 0);

        let mut cache = Cache::new();
        cache.write(
            ResourceName::VehicleResource,
            1,
            make_vehicle(1, 701, "BA-01-PA-7001", Some("2026-11-01T00:00:00Z")),
        );

        // Ignored statuses with varied casing and whitespace
        for (idx, st) in [" deleted ", "Amended", "ROLLED_BACK"].into_iter().enumerate() {
            cache.write(
                ResourceName::BillResource,
                (idx + 1) as i32,
                make_bill(
                    (idx + 1) as i32,
                    "701",
                    "BA-01-PA-7001",
                    "2083/84",
                    "Fee",
                    "1",
                    Some(st),
                ),
            );
        }

        // Uncached external vehicle billed twice with top-level & PascalCase fields and different formatting
        cache.write(
            ResourceName::BillResource,
            10,
            DatabaseResource {
                id: Some(10),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(json!({
                    "vehicleId": "EXT-55",
                    "vehicleNumber": "ba 02 pa 5555",
                    "periodStart": "2026-09-01",
                    "status": "PAID"
                })),
                external_content: None,
            },
        );
        cache.write(
            ResourceName::BillResource,
            11,
            DatabaseResource {
                id: Some(11),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(json!({
                    "vehicle": { "VehicleId": "EXT-55", "VehicleNumber": "BA-02-PA-5555" },
                    "periodEnd": "2026-10-01",
                    "status": "PAID"
                })),
                external_content: None,
            },
        );

        let stats = compute_statistics(Some("2083/84"), &cache, &[], test_date());
        // Cached vehicle 701 only had ignored bills -> non-member; external EXT-55 counted once -> active member
        assert_eq!(stats.total_users_year, 1);
        assert_eq!(stats.total_active_users_year, 1);
        assert_eq!(stats.new_users_year, 1);
        assert_eq!(stats.total_inactive_users_year, 0);
    }

    #[test]
    fn test_displaced_vehicles_tagged_at_fallback_tie_breaker_and_historical_config() {
        let mut cache = Cache::new();
        let latest_vehicle = make_vehicle(50, 9001, "LU-548-GA-9001", Some("2026-11-05T11:09:07Z"));
        let old_db_vehicle = make_vehicle(12, 9001, "OLD-LU-9001", None);
        cache.write(ResourceName::VehicleResource, 50, latest_vehicle.clone());
        cache.write(
            ResourceName::VehicleResource,
            51,
            make_vehicle(51, 9002, "LU-548-GA-9002", Some("2026-11-05T11:09:07Z")),
        );

        // Vehicle 9001: older taggedAt (external_content None) says ACTIVE, newer taggedAt referencing old DB config says " displaced " (without isDisplaced boolean)
        cache.write(
            ResourceName::MemberResource,
            1,
            DatabaseResource {
                id: Some(1),
                resource_name: Some(ResourceName::MemberResource),
                inner_content: Some(json!({
                    "vehicleId": "9001",
                    "vehicleNumber": "LU-548-GA-9001",
                    "status": "ACTIVE",
                    "taggedAt": "2026-11-01T10:00:00Z"
                })),
                external_content: None,
            },
        );
        cache.write(
            ResourceName::MemberResource,
            2,
            DatabaseResource {
                id: Some(2),
                resource_name: Some(ResourceName::MemberResource),
                inner_content: Some(json!({
                    "vehicleId": "12",
                    "vehicleNumber": "OLD-LU-9001",
                    "status": " displaced ",
                    "taggedAt": "2026-11-10T10:00:00Z"
                })),
                external_content: None,
            },
        );

        // Vehicle 9002: same missing timestamp, tie-broken by higher DB id (id: 4 ACTIVE beats id: 3 DISPLACED)
        cache.write(
            ResourceName::MemberResource,
            3,
            make_member(3, "9002", "LU-548-GA-9002", true, "DISPLACED", None),
        );
        cache.write(
            ResourceName::MemberResource,
            4,
            make_member(4, "9002", "LU-548-GA-9002", false, "ACTIVE", None),
        );

        let stats = compute_statistics(
            Some("2083/84"),
            &cache,
            &[old_db_vehicle, latest_vehicle],
            test_date(),
        );
        assert_eq!(stats.displaced_users_year, 1);
    }

    #[actix_web::test]
    async fn test_dashboard_query_variants_and_by_fy_endpoint() {
        let q1 = DashboardQuery {
            fiscal_year: Some(" 2083/84 ".to_string()),
            fiscal_year_camel: Some("2082/83".to_string()),
            fiscal_year_id: Some("uuid".to_string()),
        };
        assert_eq!(q1.requested_fiscal_year(), Some("2083/84"));

        let q2 = DashboardQuery {
            fiscal_year: None,
            fiscal_year_camel: Some(" 2082/83 ".to_string()),
            fiscal_year_id: Some("uuid".to_string()),
        };
        assert_eq!(q2.requested_fiscal_year(), Some("2082/83"));

        let q3 = DashboardQuery {
            fiscal_year: None,
            fiscal_year_camel: None,
            fiscal_year_id: Some("  15f21721-50e8-41af-9596-9d8cdb8d6a16 ".to_string()),
        };
        assert_eq!(
            q3.requested_fiscal_year(),
            Some("15f21721-50e8-41af-9596-9d8cdb8d6a16")
        );

        let q_blank = DashboardQuery {
            fiscal_year: Some("   ".to_string()),
            fiscal_year_camel: None,
            fiscal_year_id: None,
        };
        assert_eq!(q_blank.requested_fiscal_year(), None);

        let mut cache = Cache::new();
        cache.write(
            ResourceName::VehicleResource,
            1,
            make_vehicle(1, 36017, "LU-548-GA-6236", Some("2026-11-05T11:09:07Z")),
        );
        cache.write(
            ResourceName::BillResource,
            1,
            make_bill(1, "36017", "LU-548-GA-6236", "2083/84", "Membership", "1", None),
        );

        let s_data = create_mock_server_data_for_tests(cache).await;

        let resp_explicit =
            get_dashboard_statistics_by_fy(web::Path::from("2083/84".to_string()), s_data.clone())
                .await;
        assert_eq!(resp_explicit.status(), StatusCode::OK);
        let body_explicit = actix_web::body::to_bytes(resp_explicit.into_body()).await.unwrap();
        let parsed_explicit: DashBoardStatistics = serde_json::from_slice(&body_explicit).unwrap();
        assert_eq!(parsed_explicit.fiscal_year, "2083/84");
        assert_eq!(parsed_explicit.total_active_users_year, 1);

        let resp_blank =
            get_dashboard_statistics_by_fy(web::Path::from("   ".to_string()), s_data).await;
        assert_eq!(resp_blank.status(), StatusCode::OK);
        let body_blank = actix_web::body::to_bytes(resp_blank.into_body()).await.unwrap();
        let parsed_blank: DashBoardStatistics = serde_json::from_slice(&body_blank).unwrap();
        assert_eq!(parsed_blank.fiscal_year, "2083/2084");
        assert_eq!(parsed_blank.total_active_users_year, 1);
    }

    #[test]
    fn test_real_bill_resource_sample_counts_all_active_members_and_ignores_cancelled() {
        let mut cache = Cache::new();

        // Seed all 7 real SettingResource entries from configuration.
        let real_settings = [
            DatabaseResource {
                id: Some(6),
                resource_name: Some(ResourceName::SettingResource),
                inner_content: Some(json!({
                    "key": "arrears-list-rules",
                    "monthlyPaid": {
                        "enabled": true,
                        "fiscalYearId": "15f21721-50e8-41af-9596-9d8cdb8d6a16",
                        "billingRuleId": "5"
                    },
                    "paidExpired": {
                        "enabled": true,
                        "billingRuleId": "5"
                    }
                })),
                external_content: None,
            },
            DatabaseResource {
                id: Some(5),
                resource_name: Some(ResourceName::SettingResource),
                inner_content: Some(json!({
                    "key": "linked-fee-rules",
                    "rules": [
                        {
                            "id": "f72c97c7-ddd4-4d35-bbdd-86e4746e57db",
                            "active": true,
                            "fiscalYearId": "15f21721-50e8-41af-9596-9d8cdb8d6a16",
                            "linkedFeeHeadId": "21",
                            "sourceFeeHeadId": "3",
                            "oncePerFiscalYear": true
                        },
                        {
                            "id": "f604ebf4-8c7a-4e48-b9c5-40be292dcb7d",
                            "active": true,
                            "fiscalYearId": "15f21721-50e8-41af-9596-9d8cdb8d6a16",
                            "linkedFeeHeadId": "36",
                            "sourceFeeHeadId": "25",
                            "oncePerFiscalYear": true
                        }
                    ],
                    "enabled": true
                })),
                external_content: None,
            },
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
                            "endDate": "2026-07-16",
                            "startDate": "2025-07-17"
                        }
                    ]
                })),
                external_content: None,
            },
            DatabaseResource {
                id: Some(7),
                resource_name: Some(ResourceName::SettingResource),
                inner_content: Some(json!({
                    "key": "organization-vat",
                    "rate": 13,
                    "enabled": true,
                    "billingEntityIds": ["1"]
                })),
                external_content: None,
            },
            DatabaseResource {
                id: Some(4),
                resource_name: Some(ResourceName::SettingResource),
                inner_content: Some(json!({
                    "key": "billing-sms-phone",
                    "phone": "9600331176",
                    "vehicleId": "23437"
                })),
                external_content: Some(json!({
                    "phoneChangeReason": "Number pariwartan bhayeko"
                })),
            },
            DatabaseResource {
                id: Some(3),
                resource_name: Some(ResourceName::SettingResource),
                inner_content: Some(json!({
                    "key": "fiscal-year-selector",
                    "yearsBack": 4,
                    "yearsForward": 1,
                    "archivedYears": []
                })),
                external_content: None,
            },
            DatabaseResource {
                id: Some(1),
                resource_name: Some(ResourceName::SettingResource),
                inner_content: Some(json!({
                    "key": "tqms-sangh-branch-filter"
                })),
                external_content: None,
            },
        ];
        for setting in real_settings {
            let sid = setting.id.unwrap();
            cache.write(ResourceName::SettingResource, sid, setting);
        }

        let sample_vehicles = vec![
            (1, 32940, "lu-4-kha-5334"),
            (2, 36001, "Province-4-56-02-JHA-9922"),
            (3, 35994, "State-2-E-MM-3333"),
            (4, 35993, "State-2-E-GH-3333"),
            (5, 36020, "Province-4-01-001-KHA-0021"),
            (6, 35909, "State-4-D-KK-5656"),
            (7, 31361, "DH-90-GHA-5555"),
            (8, 36004, "State-3-F-AA-0010"),
            (9, 292, "State-2-F-AB-0010"),
            (10, 36040, "KA-09-PHA-9009"),
            (11, 36039, "NA-50-PHA-3690"),
            (12, 36051, "State-4-E-AA-5699"),
            (13, 35986, "State-7-G-TR-3892"),
            (14, 35903, "Province-3-53-01-0-2256"),
            (15, 34442, "Su Pra-03-6-PA-9913"),
            (16, 33053, "KO PRA-11-11-YA-11"),
            (17, 36017, "LU-548-GA-6237"),
            (18, 36028, "Province-3-02-59-JHA-9988"),
            (19, 35997, "Province-4-23-45-GA-3320"),
            (20, 36003, "Province-5-53-21-GA-0099"),
            (21, 36002, "Ga Pra-15-23-KHA-5555"),
            (22, 35999, "State-3-D-HN-9866"),
            (23, 34473, "Ma Pra-11-11-PHA-1110"),
            (24, 36053, "State-3-G-JJ-0765"),
            (25, 36012, "State-7-F-GJ-0768"),
            (26, 36005, "DH-288-PHA-3214"),
            (27, 35953, "State-3-E-YY-3000"),
            (28, 36047, "State-5-G-BB-0456"),
            (29, 36026, "State-3-C1-GH-5556"),
            (30, 36048, "KA-02-GHA-0006"),
            (31, 36049, "State-4-H-RR-5677"),
            (32, 63646, "Ba Pra-03-001-KHA-5642"),
            (33, 36029, "BHE-02-JHA-0202"),
            (34, 31532, "DH-02-JHA-223"),
            (35, 36046, "Province-2-23-349-BA-3333"),
            (36, 1898, "BA-22-JA-3333"),
            (37, 36036, "Province-4-56-02-JHA-2233"),
            (38, 36030, "State-3-I1-ER-0928"),
            (39, 36009, "STATE-3-E-DD-9987"),
            (40, 290, "State-4-G-AB-1101"),
            (41, 2933, "BA-20-JA-5555"),
            (42, 25826, "RA-333-PHA-4433"),
            (43, 31676, "Province-3-33-333-KHA-3331"),
            (44, 35964, "State-4-J1-UU-3333"),
            (45, 31850, "State-3-B-VV-0011"),
            (46, 28751, "BA-01-JHA-01"),
            (47, 773, "BA-09-KHA-5555"),
            // Vehicles whose only bills are CANCELLED
            (48, 36027, "Ba Pra-57-122-GA-2850"),
            (49, 36031, "State-2-F-AB-9767"),
        ];

        for (db_id, vid, num) in sample_vehicles {
            cache.write(
                ResourceName::VehicleResource,
                db_id,
                make_vehicle(db_id, vid, num, Some("2026-11-05T11:09:07Z")),
            );
        }

        let uuid_fy = "15f21721-50e8-41af-9596-9d8cdb8d6a16";
        let sample_bills = vec![
            (28, "32940", "lu-4-kha-5334", "2083/84", None),
            (26, "32940", "lu-4-kha-5334", "2083/84", None),
            (23, "32940", "lu-4-kha-5334", "2083/84", None),
            (22, "32940", "lu-4-kha-5334", "2083/84", None),
            (25, "32940", "lu-4-kha-5334", "2083/84", None),
            (24, "32940", "lu-4-kha-5334", "2083/84", None),
            (46, "36001", "Province-4-56-02-JHA-9922", uuid_fy, None),
            (38, "35994", "State-2-E-MM-3333", uuid_fy, None),
            (39, "35994", "State-2-E-MM-3333", uuid_fy, None),
            (29, "35993", "State-2-E-GH-3333", uuid_fy, None),
            (16, "35993", "State-2-E-GH-3333", uuid_fy, None),
            (44, "35993", "State-2-E-GH-3333", uuid_fy, None),
            (45, "35993", "State-2-E-GH-3333", uuid_fy, None),
            (50, "35993", "State-2-E-GH-3333", uuid_fy, None),
            (69, "36020", "Province-4-01-001-KHA-0021", uuid_fy, None),
            (74, "36020", "Province-4-01-001-KHA-0021", uuid_fy, None),
            (81, "36020", "Province-4-01-001-KHA-0021", uuid_fy, None),
            (78, "36020", "Province-4-01-001-KHA-0021", uuid_fy, None),
            (72, "36020", "Province-4-01-001-KHA-0021", uuid_fy, Some("CANCELLED")),
            (51, "35909", "State-4-D-KK-5656", uuid_fy, None),
            (54, "35909", "State-4-D-KK-5656", uuid_fy, None),
            (55, "35909", "State-4-D-KK-5656", uuid_fy, None),
            (18, "31361", "DH-90-GHA-5555", uuid_fy, None),
            (21, "36004", "State-3-F-AA-0010", uuid_fy, None),
            (15, "36004", "State-3-F-AA-0010", uuid_fy, None),
            (30, "36004", "State-3-F-AA-0010", uuid_fy, None),
            (43, "36004", "State-3-F-AA-0010", uuid_fy, None),
            (42, "36004", "State-3-F-AA-0010", uuid_fy, None),
            (41, "36004", "State-3-F-AA-0010", uuid_fy, None),
            (61, "292", "State-1-F-AB-0010", uuid_fy, None),
            (63, "292", "State-2-F-AB-0010", uuid_fy, None),
            (59, "292", "State-1-F-AB-0010", uuid_fy, None),
            (92, "VH-101", "KA-09-PHA-9009", "2083/84", None),
            (93, "VH-101", "KA-09-PHA-9009", "2083/84", None),
            (94, "VH-101", "KA-09-PHA-9009", "2083/2084", None),
            (95, "36039", "KA-09-PHA-9009", "2083/2084", None),
            (97, "36040", "KA-09-PHA-9009", uuid_fy, None),
            (98, "36040", "KA-09-PHA-9009", uuid_fy, None),
            (99, "36040", "KA-09-PHA-9008", uuid_fy, None),
            (105, "36040", "KA-09-PHA-9008", uuid_fy, None),
            (116, "36051", "State-4-E-AA-5699", uuid_fy, None),
            (103, "36039", "NA-50-PHA-3690", "2083/2084", None),
            (79, "35986", "State-7-G-TR-3892", uuid_fy, None),
            (80, "35986", "State-7-G-TR-3892", uuid_fy, None),
            (82, "35986", "State-7-G-TR-3892", uuid_fy, None),
            (70, "35986", "State-7-G-TR-3892", uuid_fy, None),
            (71, "35986", "State-7-G-TR-3892", uuid_fy, None),
            (13, "35903", "Province-3-53-01-0-2256", uuid_fy, None),
            (12, "35903", "Province-3-53-01-0-2256", uuid_fy, None),
            (11, "35903", "Province-3-53-01-0-2256", uuid_fy, None),
            (33, "34442", "Su Pra-03-6-PA-9913", uuid_fy, None),
            (75, "33053", "KO PRA-11-11-YA-11", uuid_fy, None),
            (115, "36017", "LU-548-GA-6237", uuid_fy, None),
            (57, "36017", "LU-548-GA-6236", uuid_fy, None),
            (58, "36017", "LU-548-GA-6236", uuid_fy, None),
            (85, "36028", "Province-3-02-59-JHA-9988", uuid_fy, None),
            (108, "36028", "Province-3-02-59-JHA-9988", uuid_fy, None),
            (35, "35997", "Ga Pra-23-45-GA-3320", uuid_fy, None),
            (36, "35997", "Ga Pra-23-45-GA-3320", uuid_fy, None),
            (37, "35997", "Province-4-23-45-GA-3320", uuid_fy, None),
            (34, "35997", "Ga Pra-23-45-GA-3320", uuid_fy, None),
            (7, "35997", "Ga Pra-23-45-GA-3320", uuid_fy, None),
            (5, "36003", "Province-5-53-21-GA-0099", uuid_fy, None),
            (4, "36002", "Ga Pra-15-23-KHA-5555", uuid_fy, None),
            (8, "35999", "State-3-D-HN-9865", uuid_fy, None),
            (9, "35999", "State-3-D-HN-9866", uuid_fy, None),
            (14, "35999", "State-3-D-HN-9866", uuid_fy, None),
            (49, "35999", "State-3-D-HN-9866", uuid_fy, None),
            (10, "35999", "State-3-D-HN-9865", uuid_fy, None),
            (91, "34473", "Ma Pra-11-11-PHA-1110", uuid_fy, None),
            (114, "36053", "State-3-G-JJ-0765", uuid_fy, None),
            (62, "36012", "State-7-F-GJ-0767", uuid_fy, None),
            (65, "36012", "State-7-F-GJ-0768", uuid_fy, None),
            (68, "36012", "State-7-F-GJ-0768", uuid_fy, None),
            (64, "36012", "State-7-F-GJ-0767", uuid_fy, None),
            (48, "36005", "DH-288-PHA-3213", uuid_fy, None),
            (32, "36005", "DH-288-PHA-3214", uuid_fy, None),
            (52, "35953", "State-3-E-YY-3000", uuid_fy, None),
            (106, "36047", "State-5-G-BB-0456", uuid_fy, None),
            (77, "36026", "State-3-C1-GH-5556", uuid_fy, None),
            (109, "36048", "KA-02-GHA-0006", uuid_fy, None),
            (117, "36048", "KA-02-GHA-0006", uuid_fy, None),
            (111, "36049", "State-4-H-RR-5677", uuid_fy, None),
            (112, "36049", "State-4-H-RR-5677", uuid_fy, None),
            (1, "63646", "Ba Pra-03-001-KHA-5642", "2", None),
            (87, "36029", "BHE-02-JHA-0202", uuid_fy, None),
            (118, "31532", "DH-02-JHA-222", uuid_fy, None),
            (100, "31532", "DH-02-JHA-222", uuid_fy, None),
            (101, "31532", "DH-02-JHA-223", uuid_fy, None),
            (96, "31532", "DH-02-JHA-222", uuid_fy, None),
            (102, "31532", "DH-02-JHA-223", uuid_fy, None),
            (104, "36046", "Province-2-23-349-BA-3333", uuid_fy, None),
            (3, "1898", "BA-22-JA-3333", uuid_fy, None),
            (90, "36036", "Province-4-56-02-JHA-2233", uuid_fy, None),
            (88, "36030", "State-3-I1-ER-0928", uuid_fy, None),
            (53, "36009", "STATE-3-E-DD-9987", uuid_fy, None),
            (56, "290", "State-4-G-AB-1101", uuid_fy, None),
            (31, "2933", "BA-20-JA-5555", uuid_fy, None),
            (47, "25826", "RA-333-PHA-4433", uuid_fy, None),
            (107, "31676", "Province-3-33-333-KHA-3331", uuid_fy, None),
            (19, "35964", "State-4-J1-UU-3333", uuid_fy, None),
            (67, "31850", "State-3-B-VV-0011", uuid_fy, None),
            (84, "28751", "BA-01-JHA-01", uuid_fy, None),
            (2, "773", "BA-09-KHA-5555", uuid_fy, None),
            // Cancelled bills for vehicles with no other valid bills
            (86, "36027", "Ba Pra-57-122-GA-2850", uuid_fy, Some("CANCELLED")),
            (89, "36031", "State-2-F-AB-9767", uuid_fy, Some("CANCELLED")),
        ];

        for (bill_id, vid, num, fy, status) in sample_bills {
            let mut bill = make_bill(bill_id, vid, num, fy, "Fee", "1", status);
            if fy == uuid_fy || fy == "2" {
                if let Some(Value::Object(ref mut map)) = bill.inner_content {
                    map.insert("periodStart".to_string(), json!("2026-11-16"));
                }
            }
            cache.write(ResourceName::BillResource, bill_id, bill);
        }

        // Verify all 47 unique active vehicles are counted regardless of query format,
        // and the 2 cancelled-only vehicles remain inactive.
        for requested in [
            None,
            Some("2082/83"),
            Some("2083"),
            Some("2083/84"),
            Some("2083/2084"),
            Some(uuid_fy),
        ] {
            let stats = compute_statistics(requested, &cache, &[], test_date());
            assert_eq!(stats.total_users_year, 47);
            assert_eq!(
                stats.total_active_users_year, 47,
                "active mismatch for {:?}",
                requested
            );
            assert_eq!(stats.new_users_year, 47);
            assert_eq!(stats.total_inactive_users_year, 0);
        }

        // Verify rule IDs in linked-fee-rules are never registered as fiscal years
        let settings_only = cache.read_all(ResourceName::SettingResource).unwrap();
        let fy_check = FiscalYearConfig::from_settings(&settings_only);
        assert!(
            !fy_check.has_ranked_group("f72c97c7-ddd4-4d35-bbdd-86e4746e57db"),
            "linked-fee-rules rule id must not be treated as a fiscal year"
        );

        // Even if SettingResource had an arbitrary unparsed label for uuid_fy,
        // periodStart AD-to-BS conversion ("2026-11-16" -> "2083-08-01" -> "2083/84") still matches all 47.
        cache.write(
            ResourceName::SettingResource,
            2,
            DatabaseResource {
                id: Some(2),
                resource_name: Some(ResourceName::SettingResource),
                inner_content: Some(json!({
                    "key": "fiscal-year-management",
                    "fiscalYears": [
                        { "id": uuid_fy, "name": "Current FY", "status": "ACTIVE" }
                    ]
                })),
                external_content: None,
            },
        );
        for requested in [None, Some("2083"), Some("2083/84"), Some(uuid_fy)] {
            let stats = compute_statistics(requested, &cache, &[], test_date());
            assert_eq!(stats.total_active_users_year, 47);
        }

        // Verify auditHistory cancellation without top-level status is also ignored
        cache.write(
            ResourceName::BillResource,
            999,
            DatabaseResource {
                id: Some(999),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(json!({
                    "vehicle": { "id": "36031", "vehicleNumber": "State-2-F-AB-9767" },
                    "billingDate": "2026-11-06",
                    "fiscalYearId": uuid_fy,
                    "auditHistory": [{ "action": "CANCELLED", "reason": "mistake" }]
                })),
                external_content: None,
            },
        );
        let stats_audit = compute_statistics(Some("2083/84"), &cache, &[], test_date());
        assert_eq!(stats_audit.total_active_users_year, 47);
    }

    #[test]
    fn test_migrated_billing_rule_fiscal_year_and_schema_v1_setting_resource() {
        let mut cache = Cache::new();
        let uuid_fy = "15f21721-50e8-41af-9596-9d8cdb8d6a16";

        // Migrated BillingRuleResource using fiscalYear: "2082/2083"
        cache.write(
            ResourceName::BillingRuleResource,
            24,
            DatabaseResource {
                id: Some(24),
                resource_name: Some(ResourceName::BillingRuleResource),
                inner_content: Some(json!({
                    "scope": "COMMON",
                    "amount": 0,
                    "period": null,
                    "status": "ACTIVE",
                    "feeHead": { "id": 23, "name": "तेश्रो पक्ष दुर्घटना सेवा सहयोग" },
                    "validity": "SAME_DAY",
                    "frequency": "ONE_TIME",
                    "fiscalYear": "2082/2083",
                    "resourceType": "BILLING_RULE",
                    "effectiveDate": "2026-11-01",
                    "billingEntityId": 1,
                    "meetingReference": "Testing Rule",
                    "vehicleCategoryId": null
                })),
                external_content: None,
            },
        );

        // Unified schemaVersion 1 SettingResource where linkedFeeRules rule UUID must not pollute setting_year_to_id
        cache.write(
            ResourceName::SettingResource,
            1,
            DatabaseResource {
                id: Some(1),
                resource_name: Some(ResourceName::SettingResource),
                inner_content: Some(json!({
                    "vat": { "rate": 13, "enabled": true, "billingEntityIds": ["1"] },
                    "arrears": {
                        "monthlyPaid": {
                            "enabled": true,
                            "fiscalYearId": uuid_fy,
                            "billingRuleId": "5"
                        },
                        "paidExpired": { "enabled": true, "billingRuleId": "5" }
                    },
                    "fiscalYear": {
                        "selector": { "yearsBack": 4, "yearsForward": 1, "archivedYears": [] },
                        "activeFiscalYears": []
                    },
                    "SanghBranch": "Loadboard sthaniya",
                    "schemaVersion": 1,
                    "linkedFeeRules": {
                        "rules": [
                            {
                                "id": "f72c97c7-ddd4-4d35-bbdd-86e4746e57db",
                                "active": true,
                                "fiscalYear": "2082/2083",
                                "linkedFeeHeadId": "21",
                                "sourceFeeHeadId": "3",
                                "oncePerFiscalYear": true
                            }
                        ],
                        "enabled": true
                    },
                    "displacedTagging": { "enabled": true },
                    "receiptCancelWindow": { "hours": 24, "enabled": false },
                    "insuranceEligibility": { "enabled": false, "ruleIds": [], "minPaidRules": 1 }
                })),
                external_content: None,
            },
        );

        let settings_only = cache.read_all(ResourceName::SettingResource).unwrap();
        let fy_check = FiscalYearConfig::from_settings(&settings_only);
        assert!(
            !fy_check.has_ranked_group("f72c97c7-ddd4-4d35-bbdd-86e4746e57db"),
            "linkedFeeRules rule id must not be registered as a fiscal year"
        );
        assert!(
            !fy_check.setting_year_to_id.contains_key(&2082),
            "linkedFeeRules rule id must not pollute setting_year_to_id"
        );

        for (id, vid, num) in [
            (1, 1001i64, "BA-01-PA-1001"),
            (2, 1002i64, "BA-01-PA-1002"),
            (3, 1003i64, "BA-01-PA-1003"),
            (4, 1004i64, "BA-01-PA-1004"),
        ] {
            cache.write(
                ResourceName::VehicleResource,
                id,
                make_vehicle(id, vid, num, None),
            );
        }

        // Bill 1 uses migrated "fiscalYear": "2082/2083"
        let mut b1 = make_bill(1, "1001", "BA-01-PA-1001", "2082/2083", "Fee", "1", None);
        if let Some(Value::Object(ref mut m)) = b1.inner_content {
            m.remove("fiscalYearId");
            m.insert("fiscalYear".to_string(), json!("2082/2083"));
            m.insert("periodStart".to_string(), json!("2025-10-10"));
        }
        cache.write(ResourceName::BillResource, 1, b1);

        // Bill 2 uses short "fiscalYear": "2082/83"
        let mut b2 = make_bill(2, "1002", "BA-01-PA-1002", "2082/83", "Fee", "1", None);
        if let Some(Value::Object(ref mut m)) = b2.inner_content {
            m.remove("fiscalYearId");
            m.insert("fiscalYear".to_string(), json!("2082/83"));
            m.insert("periodStart".to_string(), json!("2025-10-12"));
        }
        cache.write(ResourceName::BillResource, 2, b2);

        // Bill 3 uses legacy UUID fiscalYearId mapped via BS 2082 periodStart
        let mut b3 = make_bill(3, "1003", "BA-01-PA-1003", uuid_fy, "Fee", "1", None);
        if let Some(Value::Object(ref mut m)) = b3.inner_content {
            m.insert("periodStart".to_string(), json!("2025-10-15"));
        }
        cache.write(ResourceName::BillResource, 3, b3);

        for requested in [None, Some("2082/2083"), Some("2082/83"), Some(uuid_fy)] {
            let stats = compute_statistics(requested, &cache, &[], test_date());
            assert_eq!(stats.total_users_year, 3, "total mismatch for {requested:?}");
            assert_eq!(
                stats.total_active_users_year, 3,
                "active mismatch for {requested:?}"
            );
            assert_eq!(stats.new_users_year, 3, "new mismatch for {requested:?}");
            assert_eq!(
                stats.total_inactive_users_year, 0,
                "inactive mismatch for {requested:?}"
            );
        }
    }

    #[test]
    fn test_migrated_billing_rule_fallback_with_unmapped_setting_uuid_and_fiscal_year_bills() {
        let mut cache = Cache::new();
        let uuid_fy = "15f21721-50e8-41af-9596-9d8cdb8d6a16";

        // SettingResource has empty activeFiscalYears and an unmapped UUID in arrears.monthlyPaid
        cache.write(
            ResourceName::SettingResource,
            1,
            DatabaseResource {
                id: Some(1),
                resource_name: Some(ResourceName::SettingResource),
                inner_content: Some(json!({
                    "arrears": {
                        "monthlyPaid": {
                            "enabled": true,
                            "fiscalYearId": uuid_fy,
                            "billingRuleId": "5"
                        }
                    },
                    "fiscalYear": { "activeFiscalYears": [] },
                    "schemaVersion": 1
                })),
                external_content: None,
            },
        );

        // Migrated active BillingRuleResource provides the authoritative fiscalYear "2082/2083"
        cache.write(
            ResourceName::BillingRuleResource,
            24,
            DatabaseResource {
                id: Some(24),
                resource_name: Some(ResourceName::BillingRuleResource),
                inner_content: Some(json!({
                    "status": "ACTIVE",
                    "fiscalYear": "2082/2083",
                    "resourceType": "BILLING_RULE"
                })),
                external_content: None,
            },
        );

        cache.write(
            ResourceName::VehicleResource,
            1,
            make_vehicle(1, 2001, "BA-02-PA-2001", None),
        );
        cache.write(
            ResourceName::VehicleResource,
            2,
            make_vehicle(2, 2002, "BA-02-PA-2002", None),
        );

        let mut bill = make_bill(1, "2001", "BA-02-PA-2001", "2082/2083", "Fee", "1", None);
        if let Some(Value::Object(ref mut m)) = bill.inner_content {
            m.remove("fiscalYearId");
            m.insert("fiscalYear".to_string(), json!("2082/2083"));
        }
        cache.write(ResourceName::BillResource, 1, bill);

        let stats = compute_statistics(None, &cache, &[], test_date());
        assert_eq!(stats.fiscal_year, "2082/2083");
        assert_eq!(stats.total_users_year, 1);
        assert_eq!(stats.total_active_users_year, 1);
        assert_eq!(stats.new_users_year, 1);
        assert_eq!(stats.total_inactive_users_year, 0);
    }

    #[test]
    fn test_schema_v1_setting_resource_active_fiscal_years_strings() {
        let mut cache = Cache::new();
        cache.write(
            ResourceName::SettingResource,
            1,
            DatabaseResource {
                id: Some(1),
                resource_name: Some(ResourceName::SettingResource),
                inner_content: Some(json!({
                    "fiscalYear": {
                        "selector": { "yearsBack": 4, "yearsForward": 1 },
                        "activeFiscalYears": ["2082/2083"]
                    },
                    "schemaVersion": 1
                })),
                external_content: None,
            },
        );

        cache.write(
            ResourceName::VehicleResource,
            1,
            make_vehicle(1, 3001, "GA-01-PA-3001", None),
        );
        cache.write(
            ResourceName::VehicleResource,
            2,
            make_vehicle(2, 3002, "GA-01-PA-3002", None),
        );

        let mut b1 = make_bill(1, "3001", "GA-01-PA-3001", "2082/2083", "Fee", "1", None);
        if let Some(Value::Object(ref mut m)) = b1.inner_content {
            m.remove("fiscalYearId");
            m.insert("fiscalYear".to_string(), json!("2082/2083"));
        }
        cache.write(ResourceName::BillResource, 1, b1);

        let mut b2 = make_bill(2, "3002", "GA-01-PA-3002", "2082/83", "Fee", "1", None);
        if let Some(Value::Object(ref mut m)) = b2.inner_content {
            m.remove("fiscalYearId");
            m.insert("fiscalYear".to_string(), json!("2082/83"));
        }
        cache.write(ResourceName::BillResource, 2, b2);

        let stats = compute_statistics(None, &cache, &[], test_date());
        assert_eq!(stats.fiscal_year, "2082/2083");
        assert_eq!(stats.total_users_year, 2);
        assert_eq!(stats.total_active_users_year, 2);
        assert_eq!(stats.new_users_year, 2);
        assert_eq!(stats.total_inactive_users_year, 0);
    }

    #[test]
    fn test_nested_schema_v1_settings_billing_access_ignored_and_displaced_tagging_toggle() {
        let mut cache = Cache::new();
        let make_nested_setting = |displaced_enabled: bool| DatabaseResource {
            id: Some(1),
            resource_name: Some(ResourceName::SettingResource),
            inner_content: Some(json!({
                "billing": {
                    "vat": { "rate": 13, "enabled": true, "billingEntityIds": ["1"] },
                    "arrears": {
                        "monthlyPaid": { "enabled": true, "fiscalYear": "2083/84", "billingRuleId": "45" },
                        "paidExpired": { "enabled": true, "billingRuleId": "5" }
                    },
                    "linkedFeeRules": {
                        "enabled": true,
                        "rules": [
                            { "id": "r1", "active": true, "fiscalYear": "2082/83", "linkedFeeHeadId": "21", "sourceFeeHeadId": "3", "oncePerFiscalYear": true }
                        ]
                    }
                },
                "fiscalYear": {
                    "selector": { "yearsBack": 4, "yearsForward": 1, "archivedYears": [] },
                    "billingAccess": {
                        "nextYear": null,
                        "previousYear": {
                            "status": "CLOSED",
                            "updatedAt": "2026-11-22T12:08:22.422Z",
                            "updatedBy": "System Administrator",
                            "fiscalYear": "2082/83"
                        }
                    }
                },
                "integrations": { "tqms": { "SanghBranch": "Loadboard sthaniya" } },
                "vehicleManagement": {
                    "displacedTagging": { "enabled": displaced_enabled }
                },
                "schemaVersion": 1
            })),
            external_content: None,
        };

        cache.write(ResourceName::SettingResource, 1, make_nested_setting(true));
        cache.write(
            ResourceName::VehicleResource,
            1,
            make_vehicle(1, 4001, "GA-01-PA-4001", Some("2026-11-01T00:00:00Z")),
        );
        cache.write(
            ResourceName::VehicleResource,
            2,
            make_vehicle(2, 4002, "GA-01-PA-4002", Some("2025-11-01T00:00:00Z")),
        );
        cache.write(
            ResourceName::BillResource,
            1,
            make_bill(1, "4001", "GA-01-PA-4001", "2083/84", "Fee", "1", Some("PAID")),
        );
        cache.write(
            ResourceName::BillResource,
            2,
            make_bill(2, "4002", "GA-01-PA-4002", "2082/83", "Fee", "1", Some("PAID")),
        );
        cache.write(
            ResourceName::MemberResource,
            10,
            make_member(10, "4002", "GA-01-PA-4002", true, "DISPLACED", Some("2026-11-10T00:00:00Z")),
        );

        let stats_enabled = compute_statistics(None, &cache, &[], test_date());
        // billingAccess.previousYear ("2082/83") must NOT override monthlyPaid.fiscalYear ("2083/84")
        assert_eq!(stats_enabled.fiscal_year, "2083/84");
        assert_eq!(stats_enabled.total_users_year, 2);
        assert_eq!(stats_enabled.total_active_users_year, 1);
        assert_eq!(stats_enabled.total_inactive_users_year, 1);
        assert_eq!(stats_enabled.displaced_users_year, 1);

        // When displacedTagging is disabled in SettingResource, displaced count must drop to 0
        cache.write(ResourceName::SettingResource, 1, make_nested_setting(false));
        let stats_disabled = compute_statistics(None, &cache, &[], test_date());
        assert_eq!(stats_disabled.fiscal_year, "2083/84");
        assert_eq!(stats_disabled.displaced_users_year, 0);
    }

    #[actix_web::test]
    async fn test_fiscal_year_change_updates_members_for_selected_range_and_does_not_merge_explicit_fy_with_period_start()
    {
        let mut cache = Cache::new();

        for (id, vid, num) in [
            (1, 1001i64, "BA-01-PA-1001"),
            (2, 1002i64, "BA-01-PA-1002"),
            (3, 1003i64, "BA-01-PA-1003"),
        ] {
            cache.write(
                ResourceName::VehicleResource,
                id,
                make_vehicle(id, vid, num, Some("2026-11-27T00:00:00Z")),
            );
        }

        // Vehicle 1001 billed explicitly in "2081/2082"
        cache.write(
            ResourceName::BillResource,
            1,
            DatabaseResource {
                id: Some(1),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(json!({
                    "total": 1000,
                    "vehicle": { "id": "1001", "vehicleNumber": "BA-01-PA-1001" },
                    "fiscalYear": "2081/2082",
                    "billingDate": "2024-11-27",
                    "periodStart": "2024-11-27",
                    "billingType": "MEMBERSHIP"
                })),
                external_content: None,
            },
        );

        // Vehicle 1002 billed explicitly in "2082/2083", EVEN THOUGH periodStart is "2026-11-27" (BS 2083/84)
        cache.write(
            ResourceName::BillResource,
            2,
            DatabaseResource {
                id: Some(2),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(json!({
                    "total": 1000,
                    "vehicle": { "id": "1002", "vehicleNumber": "BA-01-PA-1002" },
                    "fiscalYear": "2082/2083",
                    "billingDate": "2026-11-27",
                    "periodStart": "2026-11-27",
                    "billingType": "MEMBERSHIP"
                })),
                external_content: None,
            },
        );

        // Vehicle 1003 billed explicitly in "2083/2084"
        cache.write(
            ResourceName::BillResource,
            3,
            DatabaseResource {
                id: Some(3),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(json!({
                    "total": 1000,
                    "vehicle": { "id": "1003", "vehicleNumber": "BA-01-PA-1003" },
                    "fiscalYear": "2083/2084",
                    "billingDate": "2026-11-27",
                    "periodStart": "2026-11-27",
                    "billingType": "MEMBERSHIP"
                })),
                external_content: None,
            },
        );

        // Member displaced in BS 2083/84 ("2026-11-27") must appear in 2083/84, NOT in earlier FYs
        cache.write(
            ResourceName::MemberResource,
            10,
            make_member(
                10,
                "1002",
                "BA-01-PA-1002",
                true,
                "DISPLACED",
                Some("2026-11-27T00:00:00Z"),
            ),
        );

        // Verify compute_statistics produces distinct scoped counts per selected fiscal year
        let s_2080 = compute_statistics(Some("2080/2081"), &cache, &[], test_date());
        assert_eq!(
            (
                s_2080.total_users_year,
                s_2080.total_active_users_year,
                s_2080.new_users_year,
                s_2080.total_inactive_users_year,
                s_2080.displaced_users_year
            ),
            (0, 0, 0, 0, 0)
        );

        let s_2081 = compute_statistics(Some("2081/2082"), &cache, &[], test_date());
        assert_eq!(
            (
                s_2081.total_users_year,
                s_2081.total_active_users_year,
                s_2081.new_users_year,
                s_2081.total_inactive_users_year,
                s_2081.displaced_users_year
            ),
            (1, 1, 1, 0, 0)
        );

        let s_2082 = compute_statistics(Some("2082/2083"), &cache, &[], test_date());
        assert_eq!(
            (
                s_2082.total_users_year,
                s_2082.total_active_users_year,
                s_2082.new_users_year,
                s_2082.total_inactive_users_year,
                s_2082.displaced_users_year
            ),
            (2, 1, 1, 1, 0)
        );

        let s_2083 = compute_statistics(Some("2083/2084"), &cache, &[], test_date());
        assert_eq!(
            (
                s_2083.total_users_year,
                s_2083.total_active_users_year,
                s_2083.new_users_year,
                s_2083.total_inactive_users_year,
                s_2083.displaced_users_year
            ),
            (3, 1, 1, 2, 1)
        );

        // Verify HTTP routes with slash-separated, hyphenated, percent-encoded, and bare year params
        let s_data = create_mock_server_data_for_tests(cache).await;
        let app = actix_web::test::init_service(
            actix_web::App::new().app_data(s_data).service(
                web::scope("/api/v1/dashboard")
                    .route("/statistics", web::get().to(get_dashboard_statistics))
                    .route(
                        "/statistics/{fiscal_year:.*}",
                        web::get().to(get_dashboard_statistics_by_fy),
                    ),
            ),
        )
        .await;

        let route_expectations = [
            ("/api/v1/dashboard/statistics/2080/2081", (0, 0, 0, 0, 0)),
            ("/api/v1/dashboard/statistics/2081/2082", (1, 1, 1, 0, 0)),
            ("/api/v1/dashboard/statistics/2082/2083", (2, 1, 1, 1, 0)),
            ("/api/v1/dashboard/statistics/2082-2083", (2, 1, 1, 1, 0)),
            ("/api/v1/dashboard/statistics/2082%2F2083", (2, 1, 1, 1, 0)),
            ("/api/v1/dashboard/statistics/2082", (2, 1, 1, 1, 0)),
            ("/api/v1/dashboard/statistics/2083/2084", (3, 1, 1, 2, 1)),
        ];

        for (uri, expected) in route_expectations {
            let resp = actix_web::test::call_service(
                &app,
                actix_web::test::TestRequest::get().uri(uri).to_request(),
            )
            .await;
            assert_eq!(resp.status(), StatusCode::OK, "failed route {uri}");
            let parsed: DashBoardStatistics = actix_web::test::read_body_json(resp).await;
            assert_eq!(
                (
                    parsed.total_users_year,
                    parsed.total_active_users_year,
                    parsed.new_users_year,
                    parsed.total_inactive_users_year,
                    parsed.displaced_users_year
                ),
                expected,
                "stats mismatch for {uri}"
            );
        }
    }
}
