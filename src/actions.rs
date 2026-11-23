use crate::resource::{DatabaseResource, ResourceName};
use crate::ServerData;
use actix_web::{web, HttpResponse, HttpMessage};

use std::sync::Arc;

pub async fn search(
    path: web::Path<(String, String, String, String)>,
    req: actix_web::HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let started_at = std::time::Instant::now();
    let (resource_name, search_item, search_query, limit) = path.into_inner();
    let is_admin_caller = is_admin(req.clone());
    let (response, resource_label, details, metadata) = search_inner(
        resource_name,
        search_item,
        search_query,
        limit,
        is_admin_caller,
        &s_data,
    )
    .await;

    crate::log::record_http_action_async(
        Some(&req),
        crate::log::ResourceActionKind::Search,
        Some(resource_label),
        None,
        response.status().as_u16(),
        started_at,
        details,
        metadata,
    );

    response
}

async fn search_inner(
    resource_name_raw: String,
    search_item: String,
    search_query: String,
    limit: String,
    is_admin_caller: bool,
    s_data: &Arc<ServerData>,
) -> (HttpResponse, String, Option<String>, Option<serde_json::Value>) {
    let resource_name: ResourceName = match ResourceName::from_str_case_insensitive(&resource_name_raw) {
        Some(r) => {
            if r == ResourceName::UserResource || r == ResourceName::PermissionResource {
                return (
                    HttpResponse::Forbidden().body(format!(
                        "Search forbidden in {:?} is forbidden",
                        r
                    )),
                    r.to_string(),
                    Some(format!("Search forbidden in {:?}", r)),
                    None,
                );
            } else if r == ResourceName::ActionLogsResource && !is_admin_caller {
                return (
                    HttpResponse::Forbidden().body("Access to ActionLogsResource is forbidden"),
                    r.to_string(),
                    Some("Non-admin search ActionLogsResource rejected".to_string()),
                    None,
                );
            } else {
                r
            }
        }
        None => {
            return (
                HttpResponse::BadRequest().body("Invalid resource name"),
                resource_name_raw,
                Some("Invalid resource name".to_string()),
                None,
            );
        }
    };
    let resource_label = resource_name.to_string();
    let ulimit: usize = if limit == "all" {
        usize::MAX
    } else {
        match limit.parse() {
            Ok(i) => i,
            Err(_) => {
                return (
                    HttpResponse::BadRequest().body("Invalid limit"),
                    resource_label,
                    Some("Invalid limit".to_string()),
                    None,
                );
            }
        }
    };

    let cache = s_data.cache.read().await;
    let db_resources =
        cache.search_innercontent(resource_name, &search_item, &search_query, ulimit);
    let result_count = db_resources.1;
    let metadata = Some(serde_json::json!({
        "searchItem": search_item,
        "searchQuery": search_query,
        "limit": limit,
        "resultCount": result_count,
    }));

    if result_count == 0 {
        (
            HttpResponse::NotFound().body(format!(
                "No resources found for search query: {}",
                search_query
            )),
            resource_label,
            Some(format!("No results for '{}'", search_query)),
            metadata,
        )
    } else {
        (
            HttpResponse::Ok().json(db_resources),
            resource_label,
            Some(format!("Matched {} resource(s)", result_count)),
            metadata,
        )
    }
}

pub fn is_admin(req: actix_web::HttpRequest) -> bool {
    let claims = req.extensions();
    let claims = claims.get::<crate::auth::Claims>();

    match claims {
        Some(claims) => {
            if claims.role == crate::roles::Roles::Admin {
                true
            } else {
                false
            }
        }
        None => false,
    }
}

pub async fn update(
    path: web::Path<(ResourceName, i32)>,
    db_resource: web::Json<DatabaseResource>,
    req: actix_web::HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let started_at = std::time::Instant::now();
    let (resource_name, id) = path.into_inner();
    let resource_label = resource_name.to_string();
    let resource_id = Some(id.to_string());
    let (response, details) =
        update_inner(resource_name, id, db_resource.into_inner(), req.clone(), s_data).await;

    crate::log::record_http_action_async(
        Some(&req),
        crate::log::ResourceActionKind::Update,
        Some(resource_label),
        resource_id,
        response.status().as_u16(),
        started_at,
        details,
        None,
    );

    response
}

async fn update_inner(
    resource_name: ResourceName,
    id: i32,
    db_resource: DatabaseResource,
    req: actix_web::HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> (HttpResponse, Option<String>) {
    if resource_name == ResourceName::PermissionResource {
        if !is_admin(req.clone()) {
            return (
                HttpResponse::Forbidden().body("Access to PermissionResource is forbidden"),
                Some("Non-admin update PermissionResource rejected".to_string()),
            );
        }

        let Some(inner_val) = db_resource.inner_content.clone() else {
            return (
                HttpResponse::BadRequest().body("PermissionResource inner_content is required"),
                Some("PermissionResource inner_content missing".to_string()),
            );
        };
        let mut perm_inner: crate::access::PermissionInnerContent =
            match serde_json::from_value(inner_val) {
                Ok(parsed) => parsed,
                Err(err) => {
                    let msg = format!("Invalid PermissionResource inner_content: {}", err);
                    return (HttpResponse::BadRequest().body(msg.clone()), Some(msg));
                }
            };
        perm_inner.sthaniya_branch =
            crate::access::normalize_optional_branch(perm_inner.sthaniya_branch);

        match crate::access::find_user_by_id_in_db(&s_data.db, perm_inner.user_id).await {
            Ok(Some(_)) => {}
            Ok(None) => {
                let msg = format!("UserResource with id {} not found", perm_inner.user_id);
                return (HttpResponse::NotFound().body(msg.clone()), Some(msg));
            }
            Err(err) => {
                let msg = format!("Failed to verify user for PermissionResource: {:?}", err);
                return (
                    HttpResponse::InternalServerError().body(msg.clone()),
                    Some(msg),
                );
            }
        }

        match crate::access::find_permission_by_user_id_in_db(&s_data.db, perm_inner.user_id).await
        {
            Ok(Some(existing)) if existing.id != Some(id) => {
                let msg = format!(
                    "PermissionResource already exists for userId {} (1:1 mapping enforced)",
                    perm_inner.user_id
                );
                return (HttpResponse::Conflict().body(msg.clone()), Some(msg));
            }
            Ok(_) => {}
            Err(err) => {
                return (
                    HttpResponse::InternalServerError().body(err.clone()),
                    Some(err),
                );
            }
        }

        let target_user_id = perm_inner.user_id;
        let stored_res =
            crate::access::PermissionResource::new(Some(id), perm_inner, db_resource.external_content)
                .to_database_resource();

        if let Err(err) = s_data
            .db
            .update_resource(resource_name, id, stored_res.clone())
            .await
        {
            let msg = format!("Failed to update resource: {:?}", err);
            return (
                HttpResponse::InternalServerError().body(msg.clone()),
                Some(msg),
            );
        }

        {
            let mut cache = s_data.cache.write().await;
            if crate::access::is_user_logged_in_cache(&cache, target_user_id) {
                crate::access::remove_user_permissions_from_cache(&mut cache, target_user_id);
                cache.write(resource_name, id, stored_res);
            } else if let Some(entries) = cache.cache.get_mut(&ResourceName::PermissionResource) {
                entries.remove(&id);
            }
        }

        return (
            HttpResponse::Ok().json(format!("Resource updated with id: {}", id)),
            Some(format!("Updated PermissionResource {}", id)),
        );
    }

    if resource_name == ResourceName::BillingRuleResource && !is_admin(req.clone()) {
        return (
            HttpResponse::Forbidden().body("Only Admin can modify BillingRuleResource"),
            Some("Non-admin update BillingRuleResource rejected".to_string()),
        );
    }

    if resource_name == ResourceName::ActionLogsResource && !is_admin(req.clone()) {
        return (
            HttpResponse::Forbidden().body("Access to ActionLogsResource is forbidden"),
            Some("Non-admin update ActionLogsResource rejected".to_string()),
        );
    }

    if resource_name == ResourceName::MemberResource
        && db_resource
            .inner_content
            .as_ref()
            .is_some_and(crate::dashboard_statistics::is_member_displaced)
    {
        let displaced_enabled = {
            let cache = s_data.cache.read().await;
            crate::settings::is_displaced_tagging_enabled_in_cache(&cache)
        };
        if !displaced_enabled {
            return (
                HttpResponse::Forbidden().body("Displaced tagging is disabled in SettingResource"),
                Some("Displaced MemberResource update rejected".to_string()),
            );
        }
    }

    if resource_name == ResourceName::UserResource {
        if !is_admin(req.clone()) {
            return (
                HttpResponse::Forbidden().body("Access to UserResource is forbidden"),
                Some("Non-admin update UserResource rejected".to_string()),
            );
        } else {
            //if the admin is trying to update own user resource, then deny it
            let extensions = req.extensions();
            let claims = extensions.get::<crate::auth::Claims>();
            //if sub == email of the user resource, then deny it check it in cache
            if let Some(claims) = claims {
                let cache = s_data.cache.read().await;
                if let Some(resource) = cache.read(resource_name.clone(), id) {
                    if let Some(inner_content) = &resource.inner_content {
                        if let Some(email) = inner_content.get("email") {
                            if email == &claims.sub {
                                return (
                                    HttpResponse::Forbidden().body("Admin cannot update oneself"),
                                    Some("Admin self-update rejected".to_string()),
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    if let Err(err) = s_data
        .db
        .update_resource(resource_name.clone(), id, db_resource.clone())
        .await
    {
        let msg = format!("Failed to update resource: {:?}", err);
        return (
            HttpResponse::InternalServerError().body(msg.clone()),
            Some(msg),
        );
    }

    let updated_resource = DatabaseResource {
        id: Some(id),
        resource_name: Some(resource_name),
        ..db_resource
    };

    {
        let mut cache = s_data.cache.write().await;

        cache.write(resource_name, id, updated_resource.clone());
    }

    if resource_name == ResourceName::UserResource {
        if let Some(ref inner_content) = updated_resource.inner_content {
            if let Err(err) =
                crate::access::sync_permission_on_user_update(&s_data, id, inner_content).await
            {
                eprintln!(
                    "Failed to sync PermissionResource on UserResource {} update: {}",
                    id, err
                );
            }
        }
    }

    (
        HttpResponse::Ok().json(format!("Resource updated with id: {}", id)),
        Some(format!("Updated {} with id {}", resource_name.to_string(), id)),
    )
}

pub async fn write(
    path: web::Path<ResourceName>,
    db_resource: web::Json<DatabaseResource>,
    req: actix_web::HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let started_at = std::time::Instant::now();
    let resource_name = path.into_inner();
    let resource_label = resource_name.to_string();
    let (response, created_id, details) =
        write_inner(resource_name, db_resource.into_inner(), req.clone(), s_data).await;

    crate::log::record_http_action_async(
        Some(&req),
        crate::log::ResourceActionKind::Create,
        Some(resource_label),
        created_id,
        response.status().as_u16(),
        started_at,
        details,
        None,
    );

    response
}

async fn write_inner(
    resource_name: ResourceName,
    mut db_resource: DatabaseResource,
    req: actix_web::HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> (HttpResponse, Option<String>, Option<String>) {
    if resource_name == ResourceName::PermissionResource {
        if !is_admin(req.clone()) {
            return (
                HttpResponse::Forbidden().body("Access to PermissionResource is forbidden"),
                None,
                Some("Non-admin create PermissionResource rejected".to_string()),
            );
        }

        let Some(inner_val) = db_resource.inner_content.clone() else {
            return (
                HttpResponse::BadRequest().body("PermissionResource inner_content is required"),
                None,
                Some("PermissionResource inner_content missing".to_string()),
            );
        };
        let mut perm_inner: crate::access::PermissionInnerContent =
            match serde_json::from_value(inner_val) {
                Ok(parsed) => parsed,
                Err(err) => {
                    let msg = format!("Invalid PermissionResource inner_content: {}", err);
                    return (
                        HttpResponse::BadRequest().body(msg.clone()),
                        None,
                        Some(msg),
                    );
                }
            };

        let resolved_branch = crate::access::resolve_branch_for_new_creation(
            &req,
            &s_data,
            perm_inner.sthaniya_branch.clone(),
        )
        .await;
        perm_inner.sthaniya_branch = resolved_branch;

        return match crate::access::create_permission_in_db_strict(
            &s_data.db,
            perm_inner,
            db_resource.external_content.clone(),
        )
        .await
        {
            Ok(created) => {
                let perm_id_str = created.id.map(|i| i.to_string());
                if let Some(perm_id) = created.id {
                    let mut cache = s_data.cache.write().await;
                    if crate::access::is_user_logged_in_cache(
                        &cache,
                        created.inner_content.user_id,
                    ) {
                        crate::access::remove_user_permissions_from_cache(
                            &mut cache,
                            created.inner_content.user_id,
                        );
                        cache.write(
                            ResourceName::PermissionResource,
                            perm_id,
                            created.to_database_resource(),
                        );
                    }
                }
                (
                    HttpResponse::Ok().json(created.to_database_resource()),
                    perm_id_str.clone(),
                    Some(format!("Created PermissionResource {:?}", perm_id_str)),
                )
            }
            Err(err) => {
                let resp = err.to_http_response();
                (resp, None, Some(format!("{err:?}")))
            }
        };
    }

    if resource_name == ResourceName::BillingRuleResource && !is_admin(req.clone()) {
        return (
            HttpResponse::Forbidden().body("Only Admin can modify BillingRuleResource"),
            None,
            Some("Non-admin create BillingRuleResource rejected".to_string()),
        );
    }

    if resource_name == ResourceName::ActionLogsResource && !is_admin(req.clone()) {
        return (
            HttpResponse::Forbidden().body("Access to ActionLogsResource is forbidden"),
            None,
            Some("Non-admin create ActionLogsResource rejected".to_string()),
        );
    }

    if resource_name == ResourceName::MemberResource
        && db_resource
            .inner_content
            .as_ref()
            .is_some_and(crate::dashboard_statistics::is_member_displaced)
    {
        let displaced_enabled = {
            let cache = s_data.cache.read().await;
            crate::settings::is_displaced_tagging_enabled_in_cache(&cache)
        };
        if !displaced_enabled {
            return (
                HttpResponse::Forbidden().body("Displaced tagging is disabled in SettingResource"),
                None,
                Some("Displaced MemberResource create rejected".to_string()),
            );
        }
    }

    let mut effective_user_branch: Option<String> = None;

    if resource_name == ResourceName::UserResource {
        if !is_admin(req.clone()) {
            return (
                HttpResponse::Forbidden().body("Access to UserResource is forbidden"),
                None,
                Some("Non-admin create UserResource rejected".to_string()),
            );
        }
        let payload_branch = db_resource
            .inner_content
            .as_ref()
            .and_then(crate::access::extract_sangh_branch);
        effective_user_branch =
            crate::access::resolve_branch_for_new_creation(&req, &s_data, payload_branch).await;

        if let Some(ref branch) = effective_user_branch {
            if let Some(serde_json::Value::Object(ref mut map)) = db_resource.inner_content {
                map.insert(
                    "SanghBranch".to_string(),
                    serde_json::Value::String(branch.clone()),
                );
            }
        }
    }

    let id = match s_data
        .db
        .create_resource(resource_name.clone(), db_resource.clone())
        .await
    {
        Ok(id) => id,
        Err(err) => {
            let msg = format!("Failed to write resource: {:?}", err);
            return (
                HttpResponse::InternalServerError().body(msg.clone()),
                None,
                Some(msg),
            );
        }
    };

    if resource_name == ResourceName::UserResource {
        if let Some(ref inner_content) = db_resource.inner_content {
            if let Err(err) = crate::access::ensure_permission_in_db_for_user(
                &s_data.db,
                id,
                inner_content,
                effective_user_branch,
            )
            .await
            {
                eprintln!(
                    "Failed to create paired PermissionResource for UserResource {}: {}",
                    id, err
                );
            }
        }
    }

    {
        let mut cache = s_data.cache.write().await;

        cache.write(
            resource_name,
            id,
            DatabaseResource {
                id: Some(id),
                resource_name: Some(resource_name.into()),
                ..db_resource.clone()
            },
        );
    }

    (
        HttpResponse::Ok().json(DatabaseResource {
            id: Some(id),
            resource_name: Some(resource_name.into()),
            ..db_resource
        }),
        Some(id.to_string()),
        Some(format!("Created {} with id {}", resource_name.to_string(), id)),
    )
}

pub async fn read(
    path: web::Path<(String, Option<String>)>,
    s_data: web::Data<Arc<ServerData>>,
    req: actix_web::HttpRequest,
) -> HttpResponse {
    let started_at = std::time::Instant::now();
    let (resource_name_raw, id_raw) = path.into_inner();
    let (response, resource_label, resource_id, details) =
        read_inner(resource_name_raw, id_raw, s_data, req.clone()).await;

    crate::log::record_http_action_async(
        Some(&req),
        crate::log::ResourceActionKind::Read,
        Some(resource_label),
        resource_id,
        response.status().as_u16(),
        started_at,
        details,
        None,
    );

    response
}

async fn read_inner(
    resource_name_raw: String,
    id: Option<String>,
    s_data: web::Data<Arc<ServerData>>,
    req: actix_web::HttpRequest,
) -> (HttpResponse, String, Option<String>, Option<String>) {
    let resource_id_log = id.clone();

    let resource_name: ResourceName = match ResourceName::from_str_case_insensitive(&resource_name_raw) {
        Some(r) => r,
        None => {
            return (
                HttpResponse::BadRequest().body("Invalid resource name"),
                resource_name_raw,
                resource_id_log,
                Some("Invalid resource name".to_string()),
            );
        }
    };
    let resource_label = resource_name.to_string();

    if resource_name == ResourceName::UserResource {
        if !is_admin(req.clone()) {
            return (
                HttpResponse::Forbidden().body("Access to UserResource is forbidden"),
                resource_label,
                resource_id_log,
                Some("Non-admin read UserResource rejected".to_string()),
            );
        }
    }

    if resource_name == ResourceName::ActionLogsResource && !is_admin(req.clone()) {
        return (
            HttpResponse::Forbidden().body("Access to ActionLogsResource is forbidden"),
            resource_label,
            resource_id_log,
            Some("Non-admin read ActionLogsResource rejected".to_string()),
        );
    }

    if resource_name == ResourceName::PermissionResource {
        if !is_admin(req) {
            return (
                HttpResponse::Forbidden().body("Access to PermissionResource is forbidden"),
                resource_label,
                resource_id_log,
                Some("Non-admin read PermissionResource rejected".to_string()),
            );
        }

        if id == Some("all".into()) {
            let db_resources = match s_data
                .db
                .read_all_resource(ResourceName::PermissionResource)
                .await
            {
                Ok(resources) => resources,
                Err(err) => {
                    let msg = format!("Failed to read PermissionResource: {:?}", err);
                    return (
                        HttpResponse::InternalServerError().body(msg.clone()),
                        resource_label,
                        resource_id_log,
                        Some(msg),
                    );
                }
            };

            if db_resources.is_empty() {
                return (
                    HttpResponse::NotFound().body(
                        "No resources found for resource name: PermissionResource in database",
                    ),
                    resource_label,
                    resource_id_log,
                    Some("No PermissionResource rows found".to_string()),
                );
            }
            let count = db_resources.len();
            return (
                HttpResponse::Ok().json(db_resources),
                resource_label,
                resource_id_log,
                Some(format!("Read {} PermissionResource row(s)", count)),
            );
        }

        let parsed_id: i32 = match id {
            Some(id_str) => match id_str.parse() {
                Ok(i) => i,
                Err(_) => {
                    return (
                        HttpResponse::BadRequest().body("Invalid ID format"),
                        resource_label,
                        resource_id_log,
                        Some("Invalid ID format".to_string()),
                    );
                }
            },
            None => {
                return (
                    HttpResponse::BadRequest().body("ID is required for reading a resource"),
                    resource_label,
                    resource_id_log,
                    Some("Missing ID".to_string()),
                );
            }
        };

        let db_resource = match s_data
            .db
            .read_resource(ResourceName::PermissionResource, parsed_id)
            .await
        {
            Ok(resource) => resource,
            Err(err) => {
                let msg = format!("Failed to read PermissionResource: {:?}", err);
                return (
                    HttpResponse::InternalServerError().body(msg.clone()),
                    resource_label,
                    resource_id_log,
                    Some(msg),
                );
            }
        };

        if db_resource.id.is_none() {
            return (
                HttpResponse::NotFound().body(format!("Resource not found for id: {}", parsed_id)),
                resource_label,
                resource_id_log,
                Some(format!("PermissionResource {} not found", parsed_id)),
            );
        }

        return (
            HttpResponse::Ok().json(db_resource),
            resource_label,
            resource_id_log,
            Some(format!("Read PermissionResource {}", parsed_id)),
        );
    }

    let cache = s_data.cache.read().await;

    let Ok(cache_result) = cache.read_all(resource_name.clone()) else {
        let db_resources = match s_data.db.read_all_resource(resource_name.clone()).await {
            Ok(resources) => resources,
            Err(err) => {
                let msg = format!("Failed to read all resources: {:?}", err);
                return (
                    HttpResponse::InternalServerError().body(msg.clone()),
                    resource_label,
                    resource_id_log,
                    Some(msg),
                );
            }
        };

        if db_resources.is_empty() {
            return (
                HttpResponse::NotFound().body(format!(
                    "No resources found for resource name : {:?} in the database",
                    resource_name
                )),
                resource_label,
                resource_id_log,
                Some("No resources found in database".to_string()),
            );
        }

        let count = db_resources.len();
        return (
            HttpResponse::Ok().json(db_resources),
            resource_label,
            resource_id_log,
            Some(format!("Read {} row(s) from database", count)),
        );
    };

    if id == Some("all".into()) {
        if cache_result.is_empty() {
            let db_resources = match s_data.db.read_all_resource(resource_name.clone()).await {
                Ok(resources) => resources,
                Err(err) => {
                    let msg = format!("Failed to read all resources: {:?}", err);
                    return (
                        HttpResponse::InternalServerError().body(msg.clone()),
                        resource_label,
                        resource_id_log,
                        Some(msg),
                    );
                }
            };

            if db_resources.is_empty() {
                return (
                    HttpResponse::NotFound().body(format!(
                        "No resources found for resource name: {:?} in database",
                        resource_name
                    )),
                    resource_label,
                    resource_id_log,
                    Some("No resources found in database".to_string()),
                );
            }

            // Cache miss -> populate cache
            drop(cache);
            {
                let mut cache = s_data.cache.write().await;

                for resource in db_resources.clone() {
                    if let Some(id) = resource.id {
                        cache.write(resource_name.clone(), id, resource);
                    }
                }
            }
            let count = db_resources.len();
            return (
                HttpResponse::Ok().json(db_resources),
                resource_label,
                resource_id_log,
                Some(format!("Read {} row(s) (cache populated)", count)),
            );
        } else {
            let count = cache_result.len();
            return (
                HttpResponse::Ok().json(cache_result),
                resource_label,
                resource_id_log,
                Some(format!("Read {} row(s) from cache", count)),
            );
        }
    }

    let id: Option<i32> = match id {
        Some(id_str) => match id_str.parse() {
            Ok(i) => Some(i),
            Err(_) => {
                return (
                    HttpResponse::BadRequest().body("Invalid ID format"),
                    resource_label,
                    resource_id_log,
                    Some("Invalid ID format".to_string()),
                );
            }
        },
        None => None,
    };

    match id {
        None => (
            HttpResponse::BadRequest().body("ID is required for reading a resource"),
            resource_label,
            resource_id_log,
            Some("Missing ID".to_string()),
        ),

        Some(id) => {
            if let Some(resource) = cache.read(resource_name.clone(), id) {
                return (
                    HttpResponse::Ok().json(resource),
                    resource_label,
                    resource_id_log,
                    Some(format!("Cache hit for id {}", id)),
                );
            }

            let db_resource = match s_data.db.read_resource(resource_name.clone(), id).await {
                Ok(resource) => resource,
                Err(err) => {
                    let msg = format!("Failed to read resource: {:?}", err);
                    return (
                        HttpResponse::InternalServerError().body(msg.clone()),
                        resource_label,
                        resource_id_log,
                        Some(msg),
                    );
                }
            };

            if db_resource.id.is_none() {
                return (
                    HttpResponse::NotFound().body(format!("Resource not found for id: {}", id)),
                    resource_label,
                    resource_id_log,
                    Some(format!("Resource not found for id {}", id)),
                );
            }

            // Cache miss -> populate cache
            drop(cache);
            {
                let mut cache = s_data.cache.write().await;

                cache.write(resource_name, id, db_resource.clone());
            }

            (
                HttpResponse::Ok().json(db_resource),
                resource_label,
                resource_id_log,
                Some(format!("Read id {} from database", id)),
            )
        }
    }
}

pub async fn delete(
    path: web::Path<(ResourceName, i32)>,
    s_data: web::Data<Arc<ServerData>>,
    req: actix_web::HttpRequest,
) -> HttpResponse {
    let started_at = std::time::Instant::now();
    let (resource_name, id) = path.into_inner();
    let resource_label = resource_name.to_string();
    let resource_id = Some(id.to_string());
    let (response, details) = delete_inner(resource_name, id, s_data, req.clone()).await;

    crate::log::record_http_action_async(
        Some(&req),
        crate::log::ResourceActionKind::Delete,
        Some(resource_label),
        resource_id,
        response.status().as_u16(),
        started_at,
        details,
        None,
    );

    response
}

async fn delete_inner(
    resource_name: ResourceName,
    id: i32,
    s_data: web::Data<Arc<ServerData>>,
    req: actix_web::HttpRequest,
) -> (HttpResponse, Option<String>) {
    if !is_admin(req) {
        return (
            HttpResponse::Forbidden().body("Access to delete resources is forbidden"),
            Some("Non-admin delete rejected".to_string()),
        );
    }

    if let Err(err) = s_data.db.delete_resource(resource_name.clone(), id).await {
        let msg = format!("Failed to delete resource: {:?}", err);
        return (
            HttpResponse::InternalServerError().body(msg.clone()),
            Some(msg),
        );
    }

    {
        let mut cache = s_data.cache.write().await;

        //delete from cache
        cache.cache.entry(resource_name).and_modify(|resources| {
            resources.remove(&id);
        });
    }

    if resource_name == ResourceName::UserResource {
        if let Err(err) = crate::access::delete_permission_for_user(&s_data, id).await {
            eprintln!(
                "Failed to delete paired PermissionResource for UserResource {}: {}",
                id, err
            );
        }
    }

    (
        HttpResponse::Ok().json(format!("Resource deleted with id: {}", id)),
        Some(format!("Deleted {} with id {}", resource_name.to_string(), id)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[actix_web::test]
    async fn test_is_admin_true_for_admin_role() {
        let req = actix_web::test::TestRequest::default().to_http_request();
        req.extensions_mut().insert(crate::auth::Claims {
            sub: "admin@test.com".to_string(),
            exp: 9999999999,
            iat: 1000000000,
            role: crate::roles::Roles::Admin,
            user_id: None,
            jti: None,
        });

        assert!(is_admin(req));
    }

    #[actix_web::test]
    async fn test_is_admin_false_for_user_role() {
        let req = actix_web::test::TestRequest::default().to_http_request();
        req.extensions_mut().insert(crate::auth::Claims {
            sub: "user@test.com".to_string(),
            exp: 9999999999,
            iat: 1000000000,
            role: crate::roles::Roles::User,
            user_id: None,
            jti: None,
        });

        assert!(!is_admin(req));
    }

    #[actix_web::test]
    async fn test_is_admin_false_for_operator_role() {
        let req = actix_web::test::TestRequest::default().to_http_request();
        req.extensions_mut().insert(crate::auth::Claims {
            sub: "operator@test.com".to_string(),
            exp: 9999999999,
            iat: 1000000000,
            role: crate::roles::Roles::Operator,
            user_id: None,
            jti: None,
        });

        assert!(!is_admin(req));
    }

    #[actix_web::test]
    async fn test_is_admin_false_when_no_claims() {
        let req = actix_web::test::TestRequest::default().to_http_request();
        assert!(!is_admin(req));
    }

    async fn create_mock_server_data_for_update() -> web::Data<Arc<ServerData>> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let (client_io, mut server_io) = tokio::io::duplex(65536);

        tokio::spawn(async move {
            let Ok(startup_len) = server_io.read_i32().await else {
                return;
            };
            let mut startup_body = vec![0u8; (startup_len as usize).saturating_sub(4)];
            if server_io.read_exact(&mut startup_body).await.is_err() {
                return;
            }

            let handshake = [b'R', 0, 0, 0, 8, 0, 0, 0, 0, b'Z', 0, 0, 0, 5, b'I'];
            if server_io.write_all(&handshake).await.is_err() {
                return;
            }
            let _ = server_io.flush().await;

            let mut resp_buf: Vec<u8> = Vec::new();
            loop {
                let Ok(tag) = server_io.read_u8().await else {
                    break;
                };
                let Ok(msg_len) = server_io.read_i32().await else {
                    break;
                };
                let body_len = (msg_len as usize).saturating_sub(4);
                let mut body = vec![0u8; body_len];
                if server_io.read_exact(&mut body).await.is_err() {
                    break;
                }

                match tag {
                    b'P' => {
                        resp_buf.extend_from_slice(&[b'1', 0, 0, 0, 4]);
                    }
                    b'D' => {
                        if body.first() == Some(&b'S') {
                            // 3 params: JSONB (3802), JSONB (3802), INT4 (23) + NoData
                            resp_buf.extend_from_slice(&[
                                b't', 0, 0, 0, 18, 0, 3, 0, 0, 0x0e, 0xda, 0, 0, 0x0e, 0xda, 0, 0,
                                0, 23,
                            ]);
                            resp_buf.extend_from_slice(&[b'n', 0, 0, 0, 4]);
                        }
                    }
                    b'B' => {
                        resp_buf.extend_from_slice(&[b'2', 0, 0, 0, 4]);
                    }
                    b'E' => {
                        resp_buf.extend_from_slice(b"C\x00\x00\x00\x0dUPDATE 1\0");
                    }
                    b'S' => {
                        resp_buf.extend_from_slice(&[b'Z', 0, 0, 0, 5, b'I']);
                        if server_io.write_all(&resp_buf).await.is_err() {
                            break;
                        }
                        let _ = server_io.flush().await;
                        resp_buf.clear();
                    }
                    b'X' => break,
                    _ => {}
                }
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
            cache: Arc::new(tokio::sync::RwLock::new(crate::cache::Cache::new())),
        }))
    }

    #[actix_web::test]
    async fn test_update_sets_cache_id_when_payload_id_is_none() {
        let s_data = create_mock_server_data_for_update().await;
        let payload = DatabaseResource {
            id: None,
            resource_name: None,
            inner_content: Some(serde_json::json!({ "SanghBranch": "Loadboard sthaniya" })),
            external_content: None,
        };
        let req = actix_web::test::TestRequest::default().to_http_request();

        let resp = update(
            web::Path::from((ResourceName::SettingResource, 42)),
            web::Json(payload),
            req,
            s_data.clone(),
        )
        .await;

        assert_eq!(resp.status(), actix_web::http::StatusCode::OK);

        let cached = s_data
            .cache
            .read()
            .await
            .read(ResourceName::SettingResource, 42)
            .expect("updated resource should exist in cache");
        assert_eq!(cached.id, Some(42));
        assert_eq!(cached.resource_name, Some(ResourceName::SettingResource));
    }

    fn make_req_with_role(role: Option<crate::roles::Roles>) -> actix_web::HttpRequest {
        let req = actix_web::test::TestRequest::default().to_http_request();
        if let Some(r) = role {
            req.extensions_mut().insert(crate::auth::Claims {
                sub: "caller@test.com".to_string(),
                exp: 9999999999,
                iat: 1000000000,
                role: r,
                user_id: Some(1),
                jti: None,
            });
        }
        req
    }

    fn sample_billing_rule_payload(branch: Option<&str>) -> DatabaseResource {
        let mut inner = serde_json::json!({
            "scope": "COMMON",
            "amount": 500,
            "status": "ACTIVE",
            "fiscalYear": "2082/2083",
            "resourceType": "BILLING_RULE"
        });
        if let Some(b) = branch {
            inner["sthaniyaBranch"] = serde_json::Value::String(b.to_string());
        }
        DatabaseResource {
            id: None,
            resource_name: Some(ResourceName::BillingRuleResource),
            inner_content: Some(inner),
            external_content: None,
        }
    }

    #[actix_web::test]
    async fn test_billing_rule_write_only_allowed_for_admin() {
        let (s_data, db_state) = crate::access::test_helpers::create_mock_server_data_with_state(
            crate::access::test_helpers::MockDbState::default(),
        )
        .await;

        for role in [
            None,
            Some(crate::roles::Roles::User),
            Some(crate::roles::Roles::Operator),
        ] {
            let resp = write(
                web::Path::from(ResourceName::BillingRuleResource),
                web::Json(sample_billing_rule_payload(Some("Loadboard sthaniya"))),
                make_req_with_role(role),
                s_data.clone(),
            )
            .await;
            assert_eq!(resp.status(), actix_web::http::StatusCode::FORBIDDEN);
        }

        let admin_resp = write(
            web::Path::from(ResourceName::BillingRuleResource),
            web::Json(sample_billing_rule_payload(Some("Loadboard sthaniya"))),
            make_req_with_role(Some(crate::roles::Roles::Admin)),
            s_data.clone(),
        )
        .await;
        assert_eq!(admin_resp.status(), actix_web::http::StatusCode::OK);

        let guard = db_state.lock().await;
        assert_eq!(
            guard.tables.get("billingruleresource").map(Vec::len),
            Some(1)
        );
    }

    #[actix_web::test]
    async fn test_billing_rule_update_only_allowed_for_admin() {
        let mut state = crate::access::test_helpers::MockDbState::default();
        state.seed(
            ResourceName::BillingRuleResource,
            DatabaseResource {
                id: Some(12),
                resource_name: Some(ResourceName::BillingRuleResource),
                inner_content: Some(serde_json::json!({
                    "amount": 2000,
                    "status": "ACTIVE",
                    "fiscalYear": "2082/83",
                    "resourceType": "BILLING_RULE"
                })),
                external_content: None,
            },
        );
        let (s_data, db_state) =
            crate::access::test_helpers::create_mock_server_data_with_state(state).await;

        for role in [
            None,
            Some(crate::roles::Roles::User),
            Some(crate::roles::Roles::Operator),
        ] {
            let resp = update(
                web::Path::from((ResourceName::BillingRuleResource, 12)),
                web::Json(sample_billing_rule_payload(Some("Loadboard sthaniya"))),
                make_req_with_role(role),
                s_data.clone(),
            )
            .await;
            assert_eq!(resp.status(), actix_web::http::StatusCode::FORBIDDEN);
        }

        let admin_resp = update(
            web::Path::from((ResourceName::BillingRuleResource, 12)),
            web::Json(sample_billing_rule_payload(Some("Loadboard sthaniya"))),
            make_req_with_role(Some(crate::roles::Roles::Admin)),
            s_data.clone(),
        )
        .await;
        assert_eq!(admin_resp.status(), actix_web::http::StatusCode::OK);

        let guard = db_state.lock().await;
        let updated = &guard.tables.get("billingruleresource").unwrap()[0];
        assert_eq!(
            updated
                .inner_content
                .as_ref()
                .and_then(|v| v.get("sthaniyaBranch"))
                .and_then(|v| v.as_str()),
            Some("Loadboard sthaniya")
        );
    }

    #[actix_web::test]
    async fn test_billing_rule_delete_only_allowed_for_admin() {
        let mut state = crate::access::test_helpers::MockDbState::default();
        state.seed(
            ResourceName::BillingRuleResource,
            DatabaseResource {
                id: Some(29),
                resource_name: Some(ResourceName::BillingRuleResource),
                inner_content: Some(serde_json::json!({ "name": "Entry" })),
                external_content: None,
            },
        );
        let (s_data, db_state) =
            crate::access::test_helpers::create_mock_server_data_with_state(state).await;

        for role in [
            None,
            Some(crate::roles::Roles::User),
            Some(crate::roles::Roles::Operator),
        ] {
            let resp = delete(
                web::Path::from((ResourceName::BillingRuleResource, 29)),
                s_data.clone(),
                make_req_with_role(role),
            )
            .await;
            assert_eq!(resp.status(), actix_web::http::StatusCode::FORBIDDEN);
        }

        let admin_resp = delete(
            web::Path::from((ResourceName::BillingRuleResource, 29)),
            s_data.clone(),
            make_req_with_role(Some(crate::roles::Roles::Admin)),
        )
        .await;
        assert_eq!(admin_resp.status(), actix_web::http::StatusCode::OK);

        let guard = db_state.lock().await;
        assert_eq!(
            guard.tables.get("billingruleresource").map(Vec::len),
            Some(0)
        );
    }

    #[actix_web::test]
    async fn test_billing_rule_read_allowed_for_non_admin() {
        let mut state = crate::access::test_helpers::MockDbState::default();
        state.seed(
            ResourceName::BillingRuleResource,
            DatabaseResource {
                id: Some(29),
                resource_name: Some(ResourceName::BillingRuleResource),
                inner_content: Some(serde_json::json!({
                    "name": "Entry",
                    "sthaniyaBranch": "Loadboard sthaniya"
                })),
                external_content: None,
            },
        );
        let (s_data, _) =
            crate::access::test_helpers::create_mock_server_data_with_state(state).await;

        for role in [
            Some(crate::roles::Roles::User),
            Some(crate::roles::Roles::Operator),
        ] {
            let resp = read(
                web::Path::from(("BillingRuleResource".to_string(), Some("29".to_string()))),
                s_data.clone(),
                make_req_with_role(role),
            )
            .await;
            assert_eq!(resp.status(), actix_web::http::StatusCode::OK);
        }
    }

    #[actix_web::test]
    async fn test_all_actions_emit_async_logs_for_resources() {
        let mut state = crate::access::test_helpers::MockDbState::default();
        let actor_email = format!("actions-audit-{}@test.com", crate::log::generate_action_log_id());
        state.seed(
            ResourceName::UserResource,
            DatabaseResource {
                id: Some(901),
                resource_name: Some(ResourceName::UserResource),
                inner_content: Some(serde_json::json!({
                    "email": actor_email,
                    "password": "admin-secret-password",
                    "role": "Admin",
                    "SanghBranch": "Loadboard sthaniya"
                })),
                external_content: None,
            },
        );
        let (s_data, _) =
            crate::access::test_helpers::create_mock_server_data_with_state(state).await;

        let admin_req = actix_web::test::TestRequest::default().to_http_request();
        admin_req.extensions_mut().insert(crate::auth::Claims {
            sub: actor_email.clone(),
            exp: 9999999999,
            iat: 1000000000,
            role: crate::roles::Roles::Admin,
            user_id: Some(901),
            jti: None,
        });

        let operator_req = actix_web::test::TestRequest::default().to_http_request();
        operator_req.extensions_mut().insert(crate::auth::Claims {
            sub: actor_email.clone(),
            exp: 9999999999,
            iat: 1000000000,
            role: crate::roles::Roles::Operator,
            user_id: Some(902),
            jti: None,
        });

        // 1. Write SettingResource
        let unique_branch = format!("AuditBranch-{}", crate::log::generate_action_log_id());
        let w_setting = write(
            web::Path::from(ResourceName::SettingResource),
            web::Json(DatabaseResource {
                id: None,
                resource_name: None,
                inner_content: Some(serde_json::json!({ "SanghBranch": unique_branch })),
                external_content: None,
            }),
            admin_req.clone(),
            s_data.clone(),
        )
        .await;
        assert_eq!(w_setting.status(), actix_web::http::StatusCode::OK);
        let w_setting_bytes = actix_web::body::to_bytes(w_setting.into_body())
            .await
            .expect("read w_setting body");
        let created_setting: DatabaseResource =
            serde_json::from_slice(&w_setting_bytes).expect("parse created_setting");
        let setting_id = created_setting.id.unwrap();

        // 2. Read SettingResource
        let r_setting = read(
            web::Path::from(("SettingResource".to_string(), Some(setting_id.to_string()))),
            s_data.clone(),
            admin_req.clone(),
        )
        .await;
        assert_eq!(r_setting.status(), actix_web::http::StatusCode::OK);

        // 3. Search SettingResource
        let s_setting = search(
            web::Path::from((
                "SettingResource".to_string(),
                "SanghBranch".to_string(),
                unique_branch.clone(),
                "10".to_string(),
            )),
            admin_req.clone(),
            s_data.clone(),
        )
        .await;
        assert_eq!(s_setting.status(), actix_web::http::StatusCode::OK);

        // 4. Update SettingResource
        let u_setting = update(
            web::Path::from((ResourceName::SettingResource, setting_id)),
            web::Json(DatabaseResource {
                id: None,
                resource_name: None,
                inner_content: Some(serde_json::json!({ "SanghBranch": unique_branch })),
                external_content: None,
            }),
            admin_req.clone(),
            s_data.clone(),
        )
        .await;
        assert_eq!(u_setting.status(), actix_web::http::StatusCode::OK);

        // 5. Forbidden BillingRuleResource write by Operator
        let w_rule_forbidden = write(
            web::Path::from(ResourceName::BillingRuleResource),
            web::Json(sample_billing_rule_payload(Some("Loadboard sthaniya"))),
            operator_req,
            s_data.clone(),
        )
        .await;
        assert_eq!(w_rule_forbidden.status(), actix_web::http::StatusCode::FORBIDDEN);

        // 6. Create + Delete UserResource (verifying no password leaked in logs)
        let secret_pw = "SuperSecretUserPass987!";
        let w_user = write(
            web::Path::from(ResourceName::UserResource),
            web::Json(DatabaseResource {
                id: None,
                resource_name: None,
                inner_content: Some(serde_json::json!({
                    "email": "created-audit@test.com",
                    "password": secret_pw,
                    "role": "User"
                })),
                external_content: None,
            }),
            admin_req.clone(),
            s_data.clone(),
        )
        .await;
        assert_eq!(w_user.status(), actix_web::http::StatusCode::OK);
        let w_user_bytes = actix_web::body::to_bytes(w_user.into_body())
            .await
            .expect("read w_user body");
        let created_user: DatabaseResource =
            serde_json::from_slice(&w_user_bytes).expect("parse created_user");
        let created_uid = created_user.id.unwrap();

        let d_setting = delete(
            web::Path::from((ResourceName::SettingResource, setting_id)),
            s_data.clone(),
            admin_req,
        )
        .await;
        assert_eq!(d_setting.status(), actix_web::http::StatusCode::OK);

        crate::log::flush_action_logs().await;

        let actor_logs: Vec<_> = crate::log::get_all_action_logs()
            .into_iter()
            .filter(|e| e.performed_by.as_deref() == Some(actor_email.as_str()))
            .collect();

        assert!(actor_logs.iter().any(|e| {
            e.action == crate::log::ResourceActionKind::Create
                && e.resource_name.as_deref() == Some("SettingResource")
                && e.resource_id.as_deref() == Some(setting_id.to_string().as_str())
                && e.success
        }));
        assert!(actor_logs.iter().any(|e| {
            e.action == crate::log::ResourceActionKind::Read
                && e.resource_name.as_deref() == Some("SettingResource")
                && e.success
        }));
        assert!(actor_logs.iter().any(|e| {
            e.action == crate::log::ResourceActionKind::Search
                && e.resource_name.as_deref() == Some("SettingResource")
                && e.success
        }));
        assert!(actor_logs.iter().any(|e| {
            e.action == crate::log::ResourceActionKind::Update
                && e.resource_name.as_deref() == Some("SettingResource")
                && e.success
        }));
        assert!(actor_logs.iter().any(|e| {
            e.action == crate::log::ResourceActionKind::Delete
                && e.resource_name.as_deref() == Some("SettingResource")
                && e.success
        }));
        assert!(actor_logs.iter().any(|e| {
            e.action == crate::log::ResourceActionKind::Create
                && e.resource_name.as_deref() == Some("BillingRuleResource")
                && e.status_code == 403
                && !e.success
        }));
        assert!(actor_logs.iter().any(|e| {
            e.action == crate::log::ResourceActionKind::Create
                && e.resource_name.as_deref() == Some("UserResource")
                && e.resource_id.as_deref() == Some(created_uid.to_string().as_str())
                && e.success
        }));

        // Password must never appear anywhere in logs
        for entry in &actor_logs {
            let serialized = serde_json::to_string(entry).unwrap();
            assert!(!serialized.contains(secret_pw));
        }
    }

    #[actix_web::test]
    async fn test_displaced_member_resource_create_and_update_respect_setting_resource() {
        let mut state = crate::access::test_helpers::MockDbState::default();
        let initial_setting = DatabaseResource {
            id: Some(1),
            resource_name: Some(ResourceName::SettingResource),
            inner_content: Some(serde_json::json!({
                "schemaVersion": 1,
                "vehicleManagement": {
                    "displacedTagging": { "enabled": false }
                }
            })),
            external_content: None,
        };
        state.seed(ResourceName::SettingResource, initial_setting.clone());
        state.seed(
            ResourceName::MemberResource,
            DatabaseResource {
                id: Some(50),
                resource_name: Some(ResourceName::MemberResource),
                inner_content: Some(serde_json::json!({
                    "vehicleId": "7001",
                    "vehicleNumber": "NA-01-KA-7001",
                    "isDisplaced": false,
                    "status": "ACTIVE"
                })),
                external_content: None,
            },
        );
        let (s_data, _) =
            crate::access::test_helpers::create_mock_server_data_with_state(state).await;
        s_data
            .cache
            .write()
            .await
            .write(ResourceName::SettingResource, 1, initial_setting);
        let admin_req = make_req_with_role(Some(crate::roles::Roles::Admin));

        let displaced_payload = DatabaseResource {
            id: None,
            resource_name: None,
            inner_content: Some(serde_json::json!({
                "vehicleId": "7002",
                "vehicleNumber": "NA-01-KA-7002",
                "isDisplaced": true,
                "status": "DISPLACED"
            })),
            external_content: None,
        };

        // 1. Displaced MemberResource create is forbidden while disabled
        let create_blocked = write(
            web::Path::from(ResourceName::MemberResource),
            web::Json(displaced_payload.clone()),
            admin_req.clone(),
            s_data.clone(),
        )
        .await;
        assert_eq!(
            create_blocked.status(),
            actix_web::http::StatusCode::FORBIDDEN
        );

        // 2. Displaced MemberResource update is forbidden while disabled
        let update_blocked = update(
            web::Path::from((ResourceName::MemberResource, 50)),
            web::Json(displaced_payload.clone()),
            admin_req.clone(),
            s_data.clone(),
        )
        .await;
        assert_eq!(
            update_blocked.status(),
            actix_web::http::StatusCode::FORBIDDEN
        );

        // 3. Non-displaced MemberResource create is allowed even when disabled
        let create_regular = write(
            web::Path::from(ResourceName::MemberResource),
            web::Json(DatabaseResource {
                id: None,
                resource_name: None,
                inner_content: Some(serde_json::json!({
                    "vehicleId": "7003",
                    "vehicleNumber": "NA-01-KA-7003",
                    "isDisplaced": false,
                    "status": "ACTIVE"
                })),
                external_content: None,
            }),
            admin_req.clone(),
            s_data.clone(),
        )
        .await;
        assert_eq!(create_regular.status(), actix_web::http::StatusCode::OK);

        // 4. Enable displacedTagging in SettingResource -> both create and update succeed
        let enable_setting = update(
            web::Path::from((ResourceName::SettingResource, 1)),
            web::Json(DatabaseResource {
                id: Some(1),
                resource_name: Some(ResourceName::SettingResource),
                inner_content: Some(serde_json::json!({
                    "schemaVersion": 1,
                    "vehicleManagement": {
                        "displacedTagging": { "enabled": true }
                    }
                })),
                external_content: None,
            }),
            admin_req.clone(),
            s_data.clone(),
        )
        .await;
        assert_eq!(enable_setting.status(), actix_web::http::StatusCode::OK);

        let create_allowed = write(
            web::Path::from(ResourceName::MemberResource),
            web::Json(displaced_payload.clone()),
            admin_req.clone(),
            s_data.clone(),
        )
        .await;
        assert_eq!(create_allowed.status(), actix_web::http::StatusCode::OK);

        let update_allowed = update(
            web::Path::from((ResourceName::MemberResource, 50)),
            web::Json(displaced_payload),
            admin_req,
            s_data,
        )
        .await;
        assert_eq!(update_allowed.status(), actix_web::http::StatusCode::OK);
    }

    #[actix_web::test]
    async fn test_action_logs_resource_admin_only_access_for_all_actions() {
        let mut state = crate::access::test_helpers::MockDbState::default();
        state.seed(
            ResourceName::ActionLogsResource,
            DatabaseResource {
                id: Some(101),
                resource_name: Some(ResourceName::ActionLogsResource),
                inner_content: Some(serde_json::json!({
                    "id": "ACTLOG-101",
                    "action": "Create",
                    "resource_name": "BillResource",
                    "performed_by": "admin@narayani.com",
                    "role": "Admin",
                    "status_code": 200,
                    "success": true
                })),
                external_content: None,
            },
        );
        // Intentionally keep cache empty at first to exercise cache-miss DB read path as well.
        let (s_data, db_state) =
            crate::access::test_helpers::create_mock_server_data_with_state(state).await;

        let sample_log_payload = DatabaseResource {
            id: None,
            resource_name: Some(ResourceName::ActionLogsResource),
            inner_content: Some(serde_json::json!({
                "id": "ACTLOG-202",
                "action": "Login",
                "performed_by": "admin@narayani.com",
                "role": "Admin",
                "status_code": 200,
                "success": true
            })),
            external_content: None,
        };

        // 1. Unauthenticated, User, and Operator roles must be forbidden on all actions
        for role in [
            None,
            Some(crate::roles::Roles::User),
            Some(crate::roles::Roles::Operator),
        ] {
            let read_one = read(
                web::Path::from(("ActionLogsResource".to_string(), Some("101".to_string()))),
                s_data.clone(),
                make_req_with_role(role.clone()),
            )
            .await;
            assert_eq!(read_one.status(), actix_web::http::StatusCode::FORBIDDEN);

            let read_all = read(
                web::Path::from(("ActionLogsResource".to_string(), Some("all".to_string()))),
                s_data.clone(),
                make_req_with_role(role.clone()),
            )
            .await;
            assert_eq!(read_all.status(), actix_web::http::StatusCode::FORBIDDEN);

            let search_resp = search(
                web::Path::from((
                    "ActionLogsResource".to_string(),
                    "role".to_string(),
                    "Admin".to_string(),
                    "10".to_string(),
                )),
                make_req_with_role(role.clone()),
                s_data.clone(),
            )
            .await;
            assert_eq!(search_resp.status(), actix_web::http::StatusCode::FORBIDDEN);

            let create_resp = write(
                web::Path::from(ResourceName::ActionLogsResource),
                web::Json(sample_log_payload.clone()),
                make_req_with_role(role.clone()),
                s_data.clone(),
            )
            .await;
            assert_eq!(create_resp.status(), actix_web::http::StatusCode::FORBIDDEN);

            let update_resp = update(
                web::Path::from((ResourceName::ActionLogsResource, 101)),
                web::Json(sample_log_payload.clone()),
                make_req_with_role(role.clone()),
                s_data.clone(),
            )
            .await;
            assert_eq!(update_resp.status(), actix_web::http::StatusCode::FORBIDDEN);

            let delete_resp = delete(
                web::Path::from((ResourceName::ActionLogsResource, 101)),
                s_data.clone(),
                make_req_with_role(role),
            )
            .await;
            assert_eq!(delete_resp.status(), actix_web::http::StatusCode::FORBIDDEN);
        }

        // 2. Admin can read (cache-miss -> DB + populates cache without deadlocking),
        // read all, create, search, update, and delete ActionLogsResource
        let admin_read_miss = read(
            web::Path::from(("ActionLogsResource".to_string(), Some("101".to_string()))),
            s_data.clone(),
            make_req_with_role(Some(crate::roles::Roles::Admin)),
        )
        .await;
        assert_eq!(admin_read_miss.status(), actix_web::http::StatusCode::OK);

        let admin_create = write(
            web::Path::from(ResourceName::ActionLogsResource),
            web::Json(sample_log_payload.clone()),
            make_req_with_role(Some(crate::roles::Roles::Admin)),
            s_data.clone(),
        )
        .await;
        assert_eq!(admin_create.status(), actix_web::http::StatusCode::OK);

        let admin_read_all = read(
            web::Path::from(("ActionLogsResource".to_string(), Some("all".to_string()))),
            s_data.clone(),
            make_req_with_role(Some(crate::roles::Roles::Admin)),
        )
        .await;
        assert_eq!(admin_read_all.status(), actix_web::http::StatusCode::OK);

        let admin_search = search(
            web::Path::from((
                "ActionLogsResource".to_string(),
                "role".to_string(),
                "Admin".to_string(),
                "10".to_string(),
            )),
            make_req_with_role(Some(crate::roles::Roles::Admin)),
            s_data.clone(),
        )
        .await;
        assert_eq!(admin_search.status(), actix_web::http::StatusCode::OK);

        let admin_update = update(
            web::Path::from((ResourceName::ActionLogsResource, 101)),
            web::Json(sample_log_payload),
            make_req_with_role(Some(crate::roles::Roles::Admin)),
            s_data.clone(),
        )
        .await;
        assert_eq!(admin_update.status(), actix_web::http::StatusCode::OK);

        let admin_delete = delete(
            web::Path::from((ResourceName::ActionLogsResource, 101)),
            s_data,
            make_req_with_role(Some(crate::roles::Roles::Admin)),
        )
        .await;
        assert_eq!(admin_delete.status(), actix_web::http::StatusCode::OK);

        let guard = db_state.lock().await;
        let table = ResourceName::ActionLogsResource.to_string().to_lowercase();
        let remaining = guard.tables.get(&table).cloned().unwrap_or_default();
        assert_eq!(remaining.len(), 1);
    }
}
