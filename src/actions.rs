use crate::resource::{DatabaseResource, ResourceName};
use crate::ServerData;
use actix_web::{web, HttpResponse, HttpMessage};

use std::sync::Arc;

pub async fn search(
    path: web::Path<(String, String, String, String)>,
    s_data: web::Data<Arc<ServerData>>
) -> HttpResponse {
    let (resource_name, search_item, search_query, limit) = path.into_inner();

    let resource_name: ResourceName = match resource_name.try_into() {
        Ok(r) => {
            if r == ResourceName::UserResource || r == ResourceName::PermissionResource {
                return HttpResponse::Forbidden().body(format!(
                    "Search forbidden in {:?} is forbidden",
                    r
                ));
            } else {
                r
            }
        }
        Err(_) => {
            return HttpResponse::BadRequest().body("Invalid resource name");
        }
    };
    let ulimit: usize;
    if limit == "all"{
        ulimit = usize::MAX;
    }
    else{
        ulimit = match limit.parse(){
            Ok(i) => i,
            Err(_) => {
                return HttpResponse::BadRequest().body("Invalid limit");
            }
        }
    };

    let cache = s_data.cache.read().await;

    let db_resources = cache.search_innercontent(resource_name, &search_item, &search_query, ulimit);

    if db_resources.1 == 0 {
        HttpResponse::NotFound().body(format!(
            "No resources found for search query: {}",
            search_query
        ))
    } else {
        HttpResponse::Ok().json(db_resources)
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
    let (resource_name, id) = path.into_inner();
    let db_resource = db_resource.into_inner();

    if resource_name == ResourceName::PermissionResource {
        if !is_admin(req.clone()) {
            return HttpResponse::Forbidden().body("Access to PermissionResource is forbidden");
        }

        let Some(inner_val) = db_resource.inner_content.clone() else {
            return HttpResponse::BadRequest()
                .body("PermissionResource inner_content is required");
        };
        let mut perm_inner: crate::access::PermissionInnerContent =
            match serde_json::from_value(inner_val) {
                Ok(parsed) => parsed,
                Err(err) => {
                    return HttpResponse::BadRequest()
                        .body(format!("Invalid PermissionResource inner_content: {}", err));
                }
            };
        perm_inner.sthaniya_branch =
            crate::access::normalize_optional_branch(perm_inner.sthaniya_branch);

        match crate::access::find_user_by_id_in_db(&s_data.db, perm_inner.user_id).await {
            Ok(Some(_)) => {}
            Ok(None) => {
                return HttpResponse::NotFound().body(format!(
                    "UserResource with id {} not found",
                    perm_inner.user_id
                ));
            }
            Err(err) => {
                return HttpResponse::InternalServerError()
                    .body(format!("Failed to verify user for PermissionResource: {:?}", err));
            }
        }

        match crate::access::find_permission_by_user_id_in_db(&s_data.db, perm_inner.user_id).await
        {
            Ok(Some(existing)) if existing.id != Some(id) => {
                return HttpResponse::Conflict().body(format!(
                    "PermissionResource already exists for userId {} (1:1 mapping enforced)",
                    perm_inner.user_id
                ));
            }
            Ok(_) => {}
            Err(err) => {
                return HttpResponse::InternalServerError().body(err);
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
            return HttpResponse::InternalServerError()
                .body(format!("Failed to update resource: {:?}", err));
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

        return HttpResponse::Ok().json(format!("Resource updated with id: {}", id));
    }

    if resource_name == ResourceName::BillingRuleResource && !is_admin(req.clone()) {
        return HttpResponse::Forbidden().body("Only Admin can modify BillingRuleResource");
    }

    if resource_name == ResourceName::UserResource {
        if !is_admin(req.clone()) {
            return HttpResponse::Forbidden().body("Access to UserResource is forbidden");
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
                                return HttpResponse::Forbidden().body("Admin cannot update oneself");
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
        return HttpResponse::InternalServerError()
            .body(format!("Failed to update resource: {:?}", err));
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

    HttpResponse::Ok().json(format!("Resource updated with id: {}", id))
}

pub async fn write(
    path: web::Path<ResourceName>,
    db_resource: web::Json<DatabaseResource>,
    req: actix_web::HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let mut db_resource = db_resource.into_inner();
    let resource_name = path.into_inner();

    if resource_name == ResourceName::PermissionResource {
        if !is_admin(req.clone()) {
            return HttpResponse::Forbidden().body("Access to PermissionResource is forbidden");
        }

        let Some(inner_val) = db_resource.inner_content.clone() else {
            return HttpResponse::BadRequest()
                .body("PermissionResource inner_content is required");
        };
        let mut perm_inner: crate::access::PermissionInnerContent =
            match serde_json::from_value(inner_val) {
                Ok(parsed) => parsed,
                Err(err) => {
                    return HttpResponse::BadRequest()
                        .body(format!("Invalid PermissionResource inner_content: {}", err));
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
                HttpResponse::Ok().json(created.to_database_resource())
            }
            Err(err) => err.to_http_response(),
        };
    }

    if resource_name == ResourceName::BillingRuleResource && !is_admin(req.clone()) {
        return HttpResponse::Forbidden().body("Only Admin can modify BillingRuleResource");
    }

    let mut effective_user_branch: Option<String> = None;

    if resource_name == ResourceName::UserResource {
        if !is_admin(req.clone()) {
            return HttpResponse::Forbidden().body("Access to UserResource is forbidden");
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
            return HttpResponse::InternalServerError()
                .body(format!("Failed to write resource: {:?}", err));
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

    HttpResponse::Ok().json(DatabaseResource {
        id: Some(id),
        resource_name: Some(resource_name.into()),
        ..db_resource
    })
}

pub async fn read(
    path: web::Path<(String, Option<String>)>,
    s_data: web::Data<Arc<ServerData>>,
    req: actix_web::HttpRequest,
) -> HttpResponse {
    let (resource_name, id) = path.into_inner();

    let resource_name: ResourceName = match resource_name.try_into() {
        Ok(r) => r,
        Err(_) => {
            return HttpResponse::BadRequest().body("Invalid resource name");
        }
    };

    if resource_name == ResourceName::UserResource {
        if !is_admin(req.clone()) {
            return HttpResponse::Forbidden().body("Access to UserResource is forbidden");
        }
    }

    if resource_name == ResourceName::PermissionResource {
        if !is_admin(req) {
            return HttpResponse::Forbidden().body("Access to PermissionResource is forbidden");
        }

        if id == Some("all".into()) {
            let db_resources = match s_data
                .db
                .read_all_resource(ResourceName::PermissionResource)
                .await
            {
                Ok(resources) => resources,
                Err(err) => {
                    return HttpResponse::InternalServerError()
                        .body(format!("Failed to read PermissionResource: {:?}", err));
                }
            };

            if db_resources.is_empty() {
                return HttpResponse::NotFound()
                    .body("No resources found for resource name: PermissionResource in database");
            }
            return HttpResponse::Ok().json(db_resources);
        }

        let parsed_id: i32 = match id {
            Some(id_str) => match id_str.parse() {
                Ok(i) => i,
                Err(_) => return HttpResponse::BadRequest().body("Invalid ID format"),
            },
            None => {
                return HttpResponse::BadRequest().body("ID is required for reading a resource");
            }
        };

        let db_resource = match s_data
            .db
            .read_resource(ResourceName::PermissionResource, parsed_id)
            .await
        {
            Ok(resource) => resource,
            Err(err) => {
                return HttpResponse::InternalServerError()
                    .body(format!("Failed to read PermissionResource: {:?}", err));
            }
        };

        if db_resource.id.is_none() {
            return HttpResponse::NotFound()
                .body(format!("Resource not found for id: {}", parsed_id));
        }

        return HttpResponse::Ok().json(db_resource);
    }

    let cache = s_data.cache.read().await;

    let Ok(cache_result) = cache.read_all(resource_name.clone()) else {
        let db_resources = match s_data.db.read_all_resource(resource_name.clone()).await {
            Ok(resources) => resources,
            Err(err) => {
                return HttpResponse::InternalServerError()
                    .body(format!("Failed to read all resources: {:?}", err));
            }
        };

        if db_resources.is_empty() {
            return HttpResponse::NotFound().body(format!(
                "No resources found for resource name : {:?} in the database",
                resource_name
            ));
        }

        return HttpResponse::Ok().json(db_resources);
    };

    if id == Some("all".into()) {
        if cache_result.is_empty() {
            let db_resources = match s_data.db.read_all_resource(resource_name.clone()).await {
                Ok(resources) => resources,
                Err(err) => {
                    return HttpResponse::InternalServerError()
                        .body(format!("Failed to read all resources: {:?}", err));
                }
            };

            if db_resources.is_empty() {
                return HttpResponse::NotFound().body(format!(
                    "No resources found for resource name: {:?} in database",
                    resource_name
                ));
            }

            // Cache miss -> populate cache
            {
                let mut cache = s_data.cache.write().await;

                for resource in db_resources.clone() {
                    if let Some(id) = resource.id {
                        cache.write(resource_name.clone(), id, resource);
                    }
                }
            }
            return HttpResponse::Ok().json(db_resources);
        } else {
            return HttpResponse::Ok().json(cache_result);
        }
    }

    let id: Option<i32> = match id {
        Some(id_str) => match id_str.parse() {
            Ok(i) => Some(i),
            Err(_) => {
                return HttpResponse::BadRequest().body("Invalid ID format");
            }
        },
        None => None,
    };

    match id {
        None => {
            return HttpResponse::BadRequest().body("ID is required for reading a resource");
        }

        Some(id) => {
            if let Some(resource) = cache.read(resource_name.clone(), id) {
                return HttpResponse::Ok().json(resource);
            }

            let db_resource = match s_data.db.read_resource(resource_name.clone(), id).await {
                Ok(resource) => resource,
                Err(err) => {
                    return HttpResponse::InternalServerError()
                        .body(format!("Failed to read resource: {:?}", err));
                }
            };

            if db_resource.id.is_none() {
                return HttpResponse::NotFound().body(format!("Resource not found for id: {}", id));
            }

            // Cache miss -> populate cache
            {
                let mut cache = s_data.cache.write().await;

                cache.write(resource_name, id, db_resource.clone());
            }

            HttpResponse::Ok().json(db_resource)
        }
    }
}

pub async fn delete(
    path: web::Path<(ResourceName, i32)>,
    s_data: web::Data<Arc<ServerData>>,
    req: actix_web::HttpRequest,
) -> HttpResponse {

    let (resource_name, id) = path.into_inner();

    if !is_admin(req) {
        return HttpResponse::Forbidden().body("Access to delete resources is forbidden");
    }

    if let Err(err) = s_data.db.delete_resource(resource_name.clone(), id).await {
        return HttpResponse::InternalServerError()
            .body(format!("Failed to delete resource: {:?}", err));
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

    HttpResponse::Ok().json(format!("Resource deleted with id: {}", id))
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
}
