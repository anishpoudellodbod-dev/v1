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

        cache.write(resource_name, id, db_resource);
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
}
