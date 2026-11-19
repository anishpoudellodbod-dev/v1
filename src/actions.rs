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
        Ok(r) => if r == ResourceName::UserResource {
            return HttpResponse::Forbidden().body("Search forbidden in UserResource is forbidden");
        } else {
            r
        },
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

    if resource_name == ResourceName::UserResource {
        if !is_admin(req.clone()) {
            return HttpResponse::Forbidden().body("Access to UserResource is forbidden");
        }else{
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

    {
        let mut cache = s_data.cache.write().await;

        cache.write(
            resource_name,
            id,
            DatabaseResource {
                id: Some(id),
                resource_name: Some(resource_name),
                ..db_resource
            },
        );
    }

    HttpResponse::Ok().json(format!("Resource updated with id: {}", id))
}

pub async fn write(
    path: web::Path<ResourceName>,
    db_resource: web::Json<DatabaseResource>,
    req: actix_web::HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let db_resource = db_resource.into_inner();
    let resource_name = path.into_inner();
    if resource_name == ResourceName::UserResource {
        if !is_admin(req) {
            return HttpResponse::Forbidden().body("Access to UserResource is forbidden");
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
        if !is_admin(req) {
            return HttpResponse::Forbidden().body("Access to UserResource is forbidden");
        }
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
}
