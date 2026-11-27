use crate::access::{PermissionError, PermissionErrorHttpExt};
use crate::audit::{self, AuditAction, AuditCategory, HttpAuditEvent};
use crate::cache::Cache;
use crate::resource::ResourceName;
use crate::ServerData;
use actix_web::{http::StatusCode, web, HttpMessage, HttpRequest, HttpResponse};
pub use narayani_auth::policy::*;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

pub async fn find_all_role_policies_in_db(
    db: &crate::db::Database,
) -> Result<Vec<RolePermissionResource>, String> {
    let rows = db
        .read_all_resource(ResourceName::PermissionResource)
        .await
        .map_err(|e| format!("Failed to read PermissionResource table: {e:?}"))?;

    let mut policies = Vec::new();
    for row in rows {
        let Some(ref inner) = row.inner_content else {
            continue;
        };
        if is_role_permission_payload(inner) {
            if let Ok(parsed) = RolePermissionResource::from_database_resource(&row) {
                policies.push(parsed);
            }
        }
    }
    Ok(policies)
}

pub async fn find_role_policy_in_db(
    db: &crate::db::Database,
    tenant_id: &str,
    role: &str,
) -> Result<Option<RolePermissionResource>, String> {
    let target_tenant = normalize_tenant_id(tenant_id);
    let target_role = role.trim();
    let all = find_all_role_policies_in_db(db).await?;
    Ok(all.into_iter().find(|p| {
        normalize_tenant_id(&p.inner_content.tenant_id) == target_tenant
            && p.inner_content.role.eq_ignore_ascii_case(target_role)
    }))
}

/// Hydrates v1.0 role policies from DB into `cache.role_policies`.
pub async fn hydrate_role_policies(s_data: &ServerData) -> usize {
    match find_all_role_policies_in_db(&s_data.db).await {
        Ok(policies) => {
            let mut cache = s_data.cache.write().await;
            cache.role_policies.clear();
            let count = policies.len();
            for pol in policies {
                cache.upsert_role_policy(pol);
            }
            count
        }
        Err(err) => {
            eprintln!("Failed to hydrate v1.0 role policies: {err}");
            0
        }
    }
}

/// Validates, enforces uniqueness per `(tenantId, role)`, persists to DB, and updates `Cache`.
pub async fn create_role_policy_in_db(
    s_data: &ServerData,
    mut policy: RolePermissionPolicy,
    external_content: Option<Value>,
) -> Result<RolePermissionResource, PermissionError> {
    normalize_and_validate_policy(&mut policy)
        .map_err(|errs| PermissionError::BadRequest(errs.join("; ")))?;

    let existing = find_role_policy_in_db(&s_data.db, &policy.tenant_id, &policy.role)
        .await
        .map_err(PermissionError::Internal)?;
    if let Some(found) = existing {
        return Err(PermissionError::Conflict(format!(
            "PermissionResource already exists for tenant '{}' and role '{}' (id: {:?})",
            policy.tenant_id, policy.role, found.id
        )));
    }

    let draft = RolePermissionResource {
        id: None,
        resource_name: ResourceName::PermissionResource,
        inner_content: policy,
        external_content,
    };
    let perm_id = s_data
        .db
        .create_resource(ResourceName::PermissionResource, draft.to_database_resource())
        .await
        .map_err(|e| PermissionError::Internal(format!("Failed to create PermissionResource: {e:?}")))?;

    let created = RolePermissionResource {
        id: Some(perm_id),
        ..draft
    };
    s_data.cache.write().await.upsert_role_policy(created.clone());
    Ok(created)
}

/// Validates and updates an existing v1.0 `RolePermissionResource` by row `id`, syncing `Cache`.
pub async fn update_role_policy_in_db(
    s_data: &ServerData,
    id: i32,
    mut policy: RolePermissionPolicy,
    external_content: Option<Value>,
) -> Result<(Option<RolePermissionPolicy>, RolePermissionResource), PermissionError> {
    normalize_and_validate_policy(&mut policy)
        .map_err(|errs| PermissionError::BadRequest(errs.join("; ")))?;

    let all = find_all_role_policies_in_db(&s_data.db)
        .await
        .map_err(PermissionError::Internal)?;

    let before = all
        .iter()
        .find(|p| p.id == Some(id))
        .map(|p| p.inner_content.clone());

    if let Some(conflict) = all.iter().find(|p| {
        p.id != Some(id)
            && normalize_tenant_id(&p.inner_content.tenant_id) == policy.tenant_id
            && p.inner_content.role.eq_ignore_ascii_case(&policy.role)
    }) {
        return Err(PermissionError::Conflict(format!(
            "PermissionResource already exists for tenant '{}' and role '{}' (id: {:?})",
            policy.tenant_id, policy.role, conflict.id
        )));
    }

    let updated = RolePermissionResource {
        id: Some(id),
        resource_name: ResourceName::PermissionResource,
        inner_content: policy,
        external_content,
    };
    s_data
        .db
        .update_resource(
            ResourceName::PermissionResource,
            id,
            updated.to_database_resource(),
        )
        .await
        .map_err(|e| {
            PermissionError::Internal(format!("Failed to update PermissionResource {id}: {e:?}"))
        })?;

    {
        let mut cache = s_data.cache.write().await;
        cache.remove_role_policy_by_id(id);
        cache.upsert_role_policy(updated.clone());
    }

    Ok((before, updated))
}


pub fn extract_branch_id_from_req(req: &HttpRequest) -> Option<i64> {
    req.headers()
        .get("X-Branch-Id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<i64>().ok())
}

fn resolve_tenant_from_cache(cache: &Cache) -> Option<String> {
    let settings = cache.read_all(ResourceName::SettingResource).ok()?;
    for setting in settings {
        if let Some(ref inner) = setting.inner_content {
            if let Some(raw_branch) = crate::access::extract_sangh_branch(inner) {
                let slug = normalize_tenant_id(&raw_branch);
                if !slug.is_empty() {
                    return Some(slug);
                }
            }
        }
    }
    None
}

/// Resolves the tenant slug for a request (`X-Tenant-Id` -> `Claims.tenant_id` -> `SettingResource` -> `"default"`).
pub async fn resolve_request_tenant_id(req: Option<&HttpRequest>, s_data: &ServerData) -> String {
    if let Some(r) = req {
        if let Some(raw_hdr) = r.headers().get("X-Tenant-Id").and_then(|v| v.to_str().ok()) {
            let slug = normalize_tenant_id(raw_hdr);
            if !slug.is_empty() {
                return slug;
            }
        }
        let claim_tenant = {
            let ext = r.extensions();
            ext.get::<crate::auth::Claims>()
                .and_then(|c| c.tenant_id.as_deref())
                .map(normalize_tenant_id)
                .filter(|s| !s.is_empty())
        };
        if let Some(t) = claim_tenant {
            return t;
        }
    }

    if let Some(t) = {
        let cache = s_data.cache.read().await;
        resolve_tenant_from_cache(&cache)
    } {
        return t;
    }

    "default".to_string()
}

#[derive(Debug, Clone)]
struct CallerSnapshot {
    user_id: Option<i32>,
    role_name: String,
    tenant_id: Option<String>,
}

fn extract_caller_snapshot(req: &HttpRequest) -> Option<CallerSnapshot> {
    let ext = req.extensions();
    ext.get::<crate::auth::Claims>().map(|c| CallerSnapshot {
        user_id: c.user_id,
        role_name: c.effective_role_name(),
        tenant_id: c.tenant_id.clone(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionCheckOutcome {
    /// No v1.0 role policies are configured for the tenant yet; callers fall back to legacy RBAC.
    FallbackLegacy,
    Allowed {
        audit_required: bool,
        tenant_id: String,
        role: String,
        super_admin: bool,
    },
    Denied {
        message: String,
        tenant_id: String,
        role: String,
    },
}

fn emit_access_denied_audit(
    req: &HttpRequest,
    tenant_id: &str,
    role: &str,
    resource: &str,
    action_name: &str,
    branch: Option<i64>,
    reason: &str,
) {
    audit::record_http_audit_async(HttpAuditEvent {
        req: Some(req),
        category: AuditCategory::AccessControl,
        action: AuditAction::AccessDenied,
        status_code: StatusCode::FORBIDDEN.as_u16(),
        resource_name: Some(resource.to_string()),
        summary: reason.to_string(),
        branch_override: Some(tenant_id.to_string()),
        actor_role_override: Some(role.to_string()),
        metadata: Some(serde_json::json!({
            "tenantId": tenant_id,
            "role": role,
            "requestedAction": action_name,
            "branchId": branch,
            "reason": reason,
        })),
        ..Default::default()
    });
}

/// Central v1.0 role-permission enforcement helper used across generic and domain handlers.
/// Emits an `ACCESS_DENIED` audit entry whenever access is denied.
pub async fn enforce_request_permission(
    req: &HttpRequest,
    s_data: &ServerData,
    resource: &str,
    action_name: &str,
    branch: Option<i64>,
) -> PermissionCheckOutcome {
    let caller = extract_caller_snapshot(req);
    let canon_res = resolve_catalog_resource_name(resource).unwrap_or(resource.trim());
    let norm_act = normalize_action_token(action_name);

    let tenant_id = if let Some(raw_hdr) = req
        .headers()
        .get("X-Tenant-Id")
        .and_then(|v| v.to_str().ok())
    {
        let slug = normalize_tenant_id(raw_hdr);
        if slug.is_empty() {
            resolve_request_tenant_id(Some(req), s_data).await
        } else {
            slug
        }
    } else if let Some(ref snap) = caller {
        if let Some(ref t) = snap.tenant_id {
            let slug = normalize_tenant_id(t);
            if !slug.is_empty() {
                slug
            } else {
                resolve_request_tenant_id(Some(req), s_data).await
            }
        } else {
            resolve_request_tenant_id(Some(req), s_data).await
        }
    } else {
        resolve_request_tenant_id(Some(req), s_data).await
    };

    // Platform SuperAdmin works across tenants in code, still respecting system immutable logs/audit
    if let Some(ref snap) = caller {
        if is_super_admin_role(&snap.role_name) {
            if is_system_immutable_blocked(canon_res, &norm_act) {
                let msg = format!(
                    "System resource '{}' forbids '{}' even for Platform SuperAdmin",
                    canon_res, norm_act
                );
                emit_access_denied_audit(
                    req,
                    &tenant_id,
                    &snap.role_name,
                    canon_res,
                    &norm_act,
                    branch,
                    &msg,
                );
                return PermissionCheckOutcome::Denied {
                    message: msg,
                    tenant_id,
                    role: snap.role_name.clone(),
                };
            }
            return PermissionCheckOutcome::Allowed {
                audit_required: true,
                tenant_id,
                role: snap.role_name.clone(),
                super_admin: true,
            };
        }
    }

    let (has_policies, policy_opt) = {
        let cache = s_data.cache.read().await;
        let has = cache.has_role_policies_for_tenant(&tenant_id);
        let pol = caller
            .as_ref()
            .and_then(|c| cache.get_role_policy(&tenant_id, &c.role_name).cloned());
        (has, pol)
    };

    if !has_policies {
        return PermissionCheckOutcome::FallbackLegacy;
    }

    let Some(snap) = caller else {
        let msg = format!(
            "Missing authenticated claims for {}:{} on tenant '{}'",
            canon_res, norm_act, tenant_id
        );
        emit_access_denied_audit(
            req,
            &tenant_id,
            "Anonymous",
            canon_res,
            &norm_act,
            branch,
            &msg,
        );
        return PermissionCheckOutcome::Denied {
            message: msg,
            tenant_id,
            role: "Anonymous".to_string(),
        };
    };

    let Some(policy) = policy_opt else {
        let msg = format!(
            "No PermissionResource configured for role '{}' in tenant '{}' (denied by default)",
            snap.role_name, tenant_id
        );
        emit_access_denied_audit(
            req,
            &tenant_id,
            &snap.role_name,
            canon_res,
            &norm_act,
            branch,
            &msg,
        );
        return PermissionCheckOutcome::Denied {
            message: msg,
            tenant_id,
            role: snap.role_name,
        };
    };

    if !is_allowed(&policy.inner_content, &tenant_id, canon_res, &norm_act, branch) {
        let msg = format!(
            "Role '{}' is not allowed to perform '{}' on '{}' (tenant '{}', branch {:?})",
            snap.role_name, norm_act, canon_res, tenant_id, branch
        );
        emit_access_denied_audit(
            req,
            &tenant_id,
            &snap.role_name,
            canon_res,
            &norm_act,
            branch,
            &msg,
        );
        return PermissionCheckOutcome::Denied {
            message: msg,
            tenant_id,
            role: snap.role_name,
        };
    }

    let audit_required = requires_audit(&policy.inner_content, canon_res, &norm_act);
    PermissionCheckOutcome::Allowed {
        audit_required,
        tenant_id,
        role: snap.role_name,
        super_admin: false,
    }
}

fn caller_is_legacy_admin(req: &HttpRequest) -> bool {
    req.extensions()
        .get::<crate::auth::Claims>()
        .is_some_and(|c| c.role == crate::roles::Roles::Admin)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListPoliciesQuery {
    #[serde(alias = "tenant_id")]
    pub tenant_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckPermissionRequest {
    #[serde(default, alias = "tenant_id")]
    pub tenant_id: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    pub resource: String,
    pub action: String,
    #[serde(default, alias = "branch_id")]
    pub branch_id: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeedDefaultsRequest {
    #[serde(default, alias = "tenant_id")]
    pub tenant_id: Option<String>,
    #[serde(default, alias = "cashier_branch_ids")]
    pub cashier_branch_ids: Option<Vec<i64>>,
}

fn extract_wrapped_or_flat_policy(
    raw: Value,
) -> Result<(RolePermissionPolicy, Option<Value>), String> {
    let is_wrapped = raw
        .get("inner_content")
        .is_some_and(|v| v.is_object());
    if is_wrapped {
        let external = raw.get("external_content").cloned();
        let inner = raw
            .get("inner_content")
            .cloned()
            .ok_or_else(|| "inner_content is required".to_string())?;
        let policy: RolePermissionPolicy = serde_json::from_value(inner)
            .map_err(|e| format!("Invalid PermissionResource inner_content: {e}"))?;
        Ok((policy, external))
    } else {
        let policy: RolePermissionPolicy = serde_json::from_value(raw)
            .map_err(|e| format!("Invalid RolePermissionPolicy payload: {e}"))?;
        Ok((policy, Some(serde_json::json!({}))))
    }
}

/// `GET /api/v1/permissions/catalog` — returns the canonical in-code resource/action reference.
pub async fn get_action_catalog() -> HttpResponse {
    HttpResponse::Ok().json(serde_json::json!({
        "schemaVersion": PERMISSION_SCHEMA_VERSION,
        "resources": ACTION_CATALOG,
    }))
}

/// `GET /api/v1/permissions/me` — returns the caller's active role permission policy (or SuperAdmin status).
pub async fn get_my_role_permission(
    req: HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let Some(caller) = extract_caller_snapshot(&req) else {
        return HttpResponse::Unauthorized().json(serde_json::json!({
            "success": false,
            "message": "Missing authenticated claims"
        }));
    };

    let tenant_id = resolve_request_tenant_id(Some(&req), &s_data).await;
    if is_super_admin_role(&caller.role_name) {
        return HttpResponse::Ok().json(serde_json::json!({
            "superAdmin": true,
            "tenantId": tenant_id,
            "role": PLATFORM_SUPER_ADMIN_ROLE,
            "message": "Platform SuperAdmin is checked in code across tenants"
        }));
    }

    let cached = {
        let cache = s_data.cache.read().await;
        cache
            .get_role_policy(&tenant_id, &caller.role_name)
            .cloned()
    };
    if let Some(policy) = cached {
        return HttpResponse::Ok().json(policy);
    }

    match find_role_policy_in_db(&s_data.db, &tenant_id, &caller.role_name).await {
        Ok(Some(policy)) => {
            s_data.cache.write().await.upsert_role_policy(policy.clone());
            HttpResponse::Ok().json(policy)
        }
        Ok(None) => HttpResponse::NotFound().json(serde_json::json!({
            "success": false,
            "message": format!(
                "No v1.0 PermissionResource configured for tenant '{}' role '{}'",
                tenant_id, caller.role_name
            )
        })),
        Err(msg) => HttpResponse::InternalServerError().json(serde_json::json!({
            "success": false,
            "message": msg
        })),
    }
}

/// `GET /api/v1/permissions` — lists all configured v1.0 role policies (optionally filtered by `?tenantId=`).
pub async fn list_role_policies(
    req: HttpRequest,
    query: web::Query<ListPoliciesQuery>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    match enforce_request_permission(
        &req,
        &s_data,
        "PermissionResource",
        action::READ,
        extract_branch_id_from_req(&req),
    )
    .await
    {
        PermissionCheckOutcome::Denied { message, .. } => {
            return HttpResponse::Forbidden().json(serde_json::json!({
                "success": false,
                "message": message
            }));
        }
        PermissionCheckOutcome::FallbackLegacy if !caller_is_legacy_admin(&req) => {
            return HttpResponse::Forbidden().json(serde_json::json!({
                "success": false,
                "message": "Admin access required"
            }));
        }
        _ => {}
    }

    let tenant_filter = query
        .into_inner()
        .tenant_id
        .map(|s| normalize_tenant_id(&s))
        .filter(|s| !s.is_empty());

    let mut policies = find_all_role_policies_in_db(&s_data.db)
        .await
        .unwrap_or_else(|_| Vec::new());
    if policies.is_empty() {
        let cache = s_data.cache.read().await;
        policies = cache.list_role_policies(tenant_filter.as_deref());
    } else if let Some(ref tf) = tenant_filter {
        policies.retain(|p| normalize_tenant_id(&p.inner_content.tenant_id) == *tf);
    }

    HttpResponse::Ok().json(serde_json::json!({
        "total": policies.len(),
        "items": policies
    }))
}

/// `GET /api/v1/permissions/{tenant_id}/{role}` — reads the role policy for `(tenantId, role)`.
pub async fn get_role_policy_by_tenant_role(
    path: web::Path<(String, String)>,
    req: HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    match enforce_request_permission(
        &req,
        &s_data,
        "PermissionResource",
        action::READ,
        extract_branch_id_from_req(&req),
    )
    .await
    {
        PermissionCheckOutcome::Denied { message, .. } => {
            return HttpResponse::Forbidden().json(serde_json::json!({
                "success": false,
                "message": message
            }));
        }
        PermissionCheckOutcome::FallbackLegacy if !caller_is_legacy_admin(&req) => {
            return HttpResponse::Forbidden().json(serde_json::json!({
                "success": false,
                "message": "Admin access required"
            }));
        }
        _ => {}
    }

    let (raw_tenant, role) = path.into_inner();
    let tenant = normalize_tenant_id(&raw_tenant);
    if is_super_admin_role(&role) {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "success": false,
            "message": "Platform SuperAdmin has no PermissionResource document"
        }));
    }

    if let Some(cached) = s_data
        .cache
        .read()
        .await
        .get_role_policy(&tenant, &role)
        .cloned()
    {
        return HttpResponse::Ok().json(cached);
    }

    match find_role_policy_in_db(&s_data.db, &tenant, &role).await {
        Ok(Some(found)) => {
            s_data.cache.write().await.upsert_role_policy(found.clone());
            HttpResponse::Ok().json(found)
        }
        Ok(None) => HttpResponse::NotFound().json(serde_json::json!({
            "success": false,
            "message": format!("PermissionResource not found for tenant '{}' role '{}'", tenant, role)
        })),
        Err(msg) => HttpResponse::InternalServerError().json(serde_json::json!({
            "success": false,
            "message": msg
        })),
    }
}

/// `POST /api/v1/permissions` — creates a new `(tenantId, role)` `PermissionResource`.
pub async fn create_role_permission(
    payload: web::Json<Value>,
    req: HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let started_at = std::time::Instant::now();
    let audit_required = match enforce_request_permission(
        &req,
        &s_data,
        "PermissionResource",
        action::CREATE,
        extract_branch_id_from_req(&req),
    )
    .await
    {
        PermissionCheckOutcome::Denied { message, .. } => {
            return HttpResponse::Forbidden().json(serde_json::json!({
                "success": false,
                "message": message
            }));
        }
        PermissionCheckOutcome::FallbackLegacy => {
            if !caller_is_legacy_admin(&req) {
                return HttpResponse::Forbidden().json(serde_json::json!({
                    "success": false,
                    "message": "Admin access required"
                }));
            }
            true
        }
        PermissionCheckOutcome::Allowed { audit_required, .. } => audit_required,
    };

    let (mut policy, external) = match extract_wrapped_or_flat_policy(payload.into_inner()) {
        Ok(parsed) => parsed,
        Err(msg) => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "success": false,
                "message": msg
            }));
        }
    };

    let caller_id = extract_caller_snapshot(&req).and_then(|c| c.user_id);
    if policy.created_by.is_none() {
        policy.created_by = caller_id;
    }
    if policy.updated_by.is_none() {
        policy.updated_by = caller_id;
    }

    match create_role_policy_in_db(&s_data, policy, external).await {
        Ok(created) => {
            audit::record_http_audit_async(HttpAuditEvent {
                req: Some(&req),
                category: AuditCategory::AccessControl,
                action: AuditAction::PermissionGrant,
                status_code: StatusCode::CREATED.as_u16(),
                started_at: Some(started_at),
                resource_name: Some("PermissionResource".to_string()),
                resource_id: created.id.map(|i| i.to_string()),
                summary: format!(
                    "Created role permission '{}' for tenant '{}'",
                    created.inner_content.role, created.inner_content.tenant_id
                ),
                after_state: serde_json::to_value(&created.inner_content).ok(),
                branch_override: Some(created.inner_content.tenant_id.clone()),
                metadata: Some(serde_json::json!({
                    "tenantId": created.inner_content.tenant_id,
                    "role": created.inner_content.role,
                    "auditRequired": audit_required,
                })),
                ..Default::default()
            });
            HttpResponse::Created().json(created)
        }
        Err(err) => err.to_http_response(),
    }
}

/// `PUT /api/v1/permissions/{id}` — updates an existing `(tenantId, role)` `PermissionResource`.
pub async fn update_role_permission_by_id(
    path: web::Path<i32>,
    payload: web::Json<Value>,
    req: HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let started_at = std::time::Instant::now();
    let id = path.into_inner();
    let audit_required = match enforce_request_permission(
        &req,
        &s_data,
        "PermissionResource",
        action::UPDATE,
        extract_branch_id_from_req(&req),
    )
    .await
    {
        PermissionCheckOutcome::Denied { message, .. } => {
            return HttpResponse::Forbidden().json(serde_json::json!({
                "success": false,
                "message": message
            }));
        }
        PermissionCheckOutcome::FallbackLegacy => {
            if !caller_is_legacy_admin(&req) {
                return HttpResponse::Forbidden().json(serde_json::json!({
                    "success": false,
                    "message": "Admin access required"
                }));
            }
            true
        }
        PermissionCheckOutcome::Allowed { audit_required, .. } => audit_required,
    };

    let (mut policy, external) = match extract_wrapped_or_flat_policy(payload.into_inner()) {
        Ok(parsed) => parsed,
        Err(msg) => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "success": false,
                "message": msg
            }));
        }
    };

    let caller_id = extract_caller_snapshot(&req).and_then(|c| c.user_id);
    if policy.updated_by.is_none() {
        policy.updated_by = caller_id;
    }

    match update_role_policy_in_db(&s_data, id, policy, external).await {
        Ok((before, updated)) => {
            audit::record_http_audit_async(HttpAuditEvent {
                req: Some(&req),
                category: AuditCategory::AccessControl,
                action: AuditAction::PermissionUpdate,
                status_code: StatusCode::OK.as_u16(),
                started_at: Some(started_at),
                resource_name: Some("PermissionResource".to_string()),
                resource_id: Some(id.to_string()),
                summary: format!(
                    "Updated role permission '{}' ({}) for tenant '{}'",
                    updated.inner_content.role, id, updated.inner_content.tenant_id
                ),
                before_state: before.and_then(|b| serde_json::to_value(b).ok()),
                after_state: serde_json::to_value(&updated.inner_content).ok(),
                branch_override: Some(updated.inner_content.tenant_id.clone()),
                metadata: Some(serde_json::json!({
                    "tenantId": updated.inner_content.tenant_id,
                    "role": updated.inner_content.role,
                    "auditRequired": audit_required,
                })),
                ..Default::default()
            });
            HttpResponse::Ok().json(updated)
        }
        Err(err) => err.to_http_response(),
    }
}

/// `POST /api/v1/permissions/check` — evaluates whether a role/caller can perform `(resource, action, branchId)`.
pub async fn check_permission_endpoint(
    payload: web::Json<CheckPermissionRequest>,
    req: HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let body = payload.into_inner();
    let caller = extract_caller_snapshot(&req);
    let tenant_id = body
        .tenant_id
        .as_deref()
        .map(normalize_tenant_id)
        .filter(|s| !s.is_empty());
    let tenant_id = match tenant_id {
        Some(t) => t,
        None => resolve_request_tenant_id(Some(&req), &s_data).await,
    };

    let role = body
        .role
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .or_else(|| caller.as_ref().map(|c| c.role_name.clone()))
        .unwrap_or_default();

    let Some(canon_res) = resolve_catalog_resource_name(&body.resource) else {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "success": false,
            "message": format!("Unknown resource '{}'", body.resource)
        }));
    };
    let act = normalize_action_token(&body.action);
    if !is_valid_action_for_resource(canon_res, &act) {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "success": false,
            "message": format!("Unknown action '{}' for '{}'", body.action, canon_res)
        }));
    }

    if is_super_admin_role(&role) {
        let blocked = is_system_immutable_blocked(canon_res, &act);
        return HttpResponse::Ok().json(serde_json::json!({
            "allowed": !blocked,
            "auditRequired": true,
            "superAdmin": true,
            "tenantId": tenant_id,
            "role": PLATFORM_SUPER_ADMIN_ROLE,
            "resource": canon_res,
            "action": act,
            "branchId": body.branch_id,
            "reason": if blocked { "System audit/log write forbidden" } else { "Platform SuperAdmin" }
        }));
    }

    let policy_opt = {
        let cache = s_data.cache.read().await;
        cache.get_role_policy(&tenant_id, &role).cloned()
    };
    let Some(policy) = policy_opt else {
        return HttpResponse::Ok().json(serde_json::json!({
            "allowed": false,
            "auditRequired": false,
            "superAdmin": false,
            "tenantId": tenant_id,
            "role": role,
            "resource": canon_res,
            "action": act,
            "branchId": body.branch_id,
            "reason": "No PermissionResource configured for role in tenant (deny by default)"
        }));
    };

    let allowed = is_allowed(&policy.inner_content, &tenant_id, canon_res, &act, body.branch_id);
    let audit_required = requires_audit(&policy.inner_content, canon_res, &act);
    HttpResponse::Ok().json(serde_json::json!({
        "allowed": allowed,
        "auditRequired": audit_required,
        "superAdmin": false,
        "tenantId": tenant_id,
        "role": role,
        "resource": canon_res,
        "action": act,
        "branchId": body.branch_id,
        "reason": if allowed { "Allowed by role policy" } else { "Denied by role policy" }
    }))
}

/// `POST /api/v1/permissions/seed-defaults` — seeds explicit Admin, Cashier, and Viewer role policies for a tenant.
pub async fn seed_default_role_policies(
    payload: web::Json<SeedDefaultsRequest>,
    req: HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let started_at = std::time::Instant::now();
    match enforce_request_permission(
        &req,
        &s_data,
        "PermissionResource",
        action::REPAIR_ACCESS_CONTROL,
        extract_branch_id_from_req(&req),
    )
    .await
    {
        PermissionCheckOutcome::Denied { message, .. } => {
            return HttpResponse::Forbidden().json(serde_json::json!({
                "success": false,
                "message": message
            }));
        }
        PermissionCheckOutcome::FallbackLegacy if !caller_is_legacy_admin(&req) => {
            return HttpResponse::Forbidden().json(serde_json::json!({
                "success": false,
                "message": "Admin access required"
            }));
        }
        _ => {}
    }

    let body = payload.into_inner();
    let tenant_id = match body
        .tenant_id
        .as_deref()
        .map(normalize_tenant_id)
        .filter(|s| !s.is_empty())
    {
        Some(t) => t,
        None => resolve_request_tenant_id(Some(&req), &s_data).await,
    };

    let actor_id = extract_caller_snapshot(&req).and_then(|c| c.user_id);
    let cashier_branches = body.cashier_branch_ids.unwrap_or_else(|| vec![1, 2]);
    let defaults = vec![
        default_admin_policy(&tenant_id, actor_id),
        default_cashier_policy(&tenant_id, cashier_branches, actor_id),
        default_viewer_policy(&tenant_id, actor_id),
    ];

    let mut created = Vec::new();
    let mut skipped = Vec::new();

    for pol in defaults {
        let role_name = pol.role.clone();
        match create_role_policy_in_db(&s_data, pol, Some(serde_json::json!({}))).await {
            Ok(res) => created.push(res),
            Err(PermissionError::Conflict(_)) => skipped.push(role_name),
            Err(err) => return err.to_http_response(),
        }
    }

    audit::record_http_audit_async(HttpAuditEvent {
        req: Some(&req),
        category: AuditCategory::AccessControl,
        action: AuditAction::PermissionGrant,
        status_code: StatusCode::OK.as_u16(),
        started_at: Some(started_at),
        resource_name: Some("PermissionResource".to_string()),
        summary: format!(
            "Seeded default role policies for tenant '{}': {} created, {} skipped",
            tenant_id,
            created.len(),
            skipped.len()
        ),
        branch_override: Some(tenant_id.clone()),
        metadata: Some(serde_json::json!({
            "tenantId": tenant_id,
            "createdRoles": created.iter().map(|c| &c.inner_content.role).collect::<Vec<_>>(),
            "skippedRoles": skipped,
            "auditRequired": true,
        })),
        ..Default::default()
    });

    HttpResponse::Ok().json(serde_json::json!({
        "tenantId": tenant_id,
        "createdCount": created.len(),
        "skippedRoles": skipped,
        "created": created,
    }))
}

/// `POST /api/v1/permissions/repair` — reloads & validates `PermissionResource` role policies into cache.
pub async fn repair_access_control(
    req: HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let started_at = std::time::Instant::now();
    match enforce_request_permission(
        &req,
        &s_data,
        "PermissionResource",
        action::REPAIR_ACCESS_CONTROL,
        extract_branch_id_from_req(&req),
    )
    .await
    {
        PermissionCheckOutcome::Denied { message, .. } => {
            return HttpResponse::Forbidden().json(serde_json::json!({
                "success": false,
                "message": message
            }));
        }
        PermissionCheckOutcome::FallbackLegacy if !caller_is_legacy_admin(&req) => {
            return HttpResponse::Forbidden().json(serde_json::json!({
                "success": false,
                "message": "Admin access required"
            }));
        }
        _ => {}
    }

    let rows = match s_data
        .db
        .read_all_resource(ResourceName::PermissionResource)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "success": false,
                "message": format!("Failed to read PermissionResource table: {e:?}")
            }));
        }
    };

    let mut reloaded = 0usize;
    let mut invalid = Vec::new();
    let mut cache = s_data.cache.write().await;
    cache.role_policies.clear();

    for row in rows {
        let Some(ref inner) = row.inner_content else {
            continue;
        };
        if !is_role_permission_payload(inner) {
            continue;
        }
        match RolePermissionResource::from_database_resource(&row) {
            Ok(mut parsed) => {
                if let Err(errs) = normalize_and_validate_policy(&mut parsed.inner_content) {
                    invalid.push(serde_json::json!({
                        "id": row.id,
                        "errors": errs
                    }));
                } else {
                    cache.upsert_role_policy(parsed);
                    reloaded += 1;
                }
            }
            Err(err) => {
                invalid.push(serde_json::json!({
                    "id": row.id,
                    "errors": [err]
                }));
            }
        }
    }
    drop(cache);

    audit::record_http_audit_async(HttpAuditEvent {
        req: Some(&req),
        category: AuditCategory::AccessControl,
        action: AuditAction::PermissionUpdate,
        status_code: StatusCode::OK.as_u16(),
        started_at: Some(started_at),
        resource_name: Some("PermissionResource".to_string()),
        summary: format!(
            "Repaired access control cache: {} role policies loaded, {} invalid",
            reloaded,
            invalid.len()
        ),
        metadata: Some(serde_json::json!({
            "action": action::REPAIR_ACCESS_CONTROL,
            "reloadedCount": reloaded,
            "invalidCount": invalid.len(),
            "auditRequired": true,
        })),
        ..Default::default()
    });

    HttpResponse::Ok().json(serde_json::json!({
        "success": true,
        "reloadedCount": reloaded,
        "invalid": invalid
    }))
}

pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.route("/catalog", web::get().to(get_action_catalog))
        .route("/me", web::get().to(get_my_role_permission))
        .route("/check", web::post().to(check_permission_endpoint))
        .route("/seed-defaults", web::post().to(seed_default_role_policies))
        .route("/repair", web::post().to(repair_access_control))
        .route("", web::get().to(list_role_policies))
        .route("", web::post().to(create_role_permission))
        .route("/{id}", web::put().to(update_role_permission_by_id))
        .route(
            "/{tenant_id}/{role}",
            web::get().to(get_role_policy_by_tenant_role),
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_catalog_covers_all_15_resources_and_custom_domain_actions() {
        let catalog = action_catalog();
        assert_eq!(catalog.len(), 15);
        assert!(is_valid_action_for_resource(
            "VehicleResource",
            "CONFIRM_NUMBER_CHANGE"
        ));
        assert!(is_valid_action_for_resource(
            "PermissionResource",
            "REPAIR_ACCESS_CONTROL"
        ));
        assert!(is_valid_action_for_resource("BillResource", "CANCEL"));
        assert!(is_valid_action_for_resource("ReportResource", "EXPORT"));
        assert!(!is_valid_action_for_resource("ReportResource", "DELETE"));
        assert!(is_system_immutable_blocked("AuditTrailResource", "DELETE"));
        assert!(is_system_immutable_blocked("ActionLogsResource", "CREATE"));
        assert!(!is_system_immutable_blocked("AuditTrailResource", "READ"));
    }

    #[test]
    fn test_full_admin_json_parses_validates_and_evaluates_correctly() {
        let raw = serde_json::json!({
          "resource_name": "PermissionResource",
          "inner_content": {
            "schemaVersion": "1.0",
            "tenantId": "loadboard-sthaniya",
            "role": "Admin",
            "status": "ACTIVE",
            "branchIds": [],
            "allow": {
              "BillResource": [
                "CREATE", "READ", "UPDATE", "CANCEL", "CORRECT",
                "PRINT", "REPRINT", "REPAIR_INVOICE", "VOID"
              ],
              "MemberResource": [
                "CREATE", "READ", "UPDATE", "DELETE",
                "CHANGE_OWNER", "RECONCILE", "BLACKLIST", "UNBLACKLIST"
              ],
              "VehicleResource": [
                "CREATE", "READ", "UPDATE", "DELETE",
                "CONFIRM_NUMBER_CHANGE", "DISPLACE", "BLACKLIST", "UNBLACKLIST"
              ],
              "VehicleCategoryResource": ["CREATE", "READ", "UPDATE", "DELETE"],
              "BillingRuleResource": ["CREATE", "READ", "UPDATE", "DELETE", "ACTIVATE", "DEACTIVATE"],
              "BillingEntityResource": ["CREATE", "READ", "UPDATE", "DELETE"],
              "BranchResource": ["CREATE", "READ", "UPDATE", "DELETE"],
              "ReportResource": ["READ", "EXPORT"],
              "SettingResource": ["READ", "UPDATE", "MANAGE_FISCAL_YEAR", "MANAGE_SMS"],
              "UserResource": [
                "CREATE", "READ", "UPDATE", "DELETE",
                "RESET_PASSWORD", "ACTIVATE", "DEACTIVATE"
              ],
              "PermissionResource": [
                "CREATE", "READ", "UPDATE", "ASSIGN", "REPAIR_ACCESS_CONTROL"
              ],
              "CacheResource": ["READ", "CLEAR"],
              "ActionLogsResource": ["READ", "EXPORT"],
              "AuditTrailResource": ["READ", "EXPORT"]
            },
            "deny": {
              "BillResource": ["DELETE"],
              "PermissionResource": ["DELETE"],
              "ActionLogsResource": ["CREATE", "UPDATE", "DELETE"],
              "AuditTrailResource": ["CREATE", "UPDATE", "DELETE"]
            },
            "requireAudit": [
              "BillResource:CANCEL",
              "BillResource:CORRECT",
              "BillResource:REPAIR_INVOICE",
              "BillResource:VOID",
              "VehicleResource:CONFIRM_NUMBER_CHANGE",
              "MemberResource:CHANGE_OWNER",
              "PermissionResource:ASSIGN",
              "PermissionResource:REPAIR_ACCESS_CONTROL",
              "UserResource:RESET_PASSWORD"
            ],
            "createdBy": 3,
            "updatedBy": 3
          },
          "external_content": {}
        });

        let (mut policy, external) = extract_wrapped_or_flat_policy(raw).unwrap();
        assert_eq!(external, Some(serde_json::json!({})));
        assert!(normalize_and_validate_policy(&mut policy).is_ok());

        // Slug normalization matches "Loadboard sthaniya" -> "loadboard-sthaniya"
        assert!(is_allowed(
            &policy,
            "Loadboard sthaniya",
            "VehicleResource",
            "CONFIRM_NUMBER_CHANGE",
            Some(9)
        ));
        assert!(is_allowed(
            &policy,
            "loadboard-sthaniya",
            "PermissionResource",
            "REPAIR_ACCESS_CONTROL",
            None
        ));
        assert!(!is_allowed(
            &policy,
            "loadboard-sthaniya",
            "BillResource",
            "DELETE",
            Some(1)
        ));
        assert!(!is_allowed(
            &policy,
            "loadboard-sthaniya",
            "AuditTrailResource",
            "CREATE",
            None
        ));
        assert!(!is_allowed(
            &policy,
            "other-tenant",
            "BillResource",
            "READ",
            None
        ));
        assert!(requires_audit(&policy, "VehicleResource", "CONFIRM_NUMBER_CHANGE"));
        assert!(!requires_audit(&policy, "BillResource", "READ"));
    }

    #[test]
    fn test_cashier_viewer_deny_wins_and_validation_rejections() {
        let cashier = default_cashier_policy("loadboard-sthaniya", vec![1, 2], Some(1));
        assert!(is_allowed(&cashier, "loadboard-sthaniya", "BillResource", "CREATE", Some(1)));
        assert!(is_allowed(&cashier, "loadboard-sthaniya", "BillResource", "PRINT", Some(2)));
        assert!(!is_allowed(&cashier, "loadboard-sthaniya", "BillResource", "CREATE", Some(3)));
        assert!(!is_allowed(&cashier, "loadboard-sthaniya", "BillResource", "CANCEL", Some(1)));
        assert!(!is_allowed(&cashier, "loadboard-sthaniya", "SettingResource", "UPDATE", None));

        let mut inactive_cashier = cashier.clone();
        inactive_cashier.status = "INACTIVE".to_string();
        assert!(!is_allowed(&inactive_cashier, "loadboard-sthaniya", "BillResource", "READ", Some(1)));

        let viewer = default_viewer_policy("loadboard-sthaniya", Some(1));
        assert!(is_allowed(&viewer, "loadboard-sthaniya", "BillResource", "READ", Some(5)));
        assert!(!is_allowed(&viewer, "loadboard-sthaniya", "BillResource", "CREATE", Some(5)));
        assert!(!is_allowed(&viewer, "loadboard-sthaniya", "UserResource", "READ", None));
        assert!(!is_allowed(&viewer, "loadboard-sthaniya", "PermissionResource", "READ", None));
        assert!(!is_allowed(&viewer, "loadboard-sthaniya", "SettingResource", "READ", None));

        // SuperAdmin must not be created in DB
        let mut super_admin = default_admin_policy("loadboard-sthaniya", None);
        super_admin.role = "SuperAdmin".to_string();
        assert!(normalize_and_validate_policy(&mut super_admin).is_err());

        // Granting CREATE/UPDATE/DELETE on AuditTrailResource or ActionLogsResource is rejected
        let mut bad_audit = default_viewer_policy("loadboard-sthaniya", None);
        bad_audit
            .allow
            .insert("AuditTrailResource".to_string(), svec(&["DELETE"]));
        assert!(normalize_and_validate_policy(&mut bad_audit).is_err());
    }

    #[actix_web::test]
    async fn test_permission_http_lifecycle_uniqueness_and_access_denied_audit() {
        let (s_data, _) = crate::access::test_helpers::create_mock_server_data_with_state(
            crate::access::test_helpers::MockDbState::default(),
        )
        .await;

        let admin_req = actix_web::test::TestRequest::default().to_http_request();
        admin_req.extensions_mut().insert(crate::auth::Claims {
            sub: "admin@narayani.com".to_string(),
            exp: 9999999999,
            iat: 1000000000,
            role: crate::roles::Roles::Admin,
            user_id: Some(3),
            jti: None,
            role_name: Some("Admin".to_string()),
            tenant_id: Some("loadboard-sthaniya".to_string()),
        });

        let cashier_email = format!("cashier-{}@narayani.com", audit::generate_audit_id());
        let cashier_req = actix_web::test::TestRequest::default().to_http_request();
        cashier_req.extensions_mut().insert(crate::auth::Claims {
            sub: cashier_email.clone(),
            exp: 9999999999,
            iat: 1000000000,
            role: crate::roles::Roles::User,
            user_id: Some(12),
            jti: None,
            role_name: Some("Cashier".to_string()),
            tenant_id: Some("loadboard-sthaniya".to_string()),
        });

        // 0. Catalog endpoint returns all 15 resources
        let cat_resp = get_action_catalog().await;
        assert_eq!(cat_resp.status(), StatusCode::OK);
        let cat_body: Value = serde_json::from_slice(
            &actix_web::body::to_bytes(cat_resp.into_body()).await.unwrap(),
        )
        .unwrap();
        assert_eq!(cat_body["schemaVersion"], PERMISSION_SCHEMA_VERSION);
        assert_eq!(cat_body["resources"].as_array().unwrap().len(), 15);

        // 1. Seed default Admin, Cashier ([1,2]), Viewer policies
        let seed_resp = seed_default_role_policies(
            web::Json(SeedDefaultsRequest {
                tenant_id: Some("Loadboard sthaniya".to_string()),
                cashier_branch_ids: Some(vec![1, 2]),
            }),
            admin_req.clone(),
            s_data.clone(),
        )
        .await;
        assert_eq!(seed_resp.status(), StatusCode::OK);

        // 1b. Query /me, list, get by tenant/role, and update by id
        let me_resp = get_my_role_permission(cashier_req.clone(), s_data.clone()).await;
        assert_eq!(me_resp.status(), StatusCode::OK);

        let list_resp = list_role_policies(
            admin_req.clone(),
            web::Query(ListPoliciesQuery {
                tenant_id: Some("Loadboard sthaniya".to_string()),
            }),
            s_data.clone(),
        )
        .await;
        assert_eq!(list_resp.status(), StatusCode::OK);
        let list_body: Value = serde_json::from_slice(
            &actix_web::body::to_bytes(list_resp.into_body()).await.unwrap(),
        )
        .unwrap();
        assert_eq!(list_body["total"], 3);

        let get_cashier = get_role_policy_by_tenant_role(
            web::Path::from(("loadboard-sthaniya".to_string(), "Cashier".to_string())),
            admin_req.clone(),
            s_data.clone(),
        )
        .await;
        assert_eq!(get_cashier.status(), StatusCode::OK);
        let cashier_res: RolePermissionResource = serde_json::from_slice(
            &actix_web::body::to_bytes(get_cashier.into_body())
                .await
                .unwrap(),
        )
        .unwrap();
        let cashier_id = cashier_res.id.unwrap();

        let upd_resp = update_role_permission_by_id(
            web::Path::from(cashier_id),
            web::Json(serde_json::json!({
                "schemaVersion": "1.0",
                "tenantId": "loadboard-sthaniya",
                "role": "Cashier",
                "status": "ACTIVE",
                "branchIds": [1, 2, 4],
                "allow": cashier_res.inner_content.allow,
                "deny": {}
            })),
            admin_req.clone(),
            s_data.clone(),
        )
        .await;
        assert_eq!(upd_resp.status(), StatusCode::OK);

        // 2. Duplicate creation for (loadboard-sthaniya, Cashier) must return 409 Conflict
        let dup_resp = create_role_permission(
            web::Json(serde_json::json!({
                "schemaVersion": "1.0",
                "tenantId": "loadboard-sthaniya",
                "role": "Cashier",
                "branchIds": [1],
                "allow": { "BillResource": ["READ"] }
            })),
            admin_req.clone(),
            s_data.clone(),
        )
        .await;
        assert_eq!(dup_resp.status(), StatusCode::CONFLICT);

        // 3. Cashier is denied PermissionResource:UPDATE and receives ACCESS_DENIED audit entry
        let denied_outcome = enforce_request_permission(
            &cashier_req,
            &s_data,
            "PermissionResource",
            "UPDATE",
            Some(1),
        )
        .await;
        assert!(matches!(denied_outcome, PermissionCheckOutcome::Denied { .. }));
        audit::flush_audit_trail().await;

        let denied_logs = audit::global_audit_logger()
            .query(&audit::AuditTrailQuery {
                actor: Some(cashier_email),
                action: Some("ACCESS_DENIED".to_string()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(denied_logs.total, 1);

        // 4. Dry-run check endpoint evaluates custom actions cleanly
        let chk_resp = check_permission_endpoint(
            web::Json(CheckPermissionRequest {
                tenant_id: Some("loadboard-sthaniya".to_string()),
                role: Some("Admin".to_string()),
                resource: "VehicleResource".to_string(),
                action: "CONFIRM_NUMBER_CHANGE".to_string(),
                branch_id: Some(1),
            }),
            admin_req.clone(),
            s_data.clone(),
        )
        .await;
        assert_eq!(chk_resp.status(), StatusCode::OK);
        let chk_body: Value = serde_json::from_slice(
            &actix_web::body::to_bytes(chk_resp.into_body()).await.unwrap(),
        )
        .unwrap();
        assert_eq!(chk_body["allowed"], true);
        assert_eq!(chk_body["auditRequired"], true);

        // 5. Repair endpoint re-hydrates role_policies from DB
        let repair_resp = repair_access_control(admin_req, s_data.clone()).await;
        assert_eq!(repair_resp.status(), StatusCode::OK);
        let repair_body: Value = serde_json::from_slice(
            &actix_web::body::to_bytes(repair_resp.into_body())
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(repair_body["reloadedCount"], 3);
    }
}
