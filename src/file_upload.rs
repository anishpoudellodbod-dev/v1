use crate::auth;
use crate::resource::ResourceName;
use crate::ServerData;
use actix_web::{web, HttpMessage, HttpRequest, HttpResponse};
use narayani_file_upload::{
    parse_upload_payload, read_file_from_disk, save_file_to_disk, FileUploadConfig,
    FileUploadError,
};
use std::sync::Arc;

fn resolve_upload_config(req: &HttpRequest) -> FileUploadConfig {
    req.app_data::<web::Data<FileUploadConfig>>()
        .map(|cfg| cfg.get_ref().clone())
        .unwrap_or_else(FileUploadConfig::from_env)
}

pub async fn upload_file(
    req: HttpRequest,
    body: web::Bytes,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let config = resolve_upload_config(&req);

    let content_type_header = req
        .headers()
        .get(actix_web::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let fallback_filename = req
        .headers()
        .get("X-File-Name")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let uploaded_by = req
        .extensions()
        .get::<auth::Claims>()
        .map(|claims| claims.sub.clone());

    let parsed = match parse_upload_payload(
        content_type_header.as_deref(),
        fallback_filename.as_deref(),
        &body,
    ) {
        Ok(p) => p,
        Err(FileUploadError::EmptyFile) | Err(FileUploadError::InvalidMultipart(_)) => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "success": false,
                "message": "Invalid or empty file upload payload"
            }));
        }
        Err(err) => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "success": false,
                "message": err.to_string()
            }));
        }
    };

    let metadata = match save_file_to_disk(
        &config,
        &parsed.filename,
        parsed.content_type.as_deref(),
        &parsed.data,
        uploaded_by,
    )
    .await
    {
        Ok(m) => m,
        Err(FileUploadError::EmptyFile) => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "success": false,
                "message": "Uploaded file is empty"
            }));
        }
        Err(FileUploadError::FileTooLarge {
            size_bytes,
            max_bytes,
        }) => {
            return HttpResponse::PayloadTooLarge().json(serde_json::json!({
                "success": false,
                "message": format!("File size ({size_bytes} bytes) exceeds limit ({max_bytes} bytes)")
            }));
        }
        Err(err) => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "success": false,
                "message": format!("Failed to save uploaded file: {err}")
            }));
        }
    };

    let mut resource = metadata.to_file_resource();
    let created_id = match s_data
        .db
        .create_resource(ResourceName::FileResource, resource.clone())
        .await
    {
        Ok(id) => id,
        Err(err) => {
            let _ = tokio::fs::remove_file(&metadata.disk_path).await;
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "success": false,
                "message": format!("Failed to persist FileResource metadata: {err}")
            }));
        }
    };

    resource.id = Some(created_id);
    {
        let mut cache_w = s_data.cache.write().await;
        cache_w.write(ResourceName::FileResource, created_id, resource);
    }

    HttpResponse::Created().json(serde_json::json!({
        "success": true,
        "id": created_id,
        "originalName": metadata.original_name,
        "storedName": metadata.stored_name,
        "contentType": metadata.content_type,
        "sizeBytes": metadata.size_bytes,
        "url": metadata.url,
        "uploadedAt": metadata.uploaded_at,
        "uploadedBy": metadata.uploaded_by,
    }))
}

pub async fn serve_public_file(req: HttpRequest, path: web::Path<String>) -> HttpResponse {
    let filename = path.into_inner();
    let config = resolve_upload_config(&req);

    match read_file_from_disk(&config, &filename).await {
        Ok((bytes, content_type)) => HttpResponse::Ok()
            .content_type(content_type)
            .insert_header((
                "Content-Disposition",
                format!("inline; filename=\"{}\"", filename),
            ))
            .insert_header(("Cache-Control", "public, max-age=31536000, immutable"))
            .body(bytes),
        Err(FileUploadError::InvalidFilename(msg)) => {
            HttpResponse::BadRequest().json(serde_json::json!({
                "success": false,
                "message": format!("Invalid file name: {msg}")
            }))
        }
        Err(FileUploadError::NotFound(_)) => HttpResponse::NotFound().json(serde_json::json!({
            "success": false,
            "message": "File not found"
        })),
        Err(err) => HttpResponse::InternalServerError().json(serde_json::json!({
            "success": false,
            "message": format!("Failed to read file: {err}")
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::test_helpers::{create_mock_server_data_with_state, MockDbState};
    use crate::middleware::AuthMiddleware;
    use crate::roles::Roles;
    use actix_web::{
        http::{header::AUTHORIZATION, header::CONTENT_TYPE, StatusCode},
        test, App,
    };
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DIR_SEQ: AtomicU64 = AtomicU64::new(1);

    #[actix_web::test]
    async fn test_upload_and_public_file_display_without_auth() {
        let temp_dir = std::env::temp_dir().join(format!(
            "narayani_http_upload_test_{}_{}",
            std::process::id(),
            TEST_DIR_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let upload_cfg = FileUploadConfig::new(&temp_dir);
        let (s_data, db_state) = create_mock_server_data_with_state(MockDbState::default()).await;

        let app = test::init_service(
            App::new()
                .app_data(s_data.clone())
                .app_data(web::Data::new(upload_cfg))
                .service(
                    web::scope("/api/v1/files")
                        .wrap(AuthMiddleware::with_roles(&[
                            Roles::User,
                            Roles::Admin,
                            Roles::Operator,
                        ]))
                        .route("/upload", web::post().to(upload_file)),
                )
                .route("/files/{filename}", web::get().to(serve_public_file))
                .route("/uploads/{filename}", web::get().to(serve_public_file)),
        )
        .await;

        let token = auth::create_jwt_with_role("admin@narayanisangh.com", &Roles::Admin)
            .expect("JWT creation should succeed");

        let boundary = "----NarayaniTestBoundary123";
        let file_bytes = b"\x89PNG\r\n\x1a\ntest-image-payload";
        let mut multipart_body = Vec::new();
        multipart_body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"logo.png\"\r\nContent-Type: image/png\r\n\r\n"
            )
            .as_bytes(),
        );
        multipart_body.extend_from_slice(file_bytes);
        multipart_body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

        // 1. Upload without token is rejected (401)
        let unauth_upload = test::TestRequest::post()
            .uri("/api/v1/files/upload")
            .insert_header((
                CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            ))
            .set_payload(multipart_body.clone())
            .to_request();
        let unauth_err = test::try_call_service(&app, unauth_upload)
            .await
            .err()
            .expect("unauthenticated upload should return error");
        assert_eq!(
            unauth_err.as_response_error().status_code(),
            StatusCode::UNAUTHORIZED
        );

        // 2. Authenticated upload succeeds, writes to disk, and saves FileResource to DB & cache
        let upload_req = test::TestRequest::post()
            .uri("/api/v1/files/upload")
            .insert_header((AUTHORIZATION, format!("Bearer {token}")))
            .insert_header((
                CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            ))
            .set_payload(multipart_body)
            .to_request();
        let upload_resp = test::call_service(&app, upload_req).await;
        assert_eq!(upload_resp.status(), StatusCode::CREATED);

        let upload_json: serde_json::Value = test::read_body_json(upload_resp).await;
        assert_eq!(upload_json["success"], true);
        let file_id = upload_json["id"].as_i64().unwrap() as i32;
        let stored_name = upload_json["storedName"].as_str().unwrap().to_string();
        assert_eq!(upload_json["originalName"], "logo.png");
        assert_eq!(upload_json["contentType"], "image/png");
        assert_eq!(upload_json["url"], format!("/files/{stored_name}"));
        assert_eq!(upload_json["uploadedBy"], "admin@narayanisangh.com");

        {
            let guard = db_state.lock().await;
            let rows = guard.tables.get("fileresource").expect("fileresource table");
            assert_eq!(rows.len(), 1);
            assert_eq!(
                rows[0]
                    .inner_content
                    .as_ref()
                    .and_then(|v| v.get("storedName"))
                    .and_then(|v| v.as_str()),
                Some(stored_name.as_str())
            );
        }
        {
            let cache_r = s_data.cache.read().await;
            let cached = cache_r.read(ResourceName::FileResource, file_id);
            assert!(cached.is_some());
        }

        // 3. Public GET /files/{storedName} works WITHOUT any Authorization header/token
        let public_req = test::TestRequest::get()
            .uri(&format!("/files/{stored_name}"))
            .to_request();
        let public_resp = test::call_service(&app, public_req).await;
        assert_eq!(public_resp.status(), StatusCode::OK);
        assert_eq!(
            public_resp
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("image/png")
        );
        let returned_bytes = test::read_body(public_resp).await;
        assert_eq!(returned_bytes.as_ref(), file_bytes);

        // 4. Alias GET /uploads/{storedName} also works WITHOUT auth
        let alias_req = test::TestRequest::get()
            .uri(&format!("/uploads/{stored_name}"))
            .to_request();
        let alias_resp = test::call_service(&app, alias_req).await;
        assert_eq!(alias_resp.status(), StatusCode::OK);
        assert_eq!(test::read_body(alias_resp).await.as_ref(), file_bytes);

        // 5. Path traversal attempt is rejected (400)
        let traversal_req = test::TestRequest::get()
            .uri("/files/..%2Fsecret.txt")
            .to_request();
        let traversal_resp = test::call_service(&app, traversal_req).await;
        assert_eq!(traversal_resp.status(), StatusCode::BAD_REQUEST);

        // 6. Missing file returns 404
        let missing_req = test::TestRequest::get()
            .uri("/files/does-not-exist.png")
            .to_request();
        let missing_resp = test::call_service(&app, missing_req).await;
        assert_eq!(missing_resp.status(), StatusCode::NOT_FOUND);

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }
}
