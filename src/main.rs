use actix_cors::Cors;
use actix_web::{web, App, HttpResponse, HttpServer};
use dotenv::dotenv;
use rustls::{Certificate, PrivateKey, ServerConfig};
use rustls_pemfile::{certs, pkcs8_private_keys, rsa_private_keys};
use std::env;
use std::fs::File;
use std::io::{self, BufReader};
use std::sync::Arc;
mod db;
mod login;
use crate::db::{connect_to_db, Database};
use crate::resource::DatabaseResource;
mod actions;
mod auth;
mod cache;
mod middleware;
pub use narayani_core::{resource, roles};
mod tqms_integration;
pub mod billing_actions;
pub mod dashboard_statistics;
pub mod members;
mod api_stream;
pub use narayani_date as date;
pub mod log;
pub mod audit;
pub mod sms;
pub mod access;
pub mod settings;
pub mod permissions;
pub mod reports;
pub mod vehicle_history;
pub mod file_upload;
pub use narayani_plate as vehicle_number;

#[derive(Clone)]
pub struct ServerData {
    pub db: Arc<Database>,
    pub tqms_client: Arc<tqms_integration::TQMSClient>,
    pub cache: Arc<tokio::sync::RwLock<cache::Cache>>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RouteDoc {
    method: &'static str,
    path: String,
    auth: &'static str,
    request: serde_json::Value,
    response: serde_json::Value,
}

struct SampleContext {
    vehicle_number: String,
    vehicle_id: i64,
    bill_id: i32,
    fiscal_year: String,
    branch: String,
    user_id: i32,
    file_name: String,
    payment_ref: String,
    fee_head_id: i32,
}

fn is_sensitive_key(key: &str) -> bool {
    let norm: String = key
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect();
    matches!(
        norm.as_str(),
        "password" | "apikey" | "apisecret" | "secret" | "token" | "privatekey"
    )
}

fn redact_sensitive_fields(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                if is_sensitive_key(k) {
                    *v = serde_json::Value::String("[REDACTED]".to_string());
                } else {
                    redact_sensitive_fields(v);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr.iter_mut() {
                redact_sensitive_fields(item);
            }
        }
        _ => {}
    }
}

fn sanitize_resource(mut res: DatabaseResource) -> DatabaseResource {
    if let Some(ref mut inner) = res.inner_content {
        redact_sensitive_fields(inner);
    }
    if let Some(ref mut ext) = res.external_content {
        redact_sensitive_fields(ext);
    }
    res
}

async fn load_resource_samples(
    s_data: &ServerData,
    resource_name: resource::ResourceName,
    limit: usize,
) -> Vec<DatabaseResource> {
    let is_uncached = matches!(
        resource_name,
        resource::ResourceName::UserResource | resource::ResourceName::PermissionResource
    );

    let mut rows: Vec<DatabaseResource> = if !is_uncached {
        let cache_r = s_data.cache.read().await;
        match cache_r.read_all(resource_name) {
            Ok(items) if !items.is_empty() => items.into_iter().cloned().collect(),
            _ => Vec::new(),
        }
    } else {
        Vec::new()
    };

    if rows.is_empty() {
        if let Ok(db_rows) = s_data.db.read_all_resource(resource_name).await {
            rows = db_rows;
        }
    }

    rows.sort_by_key(|r| r.id.unwrap_or(i32::MAX));
    rows.into_iter().take(limit).map(sanitize_resource).collect()
}

fn first_inner_str(
    samples: &std::collections::HashMap<resource::ResourceName, Vec<DatabaseResource>>,
    resource_name: resource::ResourceName,
    keys: &[&str],
) -> Option<String> {
    let items = samples.get(&resource_name)?;
    for item in items {
        let Some(inner) = item.inner_content.as_ref() else {
            continue;
        };
        for key in keys {
            if let Some(val) = inner.get(*key).and_then(|v| v.as_str()) {
                let trimmed = val.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

fn first_inner_i64(
    samples: &std::collections::HashMap<resource::ResourceName, Vec<DatabaseResource>>,
    resource_name: resource::ResourceName,
    keys: &[&str],
) -> Option<i64> {
    let items = samples.get(&resource_name)?;
    for item in items {
        let Some(inner) = item.inner_content.as_ref() else {
            continue;
        };
        for key in keys {
            if let Some(v) = inner.get(*key) {
                if let Some(n) = v.as_i64() {
                    return Some(n);
                }
                if let Some(n) = v.as_str().and_then(|s| s.trim().parse::<i64>().ok()) {
                    return Some(n);
                }
            }
        }
    }
    None
}

fn extract_sample_context(
    samples: &std::collections::HashMap<resource::ResourceName, Vec<DatabaseResource>>,
) -> SampleContext {
    let vehicle_number = first_inner_str(
        samples,
        resource::ResourceName::VehicleResource,
        &["VehicleNumber", "vehicleNumber"],
    )
    .unwrap_or_else(|| "NA-5-KHA-1234".to_string());

    let vehicle_id = first_inner_i64(
        samples,
        resource::ResourceName::VehicleResource,
        &["VehicleId", "vehicleId"],
    )
    .unwrap_or(1);

    let bill_id = samples
        .get(&resource::ResourceName::BillResource)
        .and_then(|v| v.first())
        .and_then(|r| r.id)
        .unwrap_or(1);

    let fiscal_year = first_inner_str(
        samples,
        resource::ResourceName::BillResource,
        &["fiscalYear"],
    )
    .or_else(|| {
        first_inner_str(
            samples,
            resource::ResourceName::SettingResource,
            &["activeFiscalYear", "fiscalYear"],
        )
    })
    .unwrap_or_else(|| "2082/2083".to_string());

    let branch = first_inner_str(
        samples,
        resource::ResourceName::BranchResource,
        &["name", "branchName"],
    )
    .or_else(|| {
        first_inner_str(
            samples,
            resource::ResourceName::VehicleResource,
            &["SanghBranch", "sanghBranch"],
        )
    })
    .unwrap_or_else(|| "Loadboard sthaniya".to_string());

    let user_id = samples
        .get(&resource::ResourceName::UserResource)
        .and_then(|v| v.first())
        .and_then(|r| r.id)
        .unwrap_or(1);

    let file_name = first_inner_str(
        samples,
        resource::ResourceName::FileResource,
        &["storedName", "originalName"],
    )
    .unwrap_or_else(|| "sample-receipt.png".to_string());

    let payment_ref = first_inner_str(
        samples,
        resource::ResourceName::BillResource,
        &["lodbodPaymentReferenceId", "receiptNumber"],
    )
    .unwrap_or_else(|| "LODBOD-REF-001".to_string());

    let fee_head_id = samples
        .get(&resource::ResourceName::BillingRuleResource)
        .and_then(|v| v.first())
        .and_then(|r| r.id)
        .unwrap_or(1);

    SampleContext {
        vehicle_number,
        vehicle_id,
        bill_id,
        fiscal_year,
        branch,
        user_id,
        file_name,
        payment_ref,
        fee_head_id,
    }
}

fn push_route(
    routes: &mut Vec<RouteDoc>,
    method: &'static str,
    path: impl Into<String>,
    auth: &'static str,
    request: serde_json::Value,
    response: serde_json::Value,
) {
    routes.push(RouteDoc {
        method,
        path: path.into(),
        auth,
        request,
        response,
    });
}

fn sample_values(
    samples: &std::collections::HashMap<resource::ResourceName, Vec<DatabaseResource>>,
    resource_name: resource::ResourceName,
) -> serde_json::Value {
    samples
        .get(&resource_name)
        .map(|items| serde_json::to_value(items).unwrap_or_else(|_| serde_json::json!([])))
        .unwrap_or_else(|| serde_json::json!([]))
}

fn first_sample_inner(
    samples: &std::collections::HashMap<resource::ResourceName, Vec<DatabaseResource>>,
    resource_name: resource::ResourceName,
    fallback: serde_json::Value,
) -> serde_json::Value {
    samples
        .get(&resource_name)
        .and_then(|items| items.first())
        .and_then(|r| r.inner_content.clone())
        .unwrap_or(fallback)
}

pub async fn server_routes(s_data: web::Data<Arc<ServerData>>) -> HttpResponse {
    use resource::ResourceName::*;

    const SAMPLE_LIMIT: usize = 2;
    let resource_list = s_data.db.resource_name.clone();

    let mut samples_map = std::collections::HashMap::with_capacity(resource_list.len());
    let mut database_samples = serde_json::Map::new();
    for res_name in &resource_list {
        let rows = load_resource_samples(&s_data, *res_name, SAMPLE_LIMIT).await;
        let canonical = narayani_auth::canonical_resource_name(*res_name);
        database_samples.insert(
            canonical.to_string(),
            serde_json::to_value(&rows).unwrap_or_else(|_| serde_json::json!([])),
        );
        samples_map.insert(*res_name, rows);
    }

    let ctx = extract_sample_context(&samples_map);
    let sample_vehicle = sample_values(&samples_map, VehicleResource);
    let sample_bill = sample_values(&samples_map, BillResource);
    let sample_member = sample_values(&samples_map, MemberResource);
    let sample_setting = sample_values(&samples_map, SettingResource);
    let sample_rule = sample_values(&samples_map, BillingRuleResource);
    let sample_perm = sample_values(&samples_map, PermissionResource);
    let sample_logs = sample_values(&samples_map, ActionLogsResource);
    let sample_audit = sample_values(&samples_map, AuditTrailResource);
    let sample_history = sample_values(&samples_map, VehicleHistoryResource);
    let sample_file = sample_values(&samples_map, FileResource);

    let mut routes = Vec::new();

    // Root & Auth
    push_route(
        &mut routes,
        "GET",
        "/",
        "Public",
        serde_json::json!({}),
        serde_json::json!({
            "totalRoutes": "<number>",
            "databaseSamples": { "VehicleResource": sample_vehicle, "BillResource": sample_bill },
            "routes": [{ "method": "GET", "path": "/", "auth": "Public", "request": {}, "response": {} }]
        }),
    );
    push_route(
        &mut routes,
        "POST",
        "/api/v1/auth/login",
        "Public",
        serde_json::json!({ "body": { "email": "admin@narayanisangh.com", "password": "<password>" } }),
        serde_json::json!({ "token": "<jwt>", "user": sample_values(&samples_map, UserResource) }),
    );
    push_route(
        &mut routes,
        "POST",
        "/api/v1/auth/logout",
        "Bearer JWT",
        serde_json::json!({ "headers": { "Authorization": "Bearer <token>" } }),
        serde_json::json!({ "success": true, "message": "Logged out successfully" }),
    );

    // Dashboard & Members
    push_route(
        &mut routes,
        "GET",
        "/api/v1/dashboard/statistics",
        "Public",
        serde_json::json!({}),
        serde_json::json!({
            "fiscalYear": ctx.fiscal_year,
            "sampleBillsCounted": sample_bill,
            "summary": { "totalCollection": 0, "totalReceipts": 0, "activeMembers": 0 }
        }),
    );
    push_route(
        &mut routes,
        "GET",
        "/api/v1/dashboard/statistics/{fiscal_year}",
        "Public",
        serde_json::json!({ "pathParams": { "fiscal_year": ctx.fiscal_year } }),
        serde_json::json!({ "fiscalYear": ctx.fiscal_year, "sampleBills": sample_bill }),
    );

    let member_suffixes = [
        "",
        "/{fiscal_year}",
        "/total_users",
        "/total_users/{fiscal_year}",
        "/total",
        "/total/{fiscal_year}",
        "/total_active_users",
        "/total_active_users/{fiscal_year}",
        "/active",
        "/active/{fiscal_year}",
        "/new_users",
        "/new_users/{fiscal_year}",
        "/new",
        "/new/{fiscal_year}",
        "/displaced_users",
        "/displaced_users/{fiscal_year}",
        "/displaced",
        "/displaced/{fiscal_year}",
        "/inactive_year",
        "/inactive_year/{fiscal_year}",
        "/total_inactive_users",
        "/total_inactive_users/{fiscal_year}",
        "/inactive",
        "/inactive/{fiscal_year}",
    ];
    for prefix in ["/api/v1/dashboard/members", "/api/v1/members"] {
        for suffix in member_suffixes {
            let path = format!("{prefix}{suffix}");
            let req = if suffix.contains("{fiscal_year}") {
                serde_json::json!({ "pathParams": { "fiscal_year": ctx.fiscal_year } })
            } else {
                serde_json::json!({})
            };
            push_route(
                &mut routes,
                "GET",
                path,
                "Public",
                req,
                serde_json::json!({
                    "fiscalYear": ctx.fiscal_year,
                    "sampleMembers": sample_member,
                    "sampleVehicles": sample_vehicle
                }),
            );
        }
    }

    // TQMS Integrations
    push_route(
        &mut routes,
        "POST",
        "/api/v1/tqms/search",
        "Public",
        serde_json::json!({
            "body": {
                "page": 1,
                "pageSize": 20,
                "vehicleNumber": ctx.vehicle_number,
                "sanghBranch": ctx.branch
            }
        }),
        serde_json::json!({
            "Code": 200,
            "Data": { "cache_results": sample_vehicle, "tqms_results": [] }
        }),
    );
    push_route(
        &mut routes,
        "GET",
        "/api/v1/tqms/validate/{vehicle_number}",
        "Public",
        serde_json::json!({ "pathParams": { "vehicle_number": ctx.vehicle_number } }),
        serde_json::json!({ "Code": 200, "valid": true, "vehicleNumber": ctx.vehicle_number }),
    );
    push_route(
        &mut routes,
        "GET",
        "/api/v1/tqms/vehicle_details/{vehicle_number}",
        "Public",
        serde_json::json!({ "pathParams": { "vehicle_number": ctx.vehicle_number } }),
        serde_json::json!({ "Code": 200, "Data": sample_vehicle }),
    );
    push_route(
        &mut routes,
        "GET",
        "/api/v1/tqms/vehicle_documents/{vehicle_number}",
        "Public",
        serde_json::json!({ "pathParams": { "vehicle_number": ctx.vehicle_number } }),
        serde_json::json!({ "Code": 200, "Message": "Success", "Data": [] }),
    );
    push_route(
        &mut routes,
        "GET",
        "/api/v1/tqms/sync",
        "Public",
        serde_json::json!({}),
        serde_json::json!({ "stream": "SSE / NDJSON sync summary", "sampleVehicles": sample_vehicle }),
    );
    push_route(
        &mut routes,
        "GET",
        "/api/v1/tqms/sync/{concurency}",
        "Public",
        serde_json::json!({ "pathParams": { "concurency": 8 } }),
        serde_json::json!({ "stream": "SSE / NDJSON sync summary", "sampleVehicles": sample_vehicle }),
    );
    push_route(
        &mut routes,
        "GET",
        "/api/v1/tqms/stream_sync/{batch_size}/{concurrency}",
        "Public",
        serde_json::json!({ "pathParams": { "batch_size": 100, "concurrency": 8 } }),
        serde_json::json!({ "stream": "SSE / NDJSON batch progress", "sampleVehicles": sample_vehicle }),
    );
    push_route(
        &mut routes,
        "GET",
        "/api/v1/read_all/VehicleTypes",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({}),
        sample_values(&samples_map, VehicleCategoryResource),
    );
    push_route(
        &mut routes,
        "GET",
        "/api/v1/get_vehicles_tqms",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({}),
        serde_json::json!({ "vehicles": sample_vehicle }),
    );

    // Lodbod Integration
    push_route(
        &mut routes,
        "POST",
        "/api/integrations/lodbod/v1/vehicles/billing-details",
        "Bearer LODBOD_SERVICE_TOKEN or Admin/Operator JWT",
        serde_json::json!({
            "body": {
                "vehicleId": ctx.vehicle_id,
                "vehicleNumber": ctx.vehicle_number,
                "sthaniyaBranch": ctx.branch
            }
        }),
        serde_json::json!({
            "success": true,
            "vehicle": sample_vehicle,
            "feeHeads": sample_rule
        }),
    );
    push_route(
        &mut routes,
        "POST",
        "/api/integrations/lodbod/v1/payments/vehicle-receipts",
        "Bearer LODBOD_SERVICE_TOKEN or Admin/Operator JWT",
        serde_json::json!({
            "body": {
                "lodbodPaymentReferenceId": ctx.payment_ref,
                "vehicleId": ctx.vehicle_id,
                "vehicleNumber": ctx.vehicle_number,
                "fiscalYear": ctx.fiscal_year,
                "sthaniyaBranch": ctx.branch,
                "items": [{ "feeHeadId": ctx.fee_head_id, "amount": 500 }]
            }
        }),
        serde_json::json!({ "success": true, "receipt": sample_bill }),
    );
    push_route(
        &mut routes,
        "GET",
        "/api/integrations/lodbod/v1/payments/{lodbodPaymentReferenceId}/vehicle-receipts",
        "Bearer LODBOD_SERVICE_TOKEN or Admin/Operator JWT",
        serde_json::json!({ "pathParams": { "lodbodPaymentReferenceId": ctx.payment_ref } }),
        serde_json::json!({ "success": true, "receipts": sample_bill }),
    );

    // Bills
    push_route(
        &mut routes,
        "POST",
        "/api/v1/bills/create",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({
            "body": first_sample_inner(&samples_map, BillResource, serde_json::json!({
                "vehicleNumber": ctx.vehicle_number,
                "fiscalYear": ctx.fiscal_year,
                "total": 500
            }))
        }),
        serde_json::json!({ "success": true, "id": ctx.bill_id, "bill": sample_bill }),
    );
    push_route(
        &mut routes,
        "PUT",
        "/api/v1/bills/modify/{id}",
        "Bearer JWT (Admin|Operator)",
        serde_json::json!({
            "pathParams": { "id": ctx.bill_id },
            "body": first_sample_inner(&samples_map, BillResource, serde_json::json!({ "total": 500 }))
        }),
        serde_json::json!({ "success": true, "id": ctx.bill_id, "bill": sample_bill }),
    );
    push_route(
        &mut routes,
        "DELETE",
        "/api/v1/bills/delete/{id}",
        "Bearer JWT (Admin)",
        serde_json::json!({ "pathParams": { "id": ctx.bill_id } }),
        serde_json::json!({ "success": true, "deletedId": ctx.bill_id }),
    );
    push_route(
        &mut routes,
        "POST",
        "/api/v1/bills/rollback/{id}",
        "Bearer JWT (Admin)",
        serde_json::json!({ "pathParams": { "id": ctx.bill_id } }),
        serde_json::json!({ "success": true, "rolledBackId": ctx.bill_id }),
    );
    push_route(
        &mut routes,
        "GET",
        "/api/v1/bills/logs",
        "Bearer JWT (Admin)",
        serde_json::json!({}),
        sample_logs.clone(),
    );
    push_route(
        &mut routes,
        "GET",
        "/api/v1/bills/logs/{bill_id}",
        "Bearer JWT (Admin)",
        serde_json::json!({ "pathParams": { "bill_id": ctx.bill_id } }),
        sample_logs.clone(),
    );

    // SMS
    push_route(
        &mut routes,
        "GET",
        "/api/v1/sms/templates",
        "Bearer JWT",
        serde_json::json!({}),
        serde_json::json!({ "templates": [], "settingsSample": sample_setting }),
    );
    push_route(
        &mut routes,
        "GET",
        "/api/v1/sms/templates/{template_id}",
        "Bearer JWT",
        serde_json::json!({ "pathParams": { "template_id": "payment_receipt" } }),
        serde_json::json!({ "id": "payment_receipt", "template": "..." }),
    );
    push_route(
        &mut routes,
        "POST",
        "/api/v1/sms/preview",
        "Bearer JWT",
        serde_json::json!({ "body": { "templateId": "payment_receipt", "vehicleNumber": ctx.vehicle_number } }),
        serde_json::json!({ "message": "Preview text" }),
    );
    push_route(
        &mut routes,
        "POST",
        "/api/v1/sms/send",
        "Bearer JWT",
        serde_json::json!({ "body": { "templateId": "payment_receipt", "to": "9800000000" } }),
        serde_json::json!({ "success": true, "queued": true }),
    );
    push_route(
        &mut routes,
        "POST",
        "/api/v1/sms/send-raw",
        "Bearer JWT",
        serde_json::json!({ "body": { "to": "9800000000", "message": "Hello" } }),
        serde_json::json!({ "success": true }),
    );

    // Access & Permissions
    push_route(
        &mut routes,
        "GET",
        "/api/v1/access/me",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({}),
        serde_json::json!({ "permissions": sample_perm }),
    );
    push_route(
        &mut routes,
        "GET",
        "/api/v1/access/user/{user_id}",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({ "pathParams": { "user_id": ctx.user_id } }),
        serde_json::json!({ "userId": ctx.user_id, "permissions": sample_perm }),
    );
    push_route(
        &mut routes,
        "POST",
        "/api/v1/access/permission",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({ "body": first_sample_inner(&samples_map, PermissionResource, serde_json::json!({ "userId": ctx.user_id })) }),
        serde_json::json!({ "success": true, "permission": sample_perm }),
    );
    push_route(
        &mut routes,
        "PUT",
        "/api/v1/access/user/{user_id}",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({
            "pathParams": { "user_id": ctx.user_id },
            "body": { "allowed": [] }
        }),
        serde_json::json!({ "success": true, "userId": ctx.user_id }),
    );
    push_route(
        &mut routes,
        "GET",
        "/api/v1/permissions/catalog",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({}),
        serde_json::json!({ "schemaVersion": "1.0", "resources": narayani_auth::action_catalog() }),
    );
    push_route(
        &mut routes,
        "GET",
        "/api/v1/permissions/me",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({}),
        serde_json::json!({ "policy": sample_perm }),
    );
    push_route(
        &mut routes,
        "GET",
        "/api/v1/permissions",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({}),
        sample_perm.clone(),
    );
    push_route(
        &mut routes,
        "POST",
        "/api/v1/permissions",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({ "body": first_sample_inner(&samples_map, PermissionResource, serde_json::json!({ "role": "Cashier" })) }),
        serde_json::json!({ "success": true, "policies": sample_perm }),
    );
    push_route(
        &mut routes,
        "GET",
        "/api/v1/permissions/{tenant_id}/{role}",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({ "pathParams": { "tenant_id": "loadboard-sthaniya", "role": "Admin" } }),
        serde_json::json!({ "policy": sample_perm }),
    );
    push_route(
        &mut routes,
        "PUT",
        "/api/v1/permissions/{id}",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({ "pathParams": { "id": 1 }, "body": { "status": "ACTIVE" } }),
        serde_json::json!({ "success": true, "policy": sample_perm }),
    );
    push_route(
        &mut routes,
        "POST",
        "/api/v1/permissions/check",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({ "body": { "resource": "BillResource", "action": "READ" } }),
        serde_json::json!({ "allowed": true }),
    );
    push_route(
        &mut routes,
        "POST",
        "/api/v1/permissions/seed-defaults",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({ "body": { "tenantId": ctx.branch } }),
        serde_json::json!({ "success": true }),
    );
    push_route(
        &mut routes,
        "POST",
        "/api/v1/permissions/repair",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({}),
        serde_json::json!({ "success": true }),
    );

    // Logs & Audit
    push_route(
        &mut routes,
        "GET",
        "/api/v1/logs/actions",
        "Bearer JWT (Admin)",
        serde_json::json!({ "query": { "limit": 50 } }),
        sample_logs.clone(),
    );
    push_route(
        &mut routes,
        "GET",
        "/api/v1/logs/actions/{resource_name}",
        "Bearer JWT (Admin)",
        serde_json::json!({ "pathParams": { "resource_name": "BillResource" } }),
        sample_logs,
    );
    push_route(
        &mut routes,
        "GET",
        "/api/v1/audit",
        "Bearer JWT (Admin)",
        serde_json::json!({ "query": { "limit": 50 } }),
        sample_audit.clone(),
    );
    push_route(
        &mut routes,
        "GET",
        "/api/v1/audit/entity/{resource_name}/{resource_id}",
        "Bearer JWT (Admin)",
        serde_json::json!({ "pathParams": { "resource_name": "BillResource", "resource_id": ctx.bill_id } }),
        sample_audit.clone(),
    );
    push_route(
        &mut routes,
        "GET",
        "/api/v1/audit/summary",
        "Bearer JWT (Admin)",
        serde_json::json!({}),
        serde_json::json!({ "totalEntries": 0, "sampleEntries": sample_audit }),
    );

    // Settings
    push_route(
        &mut routes,
        "GET",
        "/api/v1/settings/resolved",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({}),
        serde_json::json!({ "settings": sample_setting }),
    );
    push_route(
        &mut routes,
        "POST",
        "/api/v1/settings/billing/calculate-vat",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({ "body": { "amount": 1000, "billingEntityId": 1 } }),
        serde_json::json!({ "baseAmount": 1000, "vatAmount": 130, "totalAmount": 1130 }),
    );
    push_route(
        &mut routes,
        "GET",
        "/api/v1/settings/billing/arrears/{vehicle}",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({ "pathParams": { "vehicle": ctx.vehicle_number } }),
        serde_json::json!({ "vehicle": ctx.vehicle_number, "arrears": [] }),
    );
    push_route(
        &mut routes,
        "POST",
        "/api/v1/settings/billing/linked-fees/evaluate",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({ "body": { "vehicle": ctx.vehicle_number, "feeHeadIds": [ctx.fee_head_id] } }),
        serde_json::json!({ "evaluated": sample_rule }),
    );
    push_route(
        &mut routes,
        "GET",
        "/api/v1/settings/receipts/cancellation-check/{bill_id}",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({ "pathParams": { "bill_id": ctx.bill_id } }),
        serde_json::json!({ "billId": ctx.bill_id, "canCancel": true }),
    );
    push_route(
        &mut routes,
        "GET",
        "/api/v1/settings/insurance/eligibility/{vehicle}",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({ "pathParams": { "vehicle": ctx.vehicle_number } }),
        serde_json::json!({ "vehicle": ctx.vehicle_number, "eligible": true }),
    );
    push_route(
        &mut routes,
        "GET",
        "/api/v1/settings/fiscal-year/selector",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({}),
        serde_json::json!({ "activeFiscalYear": ctx.fiscal_year, "available": [ctx.fiscal_year] }),
    );
    push_route(
        &mut routes,
        "GET",
        "/api/v1/settings/fiscal-year/access/{fiscal_year}",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({ "pathParams": { "fiscal_year": ctx.fiscal_year } }),
        serde_json::json!({ "fiscalYear": ctx.fiscal_year, "access": "READ_WRITE" }),
    );
    push_route(
        &mut routes,
        "GET",
        "/api/v1/settings/vehicle-management/displaced-tagging",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({}),
        serde_json::json!({ "enabled": true, "settings": sample_setting }),
    );

    // Reports
    push_route(
        &mut routes,
        "GET",
        "/api/v1/reports/filter-options",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({}),
        serde_json::json!({
            "fiscalYears": [ctx.fiscal_year],
            "branches": [ctx.branch],
            "feeHeads": sample_rule
        }),
    );
    for (path, desc) in [
        ("/api/v1/reports/income", "Paginated income report"),
        ("/api/v1/reports/income/export", "CSV/XLSX export of income report"),
        ("/api/v1/reports/fee-head-wise", "Paginated fee-head-wise breakdown"),
        ("/api/v1/reports/fee-head-wise/export", "CSV/XLSX export of fee-head-wise breakdown"),
        ("/api/v1/reports/vehicle-category", "Paginated vehicle-category breakdown"),
        ("/api/v1/reports/vehicle-category/export", "CSV/XLSX export of vehicle-category breakdown"),
    ] {
        push_route(
            &mut routes,
            "GET",
            path,
            "Bearer JWT (User|Admin|Operator)",
            serde_json::json!({ "query": { "fiscalYear": ctx.fiscal_year, "branch": ctx.branch } }),
            serde_json::json!({ "description": desc, "sampleBills": sample_bill }),
        );
    }
    for method in ["POST", "GET"] {
        push_route(
            &mut routes,
            method,
            "/api/v1/reports/mahasang",
            "Bearer JWT (User|Admin|Operator)",
            serde_json::json!({ "fiscalYear": ctx.fiscal_year, "branch": ctx.branch }),
            serde_json::json!({ "rows": [], "sampleBills": sample_bill }),
        );
    }
    push_route(
        &mut routes,
        "POST",
        "/api/v1/reports/mahasang/export",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({ "body": { "fiscalYear": ctx.fiscal_year, "format": "xlsx" } }),
        serde_json::json!({ "contentType": "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" }),
    );
    push_route(
        &mut routes,
        "GET",
        "/api/v1/reports/member-transactions/search",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({ "query": { "q": ctx.vehicle_number } }),
        serde_json::json!({ "matches": sample_vehicle }),
    );
    push_route(
        &mut routes,
        "GET",
        "/api/v1/reports/member-transactions/{vehicle_key}",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({ "pathParams": { "vehicle_key": ctx.vehicle_number } }),
        serde_json::json!({ "vehicle": sample_vehicle, "transactions": sample_bill }),
    );
    push_route(
        &mut routes,
        "GET",
        "/api/v1/reports/member-transactions/{vehicle_key}/export",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({ "pathParams": { "vehicle_key": ctx.vehicle_number }, "query": { "format": "csv" } }),
        serde_json::json!({ "contentType": "text/csv; charset=utf-8" }),
    );

    // Vehicle History
    push_route(
        &mut routes,
        "GET",
        "/api/v1/vehicles/history",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({}),
        sample_history.clone(),
    );
    push_route(
        &mut routes,
        "POST",
        "/api/v1/vehicles/history/record",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({ "body": { "vehicleNumber": ctx.vehicle_number, "event": "VEHICLE_NUMBER_CHANGE" } }),
        serde_json::json!({ "success": true, "history": sample_history }),
    );
    push_route(
        &mut routes,
        "GET",
        "/api/v1/vehicles/history/{identifier}",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({ "pathParams": { "identifier": ctx.vehicle_number } }),
        serde_json::json!({ "identifier": ctx.vehicle_number, "history": sample_history }),
    );

    // File Upload & Public File Display
    push_route(
        &mut routes,
        "POST",
        "/api/v1/files/upload",
        "Bearer JWT (User|Admin|Operator)",
        serde_json::json!({ "contentType": "multipart/form-data (field: file) or raw binary + X-File-Name" }),
        serde_json::json!({
            "success": true,
            "id": 1,
            "storedName": ctx.file_name,
            "url": format!("/files/{}", ctx.file_name),
            "sampleFiles": sample_file
        }),
    );
    push_route(
        &mut routes,
        "GET",
        "/files/{filename}",
        "Public (No auth/token required)",
        serde_json::json!({ "pathParams": { "filename": ctx.file_name } }),
        serde_json::json!({ "headers": { "Content-Disposition": "inline" }, "body": "<raw file bytes>" }),
    );
    push_route(
        &mut routes,
        "GET",
        "/uploads/{filename}",
        "Public (No auth/token required)",
        serde_json::json!({ "pathParams": { "filename": ctx.file_name } }),
        serde_json::json!({ "headers": { "Content-Disposition": "inline" }, "body": "<raw file bytes>" }),
    );

    // Generic Resource CRUD + Search + Streams with real DB samples per resource
    for resource in resource_list {
        let r_str = resource.to_string();
        let canonical = narayani_auth::canonical_resource_name(resource);
        let r_samples = database_samples
            .get(canonical)
            .cloned()
            .unwrap_or_else(|| serde_json::json!([]));
        let sample_id = samples_map
            .get(&resource)
            .and_then(|v| v.first())
            .and_then(|r| r.id)
            .unwrap_or(1);
        let sample_inner = first_sample_inner(&samples_map, resource, serde_json::json!({}));

        push_route(
            &mut routes,
            "POST",
            format!("/api/v1/create/{r_str}"),
            "Bearer JWT (User|Admin|Operator)",
            serde_json::json!({ "body": { "inner_content": sample_inner } }),
            serde_json::json!({ "id": sample_id, "resource": canonical }),
        );
        push_route(
            &mut routes,
            "GET",
            format!("/api/v1/read/{r_str}/{{id}}"),
            "Bearer JWT (User|Admin|Operator)",
            serde_json::json!({ "pathParams": { "id": sample_id }, "note": "supports id=all" }),
            r_samples.clone(),
        );
        push_route(
            &mut routes,
            "PUT",
            format!("/api/v1/update/{r_str}/{{id}}"),
            "Bearer JWT (User|Admin|Operator)",
            serde_json::json!({ "pathParams": { "id": sample_id }, "body": { "inner_content": sample_inner } }),
            serde_json::json!({ "success": true, "id": sample_id }),
        );
        push_route(
            &mut routes,
            "DELETE",
            format!("/api/v1/delete/{r_str}/{{id}}"),
            "Bearer JWT (User|Admin|Operator)",
            serde_json::json!({ "pathParams": { "id": sample_id } }),
            serde_json::json!({ "success": true, "deletedId": sample_id }),
        );
        push_route(
            &mut routes,
            "GET",
            format!("/api/v1/search/{r_str}/{{search_query}}/{{search_item}}/{{limit}}"),
            "Bearer JWT (User|Admin|Operator)",
            serde_json::json!({ "pathParams": { "search_query": "id", "search_item": sample_id, "limit": 10 } }),
            r_samples.clone(),
        );
        push_route(
            &mut routes,
            "GET",
            format!("/api/v1/streams/search/{r_str}/{{search_query}}/{{search_item}}/{{limit}}/{{offset}}"),
            "Bearer JWT (User|Admin|Operator)",
            serde_json::json!({ "pathParams": { "search_query": "all", "search_item": "all", "limit": 50, "offset": 0 } }),
            serde_json::json!({ "format": "NDJSON", "sampleRows": r_samples }),
        );
    }

    HttpResponse::Ok().json(serde_json::json!({
        "totalRoutes": routes.len(),
        "databaseSamples": database_samples,
        "routes": routes,
    }))
}

//Check if user with email admin@narayanisangh.com exists, if not create one with password Admin@123
pub async fn create_admin_user_if_not_exists(db: &Database) {
    let email = "admin@narayanisangh.com";
    let password = "Admin@123";
    let setting_branch = access::get_setting_sangh_branch_from_db(db).await;

    let db_user = db.read_all_resource(resource::ResourceName::UserResource).await;
    if let Ok(users) = db_user {
        if !users.is_empty() {
            println!("Admin user already exists");
            if let Some(admin) = users.iter().find(|u| {
                u.inner_content
                    .as_ref()
                    .and_then(|c| c.get("email"))
                    .and_then(|e| e.as_str())
                    .map(|e| e.eq_ignore_ascii_case(email))
                    .unwrap_or(false)
            }) {
                if let (Some(admin_id), Some(admin_inner)) = (admin.id, &admin.inner_content) {
                    if let Err(err) = access::ensure_permission_in_db_for_user(
                        db,
                        admin_id,
                        admin_inner,
                        setting_branch,
                    )
                    .await
                    {
                        eprintln!(
                            "Failed to ensure PermissionResource for existing admin user {}: {}",
                            admin_id, err
                        );
                    }
                }
            }
            return;
        }
    }
    let mut admin_map = serde_json::Map::new();
    admin_map.insert(
        "email".to_string(),
        serde_json::Value::String(email.to_string()),
    );
    admin_map.insert(
        "password".to_string(),
        serde_json::Value::String(password.to_string()),
    );
    admin_map.insert(
        "role".to_string(),
        serde_json::Value::String("Admin".to_string()),
    );
    if let Some(ref branch) = setting_branch {
        admin_map.insert(
            "SanghBranch".to_string(),
            serde_json::Value::String(branch.clone()),
        );
    }
    let admin_inner = serde_json::Value::Object(admin_map);

    let resource = DatabaseResource {
        id: None,
        inner_content: Some(admin_inner.clone()),
        external_content: None,
        resource_name: Some(resource::ResourceName::UserResource),
    };
    match db
        .create_resource(resource::ResourceName::UserResource, resource)
        .await
    {
        Ok(admin_id) => {
            println!("Admin user created successfully");
            if let Err(err) = access::ensure_permission_in_db_for_user(
                db,
                admin_id,
                &admin_inner,
                setting_branch,
            )
            .await
            {
                eprintln!(
                    "Failed to create 1:1 PermissionResource for admin user {}: {}",
                    admin_id, err
                );
            }
        }
        Err(err) => {
            println!("Failed to create admin user: {:?}", err);
        }
    }
}

pub async fn add_sthaniya_branch_to_billing_rule_resources(db: &Database) {
    let items = match db
        .read_all_resource(resource::ResourceName::BillingRuleResource)
        .await
    {
        Ok(items) => items,
        Err(err) => {
            eprintln!(
                "Failed to read BillingRuleResource for sthaniyaBranch migration: {:?}",
                err
            );
            return;
        }
    };

    let mut updated = 0usize;
    let mut skipped = 0usize;

    for mut item in items {
        let Some(id) = item.id else {
            continue;
        };

        let Some(serde_json::Value::Object(ref mut map)) = item.inner_content else {
            continue;
        };

        if map.get("sthaniyaBranch").and_then(|v| v.as_str()) == Some("Loadboard sthaniya") {
            skipped += 1;
            continue;
        }

        map.insert(
            "sthaniyaBranch".to_string(),
            serde_json::Value::String("Loadboard sthaniya".to_string()),
        );

        match db
            .update_resource(resource::ResourceName::BillingRuleResource, id, item)
            .await
        {
            Ok(()) => updated += 1,
            Err(err) => {
                eprintln!(
                    "Failed to update BillingRuleResource {} with sthaniyaBranch: {:?}",
                    id, err
                );
            }
        }
    }

    println!(
        "BillingRuleResource sthaniyaBranch migration complete: {} updated, {} already up-to-date",
        updated, skipped
    );
}

pub async fn migrate_fiscal_year_id_to_fiscal_year(db: &Database) {
    for resource_name in [
        resource::ResourceName::BillingRuleResource,
        resource::ResourceName::BillResource,
    ] {
        let items = match db.read_all_resource(resource_name).await {
            Ok(items) => items,
            Err(err) => {
                eprintln!(
                    "Failed to read {:?} for fiscalYearId migration: {:?}",
                    resource_name, err
                );
                continue;
            }
        };

        let mut updated = 0usize;
        let mut skipped = 0usize;

        for mut item in items {
            let Some(id) = item.id else {
                continue;
            };

            let Some(serde_json::Value::Object(ref map)) = item.inner_content else {
                continue;
            };

            let has_fiscal_year_id = map.keys().any(|k| k.eq_ignore_ascii_case("fiscalyearid"));
            if !has_fiscal_year_id {
                skipped += 1;
                continue;
            }

            let mut new_map = serde_json::Map::with_capacity(map.len());
            let mut inserted_fiscal_year = false;

            for (k, v) in map.iter() {
                if k.eq_ignore_ascii_case("fiscalyearid") {
                    if !inserted_fiscal_year {
                        new_map.insert(
                            "fiscalYear".to_string(),
                            serde_json::Value::String("2082/2083".to_string()),
                        );
                        inserted_fiscal_year = true;
                    }
                    continue;
                }
                if k.eq_ignore_ascii_case("fiscalyear") {
                    continue;
                }
                new_map.insert(k.clone(), v.clone());
            }

            item.inner_content = Some(serde_json::Value::Object(new_map));

            match db.update_resource(resource_name, id, item).await {
                Ok(()) => updated += 1,
                Err(err) => {
                    eprintln!(
                        "Failed to migrate fiscalYearId on {:?} {}: {:?}",
                        resource_name, id, err
                    );
                }
            }
        }

        println!(
            "{:?} fiscalYearId -> fiscalYear migration complete: {} updated, {} skipped",
            resource_name, updated, skipped
        );
    }
}

fn load_tls_config() -> io::Result<ServerConfig> {
    let cert_path = env::var("TLS_CERT_PATH").unwrap_or_else(|_| "cert.pem".to_owned());
    let key_path = env::var("TLS_KEY_PATH").unwrap_or_else(|_| "key.pem".to_owned());

    let cert_chain = certs(&mut BufReader::new(File::open(cert_path)?))
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
        .into_iter()
        .map(Certificate)
        .collect();

    let mut key_reader = BufReader::new(File::open(&key_path)?);
    let pkcs8_key = pkcs8_private_keys(&mut key_reader)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
        .into_iter()
        .next()
        .map(PrivateKey);
    let key = match pkcs8_key {
        Some(key) => key,
        None => {
            let mut key_reader = BufReader::new(File::open(&key_path)?);
            rsa_private_keys(&mut key_reader)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
                .into_iter()
                .next()
                .map(PrivateKey)
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "no private key found")
                })?
        }
    };

    ServerConfig::builder()
        .with_safe_defaults()
        .with_no_client_auth()
        .with_single_cert(cert_chain, key)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenv().ok();

    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let client = connect_to_db(&database_url).await.unwrap();

    // Create database tables if they don't exist
    let client = Arc::new(client);
    let db = Database::new(client.clone());

    if env::args().any(|arg| arg == "--run-migrations") {
        add_sthaniya_branch_to_billing_rule_resources(&db).await;
        migrate_fiscal_year_id_to_fiscal_year(&db).await;
        println!("One-off migrations finished; exiting (--run-migrations).");
        return Ok(());
    }
    
    //Delete previous tables
    //db.delete_tables().await.expect("Failed to delete tables");
    //db.delete_all_resources(resource::ResourceName::UserResource).await.expect("Failed to delete user resource");

    if let Err(e) = db.create_tables().await {
        eprintln!("Error creating tables: {}", e);
    }
    if let Err(e) = db.create_report_tables().await {
        eprintln!("Error creating report projection tables: {}", e);
    }
    create_admin_user_if_not_exists(&db).await;
    add_sthaniya_branch_to_billing_rule_resources(&db).await;
    migrate_fiscal_year_id_to_fiscal_year(&db).await;

    let mut cache = cache::Cache::new();
    println!("Loading cache from database...");
    cache.load_from_db(&db).await;
    println!("Cache loaded from database");
    match db.backfill_report_tables_if_empty(&cache.report_index).await {
        Ok(0) => {}
        Ok(count) => println!("Backfilled {count} receipts into report_receipts"),
        Err(e) => eprintln!("Warning: report_receipts startup backfill skipped: {e}"),
    }
    let tqms_client = Arc::new(tqms_integration::TQMSClient::new().await);
    let server_data = ServerData {
        db: Arc::new(db),
        tqms_client,
        cache: Arc::new(tokio::sync::RwLock::new(cache)),
    };
    permissions::hydrate_role_policies(&server_data).await;
    log::init_persistent_action_logger(
        server_data.db.clone(),
        Some(server_data.cache.clone()),
    )
    .await;
    audit::init_persistent_audit_trail(
        server_data.db.clone(),
        Some(server_data.cache.clone()),
    )
    .await;
    let _tls_config = load_tls_config()?;
    let app_port = env::var("APP_PORT").unwrap_or_else(|_| "5001".to_owned());
    println!("Starting server on port {}", app_port);
    // Create and start HTTPS server
    let server_result = HttpServer::new(move || {
        let cors = Cors::default()
            .allow_any_origin()
            .allow_any_method()
            .allow_any_header()
            .expose_headers([
                "X-Stream-Format",
                "X-Batch-Size",
                "X-Concurrency",
                "X-Total-Items",
                "X-Total-Pages",
                "X-Starting-Offset",
            ])
            .max_age(3600);
        App::new()
            .wrap(cors)
            //.app_data(web::Data::new(server_data.clone()))
            .app_data(web::Data::new(Arc::new(server_data.clone())))
            .route("/", web::get().to(server_routes))
            .service(
                web::scope("/api/v1/auth")
                    .route("/login", web::post().to(login::login))
                    .route("/logout", web::post().to(access::logout)),
            )
            .service(
                web::scope("/api/v1/access")
                    .wrap(middleware::AuthMiddleware::with_roles(&[
                        roles::Roles::User,
                        roles::Roles::Admin,
                        roles::Roles::Operator,
                    ]))
                    .configure(access::configure_routes),
            )
            .service(
                web::scope("/api/v1/permissions")
                    .wrap(middleware::AuthMiddleware::with_roles(&[
                        roles::Roles::User,
                        roles::Roles::Admin,
                        roles::Roles::Operator,
                    ]))
                    .configure(permissions::configure_routes),
            )
            // .service(
            //     web::scope("/api/v1/Developer")
            //     .route("/create/user", web::post().to(routes::create_user))
            //     .route("/delete/user/{id}", web::post().to(routes::delete_user))
            //     .app_data(web::Data::new(Arc::new(vec![roles::Roles::Developer])))//Store Role
            //     .wrap(middleware::AuthMiddleware::with_role(roles::Roles::Developer))
            // )
            // .service(
            //     web::scope("/api/v1/Operator")
            //     .route("/Create/user", web::post().to(routes::create_user))
            //     .route("/Delete/user/{id}", web::post().to(routes::delete_user))
            //     .route("/Create/{resource_name}", web::post().to(routes::create_resource))
            //     .route("/Read/{resource_name}/{id}", web::post().to(routes::read_resource))
            //     .app_data(web::Data::new(Arc::new(vec![roles::Roles::Operator])))//Store Role
            //     .wrap(middleware::AuthMiddleware::with_roles(&[roles::Roles::Operator, roles::Roles::Administrator, roles::Roles::Developer]))
            // )
            // .service(
            //     web::scope("/api/v1/Administrator")
            //     .route("/Create/user", web::post().to(routes::create_user))
            //     .route("/Delete/user/{id}", web::post().to(routes::delete_user))
            //     .route("/Create/{resource_name}", web::post().to(routes::create_resource))
            //     .route("/Read/{resource_name}/{id}", web::post().to(routes::read_resource))
            //     .app_data(web::Data::new(Arc::new(vec![roles::Roles::Administrator])))//Store Role
            //     .wrap(middleware::AuthMiddleware::with_roles(&[roles::Roles::Administrator, roles::Roles::Developer]))
            // )
            // .service(
            //     web::scope("/api/v1/Staff")
            //     .route("/Create/{resource_name}", web::post().to(routes::create_resource))
            //     .route("/Read/{resource_name}/{id}", web::post().to(routes::read_resource))
            //     .app_data(web::Data::new(Arc::new(vec![roles::Roles::Staff])))//Store Role
            //     .wrap(middleware::AuthMiddleware::with_roles(&[roles::Roles::Administrator, roles::Roles::Developer, roles::Roles::Operator, roles::Roles::Staff]))
            // )
            // .service(
            //     web::scope("/api/v1/public")
            //     .route("/Read/{resource_name}/{id}", web::post().to(routes::read_resource))
            //     .app_data(web::Data::new(Arc::new(vec![roles::Roles::Visitor])))//Store Role
            // )
            .service(
                web::scope("/api/v1/dashboard")
                    .route(
                        "/statistics",
                        web::get().to(dashboard_statistics::get_dashboard_statistics),
                    )
                    .route(
                        "/statistics/{fiscal_year:.*}",
                        web::get().to(dashboard_statistics::get_dashboard_statistics_by_fy),
                    )
                    .service(web::scope("/members").configure(members::configure_routes)),
            )
            .service(web::scope("/api/v1/members").configure(members::configure_routes))
            .service(web::scope("/api/v1/sms").configure(sms::configure_routes))
            .service(
                web::scope("/api/v1/tqms")
                    .route("/search", web::post().to(tqms_integration::search_vehicles))
                    .route("/validate/{vehicle_number}", web::get().to(tqms_integration::validate_vehicle))
                    .route("/vehicle_details/{vehicle_number}", web::get().to(tqms_integration::get_vehicle_details))
                    .route("/vehicle_documents/{vehicle_number}", web::get().to(tqms_integration::get_vehicle_documents))
                    .route("/sync", web::get().to(tqms_integration::sync_vehicles_tqms))
                    .route("/sync/{concurency}", web::get().to(tqms_integration::sync_vehicle_tqms_concurrency_limited))
                    .route("/stream_sync/{batch_size}/{concurrency}", web::get().to(tqms_integration::stream_sync_vehicles_tqms))
            )
            .service(
                web::scope("/api/v1/streams")
                .route("/search/{resource_name}/{search_query}/{search_item}/{limit}/{offset}", web::get().to(api_stream::search))
                .wrap(actix_web::middleware::Compress::default())
                .wrap(middleware::AuthMiddleware::with_roles(&[
                    roles::Roles::User,
                    roles::Roles::Admin,
                    roles::Roles::Operator,
                ]))
            )
            .service(
                web::scope("/api/integrations/lodbod/v1")
                    .route(
                        "/vehicles/billing-details",
                        web::post().to(billing_actions::get_vehicle_billing_details),
                    )
                    .route(
                        "/payments/vehicle-receipts",
                        web::post().to(billing_actions::submit_vehicle_payment),
                    )
                    .route(
                        "/payments/{lodbodPaymentReferenceId}/vehicle-receipts",
                        web::get().to(billing_actions::get_payment_receipt_status),
                    ),
            )
            .service(
                web::scope("/api/v1/bills")
                    .service(
                        web::resource("/create")
                            .route(web::post().to(billing_actions::create_bill))
                            .wrap(middleware::AuthMiddleware::with_roles(&[
                                roles::Roles::User,
                                roles::Roles::Admin,
                                roles::Roles::Operator,
                            ])),
                    )
                    .service(
                        web::resource("/modify/{id}")
                            .route(web::put().to(billing_actions::modify_bill))
                            .wrap(middleware::AuthMiddleware::with_roles(&[
                                roles::Roles::Admin,
                                roles::Roles::Operator,
                            ])),
                    )
                    .service(
                        web::resource("/delete/{id}")
                            .route(web::delete().to(billing_actions::delete_bill))
                            .wrap(middleware::AuthMiddleware::with_role(roles::Roles::Admin)),
                    )
                    .service(
                        web::resource("/rollback/{id}")
                            .route(web::post().to(billing_actions::rollback_bill))
                            .wrap(middleware::AuthMiddleware::with_role(roles::Roles::Admin)),
                    )
                    .service(
                        web::resource("/logs")
                            .route(web::get().to(billing_actions::get_bill_logs))
                            .wrap(middleware::AuthMiddleware::with_role(roles::Roles::Admin)),
                    )
                    .service(
                        web::resource("/logs/{bill_id}")
                            .route(web::get().to(billing_actions::get_bill_logs_by_bill_id))
                            .wrap(middleware::AuthMiddleware::with_role(roles::Roles::Admin)),
                    ),
            )
            .service(
                web::scope("/api/v1/vehicles/history")
                    .wrap(middleware::AuthMiddleware::with_roles(&[
                        roles::Roles::User,
                        roles::Roles::Admin,
                        roles::Roles::Operator,
                    ]))
                    .configure(vehicle_history::configure_routes),
            )
            .service(
                web::scope("/api/v1/settings")
                    .wrap(middleware::AuthMiddleware::with_roles(&[
                        roles::Roles::User,
                        roles::Roles::Admin,
                        roles::Roles::Operator,
                    ]))
                    .configure(settings::configure_routes),
            )
            .service(
                web::scope("/api/v1/reports")
                    .wrap(middleware::AuthMiddleware::with_roles(&[
                        roles::Roles::User,
                        roles::Roles::Admin,
                        roles::Roles::Operator,
                    ]))
                    .configure(reports::configure_routes),
            )
            .service(
                web::scope("/api/v1/logs")
                    .wrap(middleware::AuthMiddleware::with_role(roles::Roles::Admin))
                    .configure(log::configure_routes),
            )
            .service(
                web::scope("/api/v1/audit")
                    .wrap(middleware::AuthMiddleware::with_role(roles::Roles::Admin))
                    .configure(audit::configure_routes),
            )
            .route("/files/{filename}", web::get().to(file_upload::serve_public_file))
            .route("/uploads/{filename}", web::get().to(file_upload::serve_public_file))
            .service(
                web::scope("/api/v1/files")
                    .wrap(middleware::AuthMiddleware::with_roles(&[
                        roles::Roles::User,
                        roles::Roles::Admin,
                        roles::Roles::Operator,
                    ]))
                    .app_data(web::PayloadConfig::new(26 * 1024 * 1024))
                    .route("/upload", web::post().to(file_upload::upload_file)),
            )
            .service(
                web::scope("/api/v1")
                    .route(
                        "/update/{resource_name}/{id}",
                        web::put().to(actions::update),
                    )
                    .route(
                        "/delete/{resource_name}/{id}",
                        web::delete().to(actions::delete),
                    )
                    .wrap(middleware::AuthMiddleware::with_roles(&[
                        roles::Roles::User,
                        roles::Roles::Admin,
                        roles::Roles::Operator,
                    ]))
                    .route("/create/{resource_name}", web::post().to(actions::write))
                    .route(
                        "/search/{resource_name}/{search_query}/{search_item}/{limit}",
                        web::get().to(actions::search),
                    )
                    .route(
                        "/read_all/VehicleTypes",
                        web::get().to(tqms_integration::read_all_vehicle_types),
                    )
                    .route("/read/{resource_name}/{id}", web::get().to(actions::read))
                    .route(
                        "/get_vehicles_tqms",
                        web::get().to(tqms_integration::get_vehicle_from_tqms),
                    ),
            )
    })
    .bind(format!("127.0.0.1:{}", app_port))
    .expect("Failed to bind to port")
    .run()
    .await;

    log::flush_action_logs().await;
    audit::flush_audit_trail().await;
    server_result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::test_helpers::{create_mock_server_data_with_state, MockDbState};
    use crate::resource::ResourceName;

    #[actix_web::test]
    async fn test_add_sthaniya_branch_to_billing_rule_resources_updates_all_existing_items() {
        let mut state = MockDbState::default();
        state.seed(
            ResourceName::BillingRuleResource,
            DatabaseResource {
                id: Some(29),
                resource_name: Some(ResourceName::BillingRuleResource),
                inner_content: Some(serde_json::json!({
                    "code": "",
                    "name": "Entry",
                    "status": "ACTIVE",
                    "fiscalYear": "2082/2083",
                    "displayOrder": 11,
                    "resourceType": "FEE_HEAD"
                })),
                external_content: None,
            },
        );
        state.seed(
            ResourceName::BillingRuleResource,
            DatabaseResource {
                id: Some(16),
                resource_name: Some(ResourceName::BillingRuleResource),
                inner_content: Some(serde_json::json!({
                    "scope": "COMMON",
                    "amount": 0,
                    "status": "ACTIVE",
                    "fiscalYear": "2082/2083",
                    "resourceType": "BILLING_RULE",
                    "billingEntityId": 1
                })),
                external_content: Some(serde_json::json!({ "createdTime": "2026-11-16T10:11:27Z" })),
            },
        );
        state.seed(
            ResourceName::BillingRuleResource,
            DatabaseResource {
                id: Some(26),
                resource_name: Some(ResourceName::BillingRuleResource),
                inner_content: Some(serde_json::json!({
                    "scope": "COMMON",
                    "amount": 0,
                    "sthaniyaBranch": "Loadboard sthaniya"
                })),
                external_content: None,
            },
        );

        let (s_data, db_state) = create_mock_server_data_with_state(state).await;

        add_sthaniya_branch_to_billing_rule_resources(&s_data.db).await;

        {
            let guard = db_state.lock().await;
            let rows = guard.tables.get("billingruleresource").unwrap();
            assert_eq!(rows.len(), 3);
            for row in rows {
                let inner = row.inner_content.as_ref().unwrap();
                assert_eq!(
                    inner.get("sthaniyaBranch").and_then(|v| v.as_str()),
                    Some("Loadboard sthaniya")
                );
            }
            let row_29 = rows.iter().find(|r| r.id == Some(29)).unwrap();
            assert_eq!(row_29.inner_content.as_ref().unwrap()["name"], "Entry");
            let row_16 = rows.iter().find(|r| r.id == Some(16)).unwrap();
            assert_eq!(
                row_16.external_content,
                Some(serde_json::json!({ "createdTime": "2026-11-16T10:11:27Z" }))
            );
        }

        // Second run is idempotent
        add_sthaniya_branch_to_billing_rule_resources(&s_data.db).await;
        let guard = db_state.lock().await;
        for row in guard.tables.get("billingruleresource").unwrap() {
            assert_eq!(
                row.inner_content
                    .as_ref()
                    .unwrap()
                    .get("sthaniyaBranch")
                    .and_then(|v| v.as_str()),
                Some("Loadboard sthaniya")
            );
        }
    }

    #[actix_web::test]
    async fn test_migrate_fiscal_year_id_to_fiscal_year_in_billing_rule_and_bill_resources() {
        let mut state = MockDbState::default();

        // BillResource rows
        state.seed(
            ResourceName::BillResource,
            DatabaseResource {
                id: Some(149),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(serde_json::json!({
                    "total": 565,
                    "vehicle": { "id": "36010", "vehicleNumber": "PROVINCE-4-56-02-JHA-8241" },
                    "items": [{ "title": "Entry", "amount": 500 }],
                    "fiscalYearId": "15f21721-50e8-41af-9596-9d8cdb8d6a16",
                    "paymentMethod": "नगद"
                })),
                external_content: Some(serde_json::json!({ "createdTime": "2026-11-17T09:55:09Z" })),
            },
        );
        state.seed(
            ResourceName::BillResource,
            DatabaseResource {
                id: Some(150),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(serde_json::json!({
                    "total": 100,
                    "fiscalyearid": "any-value"
                })),
                external_content: None,
            },
        );
        state.seed(
            ResourceName::BillResource,
            DatabaseResource {
                id: Some(151),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(serde_json::json!({
                    "total": 200,
                    "fiscalYear": "2083/2084"
                })),
                external_content: None,
            },
        );

        // BillingRuleResource rows
        state.seed(
            ResourceName::BillingRuleResource,
            DatabaseResource {
                id: Some(24),
                resource_name: Some(ResourceName::BillingRuleResource),
                inner_content: Some(serde_json::json!({
                    "scope": "COMMON",
                    "amount": 0,
                    "fiscalYearId": "15f21721-50e8-41af-9596-9d8cdb8d6a16",
                    "resourceType": "BILLING_RULE"
                })),
                external_content: None,
            },
        );
        state.seed(
            ResourceName::BillingRuleResource,
            DatabaseResource {
                id: Some(25),
                resource_name: Some(ResourceName::BillingRuleResource),
                inner_content: Some(serde_json::json!({
                    "name": "Fee Head",
                    "fiscalYearId": null,
                    "resourceType": "FEE_HEAD"
                })),
                external_content: None,
            },
        );
        state.seed(
            ResourceName::BillingRuleResource,
            DatabaseResource {
                id: Some(29),
                resource_name: Some(ResourceName::BillingRuleResource),
                inner_content: Some(serde_json::json!({
                    "name": "Entry",
                    "fiscalYear": "2082/2083",
                    "resourceType": "FEE_HEAD"
                })),
                external_content: None,
            },
        );

        let (s_data, db_state) = create_mock_server_data_with_state(state).await;

        migrate_fiscal_year_id_to_fiscal_year(&s_data.db).await;

        {
            let guard = db_state.lock().await;
            let bill_rows = guard.tables.get("billresource").unwrap();
            assert_eq!(bill_rows.len(), 3);

            let b149 = bill_rows.iter().find(|r| r.id == Some(149)).unwrap();
            let b149_inner = b149.inner_content.as_ref().unwrap().as_object().unwrap();
            assert!(!b149_inner.keys().any(|k| k.eq_ignore_ascii_case("fiscalyearid")));
            assert_eq!(b149_inner.get("fiscalYear").and_then(|v| v.as_str()), Some("2082/2083"));
            assert_eq!(b149_inner.get("total").and_then(|v| v.as_i64()), Some(565));
            assert_eq!(
                b149.external_content,
                Some(serde_json::json!({ "createdTime": "2026-11-17T09:55:09Z" }))
            );

            let b150 = bill_rows.iter().find(|r| r.id == Some(150)).unwrap();
            let b150_inner = b150.inner_content.as_ref().unwrap().as_object().unwrap();
            assert!(!b150_inner.keys().any(|k| k.eq_ignore_ascii_case("fiscalyearid")));
            assert_eq!(b150_inner.get("fiscalYear").and_then(|v| v.as_str()), Some("2082/2083"));
            assert_eq!(b150_inner.get("total").and_then(|v| v.as_i64()), Some(100));

            let b151 = bill_rows.iter().find(|r| r.id == Some(151)).unwrap();
            assert_eq!(
                b151.inner_content
                    .as_ref()
                    .unwrap()
                    .get("fiscalYear")
                    .and_then(|v| v.as_str()),
                Some("2083/2084")
            );

            let rule_rows = guard.tables.get("billingruleresource").unwrap();
            assert_eq!(rule_rows.len(), 3);

            for rule_id in [24, 25] {
                let row = rule_rows.iter().find(|r| r.id == Some(rule_id)).unwrap();
                let obj = row.inner_content.as_ref().unwrap().as_object().unwrap();
                assert!(!obj.keys().any(|k| k.eq_ignore_ascii_case("fiscalyearid")));
                assert_eq!(obj.get("fiscalYear").and_then(|v| v.as_str()), Some("2082/2083"));
            }

            let r29 = rule_rows.iter().find(|r| r.id == Some(29)).unwrap();
            assert_eq!(
                r29.inner_content
                    .as_ref()
                    .unwrap()
                    .get("fiscalYear")
                    .and_then(|v| v.as_str()),
                Some("2082/2083")
            );
        }

        // Second run is idempotent
        migrate_fiscal_year_id_to_fiscal_year(&s_data.db).await;
        let guard = db_state.lock().await;
        for row in guard.tables.get("billresource").unwrap() {
            let obj = row.inner_content.as_ref().unwrap().as_object().unwrap();
            assert!(!obj.keys().any(|k| k.eq_ignore_ascii_case("fiscalyearid")));
        }
        let b151 = guard
            .tables
            .get("billresource")
            .unwrap()
            .iter()
            .find(|r| r.id == Some(151))
            .unwrap();
        assert_eq!(
            b151.inner_content
                .as_ref()
                .unwrap()
                .get("fiscalYear")
                .and_then(|v| v.as_str()),
            Some("2083/2084")
        );
    }

    #[actix_web::test]
    async fn test_server_routes_returns_request_response_and_sanitized_db_samples() {
        use actix_web::{http::StatusCode, test, App};

        let mut state = MockDbState::default();
        state.seed(
            ResourceName::VehicleResource,
            DatabaseResource {
                id: Some(11),
                resource_name: Some(ResourceName::VehicleResource),
                inner_content: Some(serde_json::json!({
                    "VehicleId": 41523,
                    "VehicleNumber": "Province-4-15-23-KHA-5555",
                    "SanghBranch": "Gandaki Branch"
                })),
                external_content: None,
            },
        );
        state.seed(
            ResourceName::BillResource,
            DatabaseResource {
                id: Some(22),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(serde_json::json!({
                    "receiptNumber": "RCP-2082-001",
                    "fiscalYear": "2082/2083",
                    "total": 1500
                })),
                external_content: None,
            },
        );
        state.seed(
            ResourceName::UserResource,
            DatabaseResource {
                id: Some(33),
                resource_name: Some(ResourceName::UserResource),
                inner_content: Some(serde_json::json!({
                    "email": "operator@narayanisangh.com",
                    "password": "super-secret-password",
                    "role": "Operator"
                })),
                external_content: None,
            },
        );
        state.seed(
            ResourceName::FileResource,
            DatabaseResource {
                id: Some(44),
                resource_name: Some(ResourceName::FileResource),
                inner_content: Some(serde_json::json!({
                    "originalName": "bluebook.png",
                    "storedName": "12345-1-bluebook.png",
                    "url": "/files/12345-1-bluebook.png"
                })),
                external_content: None,
            },
        );

        let (s_data, _db_state) = create_mock_server_data_with_state(state).await;

        let app = test::init_service(
            App::new()
                .app_data(s_data)
                .route("/", web::get().to(server_routes)),
        )
        .await;

        let req = test::TestRequest::get().uri("/").to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body: serde_json::Value = test::read_body_json(resp).await;
        assert!(body["totalRoutes"].as_u64().unwrap() > 100);

        let samples = &body["databaseSamples"];
        assert_eq!(
            samples["VehicleResource"][0]["inner_content"]["VehicleNumber"],
            "Province-4-15-23-KHA-5555"
        );
        assert_eq!(
            samples["BillResource"][0]["inner_content"]["receiptNumber"],
            "RCP-2082-001"
        );
        assert_eq!(
            samples["UserResource"][0]["inner_content"]["password"],
            "[REDACTED]"
        );
        assert_eq!(
            samples["FileResource"][0]["inner_content"]["storedName"],
            "12345-1-bluebook.png"
        );

        let routes = body["routes"].as_array().expect("routes array");
        assert!(!routes.is_empty());
        for route in routes {
            assert!(route.get("method").and_then(|v| v.as_str()).is_some());
            assert!(route.get("path").and_then(|v| v.as_str()).is_some());
            assert!(route.get("auth").and_then(|v| v.as_str()).is_some());
            assert!(route.get("request").is_some());
            assert!(route.get("response").is_some());
        }

        let tqms_doc_route = routes
            .iter()
            .find(|r| r["path"] == "/api/v1/tqms/vehicle_documents/{vehicle_number}")
            .expect("tqms vehicle_documents route documented");
        assert_eq!(
            tqms_doc_route["request"]["pathParams"]["vehicle_number"],
            "Province-4-15-23-KHA-5555"
        );

        let bill_read_route = routes
            .iter()
            .find(|r| r["path"] == "/api/v1/read/BillResource/{id}")
            .expect("BillResource read route documented");
        assert_eq!(bill_read_route["request"]["pathParams"]["id"], 22);
        assert_eq!(
            bill_read_route["response"][0]["inner_content"]["receiptNumber"],
            "RCP-2082-001"
        );
    }
}
