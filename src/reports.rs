use crate::auth::Claims;
use crate::resource::{DatabaseResource, ResourceName};
use crate::ServerData;
use actix_web::{http::header, web, HttpMessage, HttpRequest, HttpResponse};
use narayani_analytics::reports::{
    export::{
        export_fee_head_wise, export_income, export_mahasang, export_member_transactions,
        export_vehicle_category, parse_export_format, ExportError, ExportOutput,
    },
    fee_head_wise::{
        compute_fee_head_wise_all_rows, compute_fee_head_wise_report, FeeHeadWiseQuery,
    },
    filter_options::{compute_filter_options, FilterOptionsQuery},
    income::{build_amdani_export_rows, compute_income_report, IncomeReportQuery},
    mahasang::{build_mahasang_rows, compute_mahasang_report, MahasangReportRequest},
    member_transactions::{
        compute_member_transactions, search_member_transactions, MemberSearchQuery,
        MemberTransactionsQuery,
    },
    resolve_caller_branch_context,
    vehicle_category::{
        build_vehicle_category_rows, compute_vehicle_category_report, VehicleCategoryQuery,
    },
};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ExportFormatQuery {
    pub format: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MahasangExportRequest {
    #[serde(default)]
    pub filters: MahasangReportRequest,
    #[serde(default)]
    pub visible_columns: Option<Vec<String>>,
    pub format: Option<String>,
}

/// Keeps `report_receipts` / `report_receipt_items` / `report_vehicle_aliases` synced on mutations.
pub async fn sync_report_projection(
    s_data: &ServerData,
    resource_name: ResourceName,
    id: i32,
    deleted: bool,
) {
    match resource_name {
        ResourceName::BillResource => {
            if deleted {
                if let Err(e) = s_data.db.delete_report_receipt(id).await {
                    eprintln!("Warning: failed to delete report_receipts row {id}: {e}");
                }
            } else {
                let projected_opt = {
                    let mut cache = s_data.cache.write().await;
                    cache.ensure_reports_fresh();
                    cache.report_index.receipts.get(&id).cloned()
                };
                if let Some(proj) = projected_opt {
                    if let Err(e) = s_data.db.upsert_report_receipt(&proj).await {
                        eprintln!("Warning: failed to upsert report_receipts row {id}: {e}");
                    }
                }
            }
        }
        ResourceName::VehicleResource | ResourceName::VehicleHistoryResource => {
            let target_opt = {
                let mut cache = s_data.cache.write().await;
                cache.ensure_reports_fresh();
                cache.vehicle_aliases.get_by_db_id(id).cloned()
            };
            if let Some(target) = target_opt {
                if let Err(e) = s_data.db.upsert_report_vehicle_alias(&target).await {
                    eprintln!("Warning: failed to upsert report_vehicle_aliases row {id}: {e}");
                }
            }
        }
        _ => {}
    }
}

async fn load_users_for_reports(s_data: &ServerData) -> Vec<DatabaseResource> {
    if let Ok(db_users) = s_data.db.read_all_resource(ResourceName::UserResource).await {
        if !db_users.is_empty() {
            return db_users;
        }
    }
    let cache = s_data.cache.read().await;
    cache
        .read_all(ResourceName::UserResource)
        .map(|v| v.into_iter().cloned().collect())
        .unwrap_or_default()
}

fn export_result_to_response(res: Result<ExportOutput, ExportError>) -> HttpResponse {
    match res {
        Ok(out) => HttpResponse::Ok()
            .insert_header((header::CONTENT_TYPE, out.content_type))
            .insert_header((
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", out.filename),
            ))
            .body(out.bytes),
        Err(ExportError::PdfNotImplemented) => HttpResponse::NotImplemented().json(
            serde_json::json!({
                "success": false,
                "code": "PDF_EXPORT_NOT_IMPLEMENTED",
                "message": "PDF export is not yet implemented on backend; use format=xlsx or format=csv"
            }),
        ),
        Err(ExportError::UnsupportedFormat(msg)) => HttpResponse::BadRequest().json(
            serde_json::json!({
                "success": false,
                "code": "INVALID_EXPORT_FORMAT",
                "message": msg
            }),
        ),
        Err(ExportError::GenerationFailed(msg)) => HttpResponse::InternalServerError().json(
            serde_json::json!({
                "success": false,
                "code": "EXPORT_FAILED",
                "message": msg
            }),
        ),
    }
}

pub async fn get_filter_options_handler(
    req: HttpRequest,
    query: web::Query<FilterOptionsQuery>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let claims = req.extensions().get::<Claims>().cloned();
    let users = load_users_for_reports(&s_data).await;
    let mut cache = s_data.cache.write().await;
    cache.ensure_reports_fresh();
    let settings = cache.cached_settings();
    let caller = resolve_caller_branch_context(claims.as_ref(), &users, &cache.report_index);
    let vehicles = cache
        .read_all(ResourceName::VehicleResource)
        .unwrap_or_default();
    let resp = compute_filter_options(
        &query.into_inner(),
        &caller,
        &cache.report_index,
        &users,
        &vehicles,
        &settings,
    );
    HttpResponse::Ok().json(resp)
}

pub async fn get_income_report_handler(
    req: HttpRequest,
    query: web::Query<IncomeReportQuery>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let claims = req.extensions().get::<Claims>().cloned();
    let users = load_users_for_reports(&s_data).await;
    let mut cache = s_data.cache.write().await;
    cache.ensure_reports_fresh();
    let caller = resolve_caller_branch_context(claims.as_ref(), &users, &cache.report_index);
    let resp = compute_income_report(&query.into_inner(), &caller, &cache.report_index, &users);
    HttpResponse::Ok().json(resp)
}

pub async fn export_income_report_handler(
    req: HttpRequest,
    query: web::Query<IncomeReportQuery>,
    fmt_query: web::Query<ExportFormatQuery>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let fmt = match parse_export_format(fmt_query.format.as_deref(), true) {
        Ok(f) => f,
        Err(e) => return export_result_to_response(Err(e)),
    };
    let claims = req.extensions().get::<Claims>().cloned();
    let users = load_users_for_reports(&s_data).await;
    let mut cache = s_data.cache.write().await;
    cache.ensure_reports_fresh();
    let caller = resolve_caller_branch_context(claims.as_ref(), &users, &cache.report_index);
    let (rows, m, o, g) =
        build_amdani_export_rows(&query.into_inner(), &caller, &cache.report_index, &users);
    export_result_to_response(export_income(fmt, &rows, m, o, g))
}

pub async fn get_fee_head_wise_handler(
    req: HttpRequest,
    query: web::Query<FeeHeadWiseQuery>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let claims = req.extensions().get::<Claims>().cloned();
    let users = load_users_for_reports(&s_data).await;
    let mut cache = s_data.cache.write().await;
    cache.ensure_reports_fresh();
    let caller = resolve_caller_branch_context(claims.as_ref(), &users, &cache.report_index);
    let resp =
        compute_fee_head_wise_report(&query.into_inner(), &caller, &cache.report_index, &users);
    HttpResponse::Ok().json(resp)
}

pub async fn export_fee_head_wise_handler(
    req: HttpRequest,
    query: web::Query<FeeHeadWiseQuery>,
    fmt_query: web::Query<ExportFormatQuery>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let fmt = match parse_export_format(fmt_query.format.as_deref(), false) {
        Ok(f) => f,
        Err(e) => return export_result_to_response(Err(e)),
    };
    let claims = req.extensions().get::<Claims>().cloned();
    let users = load_users_for_reports(&s_data).await;
    let mut cache = s_data.cache.write().await;
    cache.ensure_reports_fresh();
    let caller = resolve_caller_branch_context(claims.as_ref(), &users, &cache.report_index);
    let data =
        compute_fee_head_wise_all_rows(&query.into_inner(), &caller, &cache.report_index, &users);
    export_result_to_response(export_fee_head_wise(fmt, &data))
}

pub async fn get_vehicle_category_handler(
    req: HttpRequest,
    query: web::Query<VehicleCategoryQuery>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let claims = req.extensions().get::<Claims>().cloned();
    let users = load_users_for_reports(&s_data).await;
    let mut cache = s_data.cache.write().await;
    cache.ensure_reports_fresh();
    let caller = resolve_caller_branch_context(claims.as_ref(), &users, &cache.report_index);
    let resp = compute_vehicle_category_report(
        &query.into_inner(),
        &caller,
        &cache.report_index,
        &cache.vehicle_aliases,
    );
    HttpResponse::Ok().json(resp)
}

pub async fn export_vehicle_category_handler(
    req: HttpRequest,
    query: web::Query<VehicleCategoryQuery>,
    fmt_query: web::Query<ExportFormatQuery>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let fmt = match parse_export_format(fmt_query.format.as_deref(), false) {
        Ok(f) => f,
        Err(e) => return export_result_to_response(Err(e)),
    };
    let claims = req.extensions().get::<Claims>().cloned();
    let users = load_users_for_reports(&s_data).await;
    let mut cache = s_data.cache.write().await;
    cache.ensure_reports_fresh();
    let caller = resolve_caller_branch_context(claims.as_ref(), &users, &cache.report_index);
    let (rows, totals, _) = build_vehicle_category_rows(
        &query.into_inner(),
        &caller,
        &cache.report_index,
        &cache.vehicle_aliases,
    );
    export_result_to_response(export_vehicle_category(fmt, &rows, &totals))
}

pub async fn post_mahasang_handler(
    req: HttpRequest,
    body: Option<web::Json<MahasangReportRequest>>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let filter_req = body.map(|b| b.into_inner()).unwrap_or_default();
    let claims = req.extensions().get::<Claims>().cloned();
    let users = load_users_for_reports(&s_data).await;
    let mut cache = s_data.cache.write().await;
    cache.ensure_reports_fresh();
    let settings = cache.cached_settings();
    let caller = resolve_caller_branch_context(claims.as_ref(), &users, &cache.report_index);
    let resp = compute_mahasang_report(
        &filter_req,
        &caller,
        &cache.report_index,
        &cache.vehicle_aliases,
        &settings,
    );
    HttpResponse::Ok().json(resp)
}

pub async fn get_mahasang_handler(
    req: HttpRequest,
    query: web::Query<MahasangReportRequest>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let filter_req = query.into_inner();
    let claims = req.extensions().get::<Claims>().cloned();
    let users = load_users_for_reports(&s_data).await;
    let mut cache = s_data.cache.write().await;
    cache.ensure_reports_fresh();
    let settings = cache.cached_settings();
    let caller = resolve_caller_branch_context(claims.as_ref(), &users, &cache.report_index);
    let resp = compute_mahasang_report(
        &filter_req,
        &caller,
        &cache.report_index,
        &cache.vehicle_aliases,
        &settings,
    );
    HttpResponse::Ok().json(resp)
}

pub async fn post_mahasang_export_handler(
    req: HttpRequest,
    body: Option<web::Json<MahasangExportRequest>>,
    fmt_query: web::Query<ExportFormatQuery>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let payload = body.map(|b| b.into_inner()).unwrap_or_default();
    let fmt = match parse_export_format(
        payload.format.as_deref().or(fmt_query.format.as_deref()),
        false,
    ) {
        Ok(f) => f,
        Err(e) => return export_result_to_response(Err(e)),
    };
    let claims = req.extensions().get::<Claims>().cloned();
    let users = load_users_for_reports(&s_data).await;
    let mut cache = s_data.cache.write().await;
    cache.ensure_reports_fresh();
    let settings = cache.cached_settings();
    let caller = resolve_caller_branch_context(claims.as_ref(), &users, &cache.report_index);
    let (rows, summary, _) = build_mahasang_rows(
        &payload.filters,
        &caller,
        &cache.report_index,
        &cache.vehicle_aliases,
        &settings,
    );
    export_result_to_response(export_mahasang(
        fmt,
        &rows,
        &summary,
        payload.visible_columns.as_deref(),
    ))
}

pub async fn search_member_transactions_handler(
    req: HttpRequest,
    query: web::Query<MemberSearchQuery>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let claims = req.extensions().get::<Claims>().cloned();
    let users = load_users_for_reports(&s_data).await;
    let mut cache = s_data.cache.write().await;
    cache.ensure_reports_fresh();
    let caller = resolve_caller_branch_context(claims.as_ref(), &users, &cache.report_index);
    let members = cache
        .read_all(ResourceName::MemberResource)
        .unwrap_or_default();
    let vehicles = cache
        .read_all(ResourceName::VehicleResource)
        .unwrap_or_default();
    let resp = search_member_transactions(
        &query.into_inner(),
        &caller,
        &cache.report_index,
        &cache.vehicle_aliases,
        &members,
        &vehicles,
    );
    HttpResponse::Ok().json(resp)
}

pub async fn get_member_transactions_handler(
    req: HttpRequest,
    path: web::Path<String>,
    query: web::Query<MemberTransactionsQuery>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let vehicle_key = path.into_inner();
    let claims = req.extensions().get::<Claims>().cloned();
    let users = load_users_for_reports(&s_data).await;
    let mut cache = s_data.cache.write().await;
    cache.ensure_reports_fresh();
    let caller = resolve_caller_branch_context(claims.as_ref(), &users, &cache.report_index);
    let members = cache
        .read_all(ResourceName::MemberResource)
        .unwrap_or_default();
    let vehicles = cache
        .read_all(ResourceName::VehicleResource)
        .unwrap_or_default();
    let resp = compute_member_transactions(
        &vehicle_key,
        &query.into_inner(),
        &caller,
        &cache.report_index,
        &cache.vehicle_aliases,
        &members,
        &vehicles,
    );
    HttpResponse::Ok().json(resp)
}

pub async fn export_member_transactions_handler(
    req: HttpRequest,
    path: web::Path<String>,
    fmt_query: web::Query<ExportFormatQuery>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let fmt = match parse_export_format(fmt_query.format.as_deref(), false) {
        Ok(f) => f,
        Err(e) => return export_result_to_response(Err(e)),
    };
    let vehicle_key = path.into_inner();
    let claims = req.extensions().get::<Claims>().cloned();
    let users = load_users_for_reports(&s_data).await;
    let mut cache = s_data.cache.write().await;
    cache.ensure_reports_fresh();
    let caller = resolve_caller_branch_context(claims.as_ref(), &users, &cache.report_index);
    let members = cache
        .read_all(ResourceName::MemberResource)
        .unwrap_or_default();
    let vehicles = cache
        .read_all(ResourceName::VehicleResource)
        .unwrap_or_default();
    let data = compute_member_transactions(
        &vehicle_key,
        &MemberTransactionsQuery {
            page: Some(1),
            page_size: Some(200),
        },
        &caller,
        &cache.report_index,
        &cache.vehicle_aliases,
        &members,
        &vehicles,
    );
    export_result_to_response(export_member_transactions(fmt, &data))
}

pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.route("/filter-options", web::get().to(get_filter_options_handler))
        .route("/income", web::get().to(get_income_report_handler))
        .route("/income/export", web::get().to(export_income_report_handler))
        .route("/fee-head-wise", web::get().to(get_fee_head_wise_handler))
        .route(
            "/fee-head-wise/export",
            web::get().to(export_fee_head_wise_handler),
        )
        .route(
            "/vehicle-category",
            web::get().to(get_vehicle_category_handler),
        )
        .route(
            "/vehicle-category/export",
            web::get().to(export_vehicle_category_handler),
        )
        .route("/mahasang", web::post().to(post_mahasang_handler))
        .route("/mahasang", web::get().to(get_mahasang_handler))
        .route(
            "/mahasang/export",
            web::post().to(post_mahasang_export_handler),
        )
        .route(
            "/member-transactions/search",
            web::get().to(search_member_transactions_handler),
        )
        .route(
            "/member-transactions/{vehicle_key}/export",
            web::get().to(export_member_transactions_handler),
        )
        .route(
            "/member-transactions/{vehicle_key}",
            web::get().to(get_member_transactions_handler),
        );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::test_helpers::{create_mock_server_data_with_state, MockDbState};
    use crate::auth::create_jwt_with_context;
    use crate::middleware::AuthMiddleware;
    use crate::roles::Roles;
    use actix_web::{http::StatusCode, test as actix_test, App};
    use serde_json::json;

    async fn seeded_report_server() -> (web::Data<Arc<ServerData>>, String, String) {
        let mut mock_db = MockDbState::default();
        mock_db.seed(
            ResourceName::BranchResource,
            DatabaseResource {
                id: Some(1),
                resource_name: Some(ResourceName::BranchResource),
                inner_content: Some(json!({ "id": "1", "name": "Main Branch", "BranchCode": "MAIN" })),
                external_content: None,
            },
        );
        mock_db.seed(
            ResourceName::BranchResource,
            DatabaseResource {
                id: Some(3),
                resource_name: Some(ResourceName::BranchResource),
                inner_content: Some(json!({ "id": "3", "name": "चितवन", "BranchCode": "CIT" })),
                external_content: None,
            },
        );
        mock_db.seed(
            ResourceName::UserResource,
            DatabaseResource {
                id: Some(7),
                resource_name: Some(ResourceName::UserResource),
                inner_content: Some(json!({
                    "id": "7",
                    "name": "Chitwan Staff",
                    "email": "chitwan@test.com",
                    "branchId": "3",
                    "branchName": null
                })),
                external_content: None,
            },
        );
        mock_db.seed(
            ResourceName::BillingRuleResource,
            DatabaseResource {
                id: Some(10),
                resource_name: Some(ResourceName::BillingRuleResource),
                inner_content: Some(json!({
                    "resourceType": "FEE_HEAD",
                    "feeHeadId": "116",
                    "feeHead": "सदस्यता शुल्क",
                    "fiscalYear": "2082/2083",
                    "displayOrder": 1
                })),
                external_content: None,
            },
        );
        mock_db.seed(
            ResourceName::VehicleCategoryResource,
            DatabaseResource {
                id: Some(1),
                resource_name: Some(ResourceName::VehicleCategoryResource),
                inner_content: Some(json!({ "category": "ट्रक", "vehicle_types": ["10 pangree"] })),
                external_content: None,
            },
        );
        mock_db.seed(
            ResourceName::VehicleResource,
            DatabaseResource {
                id: Some(50),
                resource_name: Some(ResourceName::VehicleResource),
                inner_content: Some(json!({
                    "VehicleId": "V-CURR",
                    "VehicleNumber": "Province-3-01-23-KHA-5555",
                    "VehicleType": "10 pangree",
                    "previousVehicleIds": ["V-OLD"],
                    "previousVehicleNumbers": ["Na-1-KA-1111"],
                    "Owners": [{ "Name": "Ram", "Phone": "9800000001" }]
                })),
                external_content: None,
            },
        );
        mock_db.seed(
            ResourceName::MemberResource,
            DatabaseResource {
                id: Some(1),
                resource_name: Some(ResourceName::MemberResource),
                inner_content: Some(json!({
                    "vehicleId": "V-CURR",
                    "vehicleNumber": "Province-3-01-23-KHA-5555",
                    "memberNumber": "MEM-101"
                })),
                external_content: None,
            },
        );
        mock_db.seed(
            ResourceName::BillResource,
            DatabaseResource {
                id: Some(201),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(json!({
                    "paidAt": "2026-11-10T10:00:00Z",
                    "billingDate": "2026-11-10",
                    "periodStart": "2026-11-10",
                    "periodEnd": "2026-12-10",
                    "status": null,
                    "billingType": null,
                    "branch": { "id": "3", "name": "चितवन", "BranchCode": "CIT" },
                    "vehicle": { "id": "V-OLD", "vehicleNumber": "Na-1-KA-1111", "vehicleType": "10 pangree" },
                    "customerName": "=1+1",
                    "fiscalYear": "2082/2083",
                    "total": 2500,
                    "items": [{ "feeHeadId": "116", "title": "सदस्यता शुल्क", "amount": 2000 }]
                })),
                external_content: None,
            },
        );
        mock_db.seed(
            ResourceName::BillResource,
            DatabaseResource {
                id: Some(202),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(json!({
                    "paidAt": "2026-11-12T10:00:00Z",
                    "billingDate": "2026-11-12",
                    "periodStart": "2026-11-12",
                    "periodEnd": "2026-12-12",
                    "status": "PAID",
                    "billingType": "GENERAL",
                    "branch": { "id": "1", "name": "Main Branch", "BranchCode": "MAIN" },
                    "vehicle": { "id": "V-OTHER", "vehicleNumber": "BA-01-KHA-9999", "vehicleType": "" },
                    "fiscalYear": "2082/2083",
                    "total": 500,
                    "items": [{ "feeHeadId": "116", "title": "सदस्यता शुल्क", "amount": 500 }]
                })),
                external_content: None,
            },
        );

        let (s_data, _state) = create_mock_server_data_with_state(mock_db).await;
        {
            let mut cache = s_data.cache.write().await;
            cache.load_from_db(&s_data.db).await;
        }

        let admin_tok =
            create_jwt_with_context("admin@narayanisangh.com", &Roles::Admin, Some(1), None, None)
                .unwrap();
        let staff_tok = create_jwt_with_context(
            "chitwan@test.com",
            &Roles::Operator,
            Some(7),
            Some("Staff".into()),
            None,
        )
        .unwrap();

        (s_data, admin_tok, staff_tok)
    }

    #[actix_web::test]
    async fn http_reports_endpoints_enforce_rbac_dates_aliases_and_exports() {
        let (s_data, admin_tok, staff_tok) = seeded_report_server().await;
        let app = actix_test::init_service(
            App::new().app_data(s_data.clone()).service(
                web::scope("/api/v1/reports")
                    .wrap(AuthMiddleware::with_roles(&[
                        Roles::User,
                        Roles::Admin,
                        Roles::Operator,
                    ]))
                    .configure(configure_routes),
            ),
        )
        .await;

        // 1. Non-admin locked to assigned branch from UserResource.branchId -> BranchResource
        let req = actix_test::TestRequest::get()
            .uri("/api/v1/reports/filter-options?branch=Main%20Branch&reportType=mahasang")
            .insert_header(("Authorization", format!("Bearer {staff_tok}")))
            .to_request();
        let resp = actix_test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body: serde_json::Value = actix_test::read_body_json(resp).await;
        assert_eq!(body["branches"], json!(["चितवन"]));

        // 2. Inverted date range returns 200 OK with empty rows
        let req = actix_test::TestRequest::get()
            .uri("/api/v1/reports/income?dateFrom=2026-12-01&dateTo=2026-11-01")
            .insert_header(("Authorization", format!("Bearer {admin_tok}")))
            .to_request();
        let resp = actix_test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body: serde_json::Value = actix_test::read_body_json(resp).await;
        assert_eq!(body["pagination"]["total"], 0);
        assert_eq!(body["rows"], json!([]));

        // 3. Income CSV export neutralizes `=1+1` formula injection
        let req = actix_test::TestRequest::get()
            .uri("/api/v1/reports/income/export?format=csv")
            .insert_header(("Authorization", format!("Bearer {admin_tok}")))
            .to_request();
        let resp = actix_test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = actix_test::read_body(resp).await;
        let csv_text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(csv_text.contains("'=1+1"));

        // 4. PDF export returns HTTP 501 Not Implemented JSON
        let req = actix_test::TestRequest::get()
            .uri("/api/v1/reports/fee-head-wise/export?format=pdf")
            .insert_header(("Authorization", format!("Bearer {admin_tok}")))
            .to_request();
        let resp = actix_test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);

        // 5. POST /mahasang resolves previous alias & overlaps interval
        let req = actix_test::TestRequest::post()
            .uri("/api/v1/reports/mahasang")
            .insert_header(("Authorization", format!("Bearer {admin_tok}")))
            .set_json(json!({
                "dateBasis": "billingDate",
                "dateFrom": "2026-11-20",
                "dateTo": "2026-11-25",
                "vehicleTypes": []
            }))
            .to_request();
        let resp = actix_test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body: serde_json::Value = actix_test::read_body_json(resp).await;
        assert_eq!(body["summary"]["totalBills"], 2);
        assert_eq!(body["summary"]["totalMembership"], 2500.0);
        assert_eq!(body["summary"]["totalOther"], 500.0);

        // 6. Member transaction search + profile + export (xlsx)
        let req = actix_test::TestRequest::get()
            .uri("/api/v1/reports/member-transactions/search?q=MEM-101")
            .insert_header(("Authorization", format!("Bearer {admin_tok}")))
            .to_request();
        let resp = actix_test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let arr: serde_json::Value = actix_test::read_body_json(resp).await;
        assert_eq!(arr.as_array().unwrap().len(), 1);

        let req = actix_test::TestRequest::get()
            .uri("/api/v1/reports/member-transactions/Province-3-01-23-KHA-5555/export?format=xlsx")
            .insert_header(("Authorization", format!("Bearer {admin_tok}")))
            .to_request();
        let resp = actix_test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let xlsx_bytes = actix_test::read_body(resp).await;
        assert!(xlsx_bytes.starts_with(b"PK"));
    }
}
