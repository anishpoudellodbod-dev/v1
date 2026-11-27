use crate::dashboard_statistics::{current_date, get_cached_db_vehicles, DashboardQuery};
use crate::resource::DatabaseResource;
use crate::ServerData;
use actix_web::{web, HttpResponse};
pub use narayani_analytics::members::*;
use std::sync::Arc;

async fn load_members_for_fy(
    requested_fy: Option<&str>,
    s_data: &Arc<ServerData>,
) -> DashboardMembers {
    let db_vehicles = get_cached_db_vehicles(s_data).await;
    let cache = s_data.cache.read().await;
    compute_members(requested_fy, &cache, &db_vehicles, current_date())
}

fn normalize_path_fy(path: web::Path<String>) -> Option<String> {
    let trimmed = path.into_inner();
    let trimmed = trimmed.trim().trim_matches('/').trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
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
        .route("/total_users/{fiscal_year:.*}", web::get().to(get_total_users_by_fy))
        .route("/total", web::get().to(get_total_users))
        .route("/total/{fiscal_year:.*}", web::get().to(get_total_users_by_fy))
        .route("/total_active_users", web::get().to(get_total_active_users))
        .route(
            "/total_active_users/{fiscal_year:.*}",
            web::get().to(get_total_active_users_by_fy),
        )
        .route("/active", web::get().to(get_total_active_users))
        .route("/active/{fiscal_year:.*}", web::get().to(get_total_active_users_by_fy))
        .route("/new_users", web::get().to(get_new_users))
        .route("/new_users/{fiscal_year:.*}", web::get().to(get_new_users_by_fy))
        .route("/new", web::get().to(get_new_users))
        .route("/new/{fiscal_year:.*}", web::get().to(get_new_users_by_fy))
        .route("/displaced_users", web::get().to(get_displaced_users))
        .route(
            "/displaced_users/{fiscal_year:.*}",
            web::get().to(get_displaced_users_by_fy),
        )
        .route("/displaced", web::get().to(get_displaced_users))
        .route("/displaced/{fiscal_year:.*}", web::get().to(get_displaced_users_by_fy))
        .route("/inactive_year", web::get().to(get_inactive_year))
        .route("/inactive_year/{fiscal_year:.*}", web::get().to(get_inactive_year_by_fy))
        .route("/total_inactive_users", web::get().to(get_inactive_year))
        .route(
            "/total_inactive_users/{fiscal_year:.*}",
            web::get().to(get_inactive_year_by_fy),
        )
        .route("/inactive", web::get().to(get_inactive_year))
        .route("/inactive/{fiscal_year:.*}", web::get().to(get_inactive_year_by_fy))
        .route("/{fiscal_year:.*}", web::get().to(get_members_by_fy));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::Cache;
    use crate::dashboard_statistics::compute_statistics;
    use crate::resource::ResourceName;
    use narayani_date::{Date, DateParams};
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

        // Vehicle 103: only cancelled bill in 2083/84 -> NON-MEMBER (excluded from total_users & inactive_year)
        cache.write(
            ResourceName::VehicleResource,
            3,
            make_vehicle(3, 103, "BA-01-PA-1003", Some("2026-11-03T00:00:00Z")),
        );

        // Vehicle 104: no bills, latest MemberResource says DISPLACED -> non-member for billing, counted in displaced_users
        cache.write(
            ResourceName::VehicleResource,
            4,
            make_vehicle(4, 104, "BA-01-PA-1004", Some("2026-11-04T00:00:00Z")),
        );

        // Vehicle 105: valid bill ONLY in 2081/82 -> member, inactive in 2083/84
        cache.write(
            ResourceName::VehicleResource,
            5,
            make_vehicle(5, 105, "BA-01-PA-1005", Some("2024-11-05T00:00:00Z")),
        );

        // Bills
        cache.write(
            ResourceName::BillResource,
            1,
            make_bill(1, "101", "BA-01-PA-1001", "2081/82", Some("2024-11-01"), None),
        );
        cache.write(
            ResourceName::BillResource,
            6,
            make_bill(6, "105", "BA-01-PA-1005", "2081/82", Some("2024-11-05"), Some("PAID")),
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

            assert_eq!(members.total_users.len(), 4); // 101, 102, 105, external 999 (excludes cancelled-only 103 & unbilled 104)
            assert_eq!(members.total_active_users.len(), 3); // 101, 102, external 999
            assert_eq!(members.new_users.len(), 2); // 102, external 999
            assert_eq!(members.displaced_users.len(), 1); // 104
            assert_eq!(members.inactive_year.len(), 1); // 105
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
        assert_eq!(parsed_all.inactive_year.len(), 1);

        // 2. Each individual array route returns its JSON array (`Vec<DatabaseResource>`)
        for (path, expected_len) in [
            ("/api/v1/members/total_users", 4usize),
            ("/api/v1/members/total_users/2083%2F84", 4),
            ("/api/v1/members/total_active_users", 3),
            ("/api/v1/members/total_active_users?fiscalYear=2083/84", 3),
            ("/api/v1/members/new_users", 2),
            ("/api/v1/members/new_users?fiscalYearId=15f21721-50e8-41af-9596-9d8cdb8d6a16", 2),
            ("/api/v1/members/displaced_users", 1),
            ("/api/v1/members/inactive_year", 1),
            ("/api/v1/dashboard/members/inactive_year?fiscal_year=2083/84", 1),
        ] {
            let resp =
                actix_test::call_service(&app, actix_test::TestRequest::get().uri(path).to_request())
                    .await;
            assert_eq!(resp.status(), StatusCode::OK, "failed route: {}", path);
            let list: Vec<DatabaseResource> = actix_test::read_body_json(resp).await;
            assert_eq!(list.len(), expected_len, "length mismatch for route: {}", path);
        }

        // 3. Past fiscal year without active FY bills shows 2081/82 active vehicles (101 and 105)
        let resp_2081 = actix_test::call_service(
            &app,
            actix_test::TestRequest::get()
                .uri("/api/v1/members/total_active_users/2081%2F82")
                .to_request(),
        )
        .await;
        assert_eq!(resp_2081.status(), StatusCode::OK);
        let list_2081: Vec<DatabaseResource> = actix_test::read_body_json(resp_2081).await;
        assert_eq!(list_2081.len(), 2);
        let ids_2081: Vec<i32> = list_2081.iter().filter_map(|r| r.id).collect();
        assert!(ids_2081.contains(&1));
        assert!(ids_2081.contains(&5));
    }

    #[test]
    fn test_compute_members_respects_vehicle_management_displaced_tagging() {
        let mut cache = build_sample_cache();
        let write_displaced_setting = |cache: &mut Cache, enabled: bool| {
            cache.write(
                ResourceName::SettingResource,
                99,
                DatabaseResource {
                    id: Some(99),
                    resource_name: Some(ResourceName::SettingResource),
                    inner_content: Some(json!({
                        "schemaVersion": 1,
                        "vehicleManagement": {
                            "displacedTagging": {
                                "enabled": enabled
                            }
                        }
                    })),
                    external_content: None,
                },
            );
        };

        write_displaced_setting(&mut cache, false);
        let disabled = compute_members(
            Some("2083/84"),
            &cache,
            &[],
            DateParams::from("2026-11-16").into(),
        );
        assert_eq!(disabled.total_active_users.len(), 3);
        assert!(disabled.displaced_users.is_empty());

        write_displaced_setting(&mut cache, true);
        let enabled = compute_members(
            Some("2083/84"),
            &cache,
            &[],
            DateParams::from("2026-11-16").into(),
        );
        assert_eq!(enabled.displaced_users.len(), 1);
    }

    #[actix_web::test]
    async fn test_dashboard_members_excludes_cancelled_only_and_unbilled_and_dedups_via_history() {
        let mut cache = Cache::new();

        // Vehicle 31409 (only CANCELLED bill 258) -> MUST NOT appear in members
        cache.write(
            ResourceName::VehicleResource,
            1,
            make_vehicle(1, 31409, "Province-3-22-55-KA-256", Some("2026-11-27T00:00:00Z")),
        );
        // Vehicle 35930 (valid bill 215, plus later updated ID/Number linked via VehicleHistoryResource)
        cache.write(
            ResourceName::VehicleResource,
            2,
            make_vehicle(2, 99999, "Province-5-01-KA-9999", Some("2026-11-27T01:00:00Z")),
        );
        // Unbilled Vehicle -> MUST NOT appear in members
        cache.write(
            ResourceName::VehicleResource,
            3,
            make_vehicle(3, 55555, "BA-01-KA-5555", Some("2026-11-27T02:00:00Z")),
        );

        // User's CANCELLED bill sample (id: 258, vehicle: 31409)
        cache.write(
            ResourceName::BillResource,
            258,
            DatabaseResource {
                id: Some(258),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(json!({
                    "total": 1000,
                    "status": "CANCELLED",
                    "vehicle": {
                        "id": "31409",
                        "vehicleNumber": "Province-3-22-55-KA-256"
                    },
                    "fiscalYear": "2082/2083",
                    "billingDate": "2026-11-27",
                    "billingType": "MEMBERSHIP",
                    "auditHistory": [{ "at": "2026-11-27T08:32:00.913Z", "action": "CANCELLED", "reason": "hjkl" }]
                })),
                external_content: None,
            },
        );

        // User's valid bill sample (id: 215, vehicle: 35930 / State-3-G-KK-8211)
        cache.write(
            ResourceName::BillResource,
            215,
            DatabaseResource {
                id: Some(215),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(json!({
                    "total": 1000,
                    "vehicle": {
                        "id": "35930",
                        "vehicleNumber": "State-3-G-KK-8211"
                    },
                    "fiscalYear": "2082/2083",
                    "billingDate": "2026-11-26",
                    "billingType": "MEMBERSHIP"
                })),
                external_content: None,
            },
        );

        // Second valid bill for the SAME physical vehicle after BOTH VehicleId & VehicleNumber changed
        cache.write(
            ResourceName::BillResource,
            216,
            DatabaseResource {
                id: Some(216),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(json!({
                    "total": 500,
                    "vehicle": {
                        "id": "99999",
                        "vehicleNumber": "Province-5-01-KA-9999"
                    },
                    "fiscalYear": "2082/2083",
                    "billingDate": "2026-11-27",
                    "billingType": "MEMBERSHIP"
                })),
                external_content: None,
            },
        );

        // VehicleHistoryResource linked list tying (35930, State-3-G-KK-8211) <-> (99999, Province-5-01-KA-9999)
        let mut chain =
            crate::vehicle_history::VehicleHistoryChain::new("VHIST-DEDUP-1".to_string());
        let old_snap = json!({ "VehicleId": 35930, "VehicleNumber": "State-3-G-KK-8211" });
        let new_snap = json!({ "VehicleId": 99999, "VehicleNumber": "Province-5-01-KA-9999" });
        chain.prepend_node(
            crate::vehicle_history::VehicleChangeType::Create,
            Some(2),
            None,
            Some(&old_snap),
            None,
            None,
        );
        chain.prepend_node(
            crate::vehicle_history::VehicleChangeType::Update,
            Some(2),
            Some(&old_snap),
            Some(&new_snap),
            None,
            None,
        );
        cache.write(
            ResourceName::VehicleHistoryResource,
            1,
            DatabaseResource {
                id: Some(1),
                resource_name: Some(ResourceName::VehicleHistoryResource),
                inner_content: Some(chain.to_json_value()),
                external_content: None,
            },
        );

        let s_data = create_mock_server_data(cache).await;
        let app = actix_test::init_service(
            App::new()
                .app_data(s_data)
                .service(web::scope("/api/v1/dashboard/members").configure(configure_routes)),
        )
        .await;

        let resp = actix_test::call_service(
            &app,
            actix_test::TestRequest::get()
                .uri("/api/v1/dashboard/members/2082%2F2083")
                .to_request(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let members: DashboardMembers = actix_test::read_body_json(resp).await;
        assert_eq!(members.total_users.len(), 1);
        assert_eq!(members.total_users[0].id, Some(2));
        assert_eq!(members.total_active_users.len(), 1);
        assert_eq!(members.inactive_year.len(), 0);
    }

    #[actix_web::test]
    async fn test_members_routes_scope_by_selected_fiscal_year_and_support_slash_paths() {
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

        for (bill_id, vid, num, fy) in [
            (1, "1001", "BA-01-PA-1001", "2081/2082"),
            (2, "1002", "BA-01-PA-1002", "2082/2083"),
            (3, "1003", "BA-01-PA-1003", "2083/2084"),
        ] {
            cache.write(
                ResourceName::BillResource,
                bill_id,
                DatabaseResource {
                    id: Some(bill_id),
                    resource_name: Some(ResourceName::BillResource),
                    inner_content: Some(json!({
                        "total": 1000,
                        "vehicle": { "id": vid, "vehicleNumber": num },
                        "fiscalYear": fy,
                        "periodStart": "2026-11-27",
                        "billingType": "MEMBERSHIP"
                    })),
                    external_content: None,
                },
            );
        }

        let s_data = create_mock_server_data(cache).await;
        let app = actix_test::init_service(
            App::new()
                .app_data(s_data)
                .service(web::scope("/api/v1/dashboard/members").configure(configure_routes)),
        )
        .await;

        for (uri, expected_total, expected_active, expected_inactive) in [
            ("/api/v1/dashboard/members/2081/2082", 1usize, 1usize, 0usize),
            ("/api/v1/dashboard/members/2082/2083", 2, 1, 1),
            ("/api/v1/dashboard/members/2083/2084", 3, 1, 2),
        ] {
            let resp = actix_test::call_service(
                &app,
                actix_test::TestRequest::get().uri(uri).to_request(),
            )
            .await;
            assert_eq!(resp.status(), StatusCode::OK, "failed route {uri}");
            let members: DashboardMembers = actix_test::read_body_json(resp).await;
            assert_eq!(members.total_users.len(), expected_total, "total mismatch for {uri}");
            assert_eq!(
                members.total_active_users.len(),
                expected_active,
                "active mismatch for {uri}"
            );
            assert_eq!(
                members.inactive_year.len(),
                expected_inactive,
                "inactive mismatch for {uri}"
            );
        }
    }
}
