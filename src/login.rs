use crate::{auth, roles, ServerData};
use actix_web::{web, HttpResponse};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize, Debug, Clone)]
pub struct LoginCredentials {
    pub email: String,
    pub password: Option<String>,
}

pub async fn login(
    credentials: web::Json<LoginCredentials>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let started_at = std::time::Instant::now();
    let credentials = credentials.into_inner();
    let email_for_log = credentials.email.clone();
    let (response, user_id, role, details) = login_inner(credentials, s_data).await;
    let status_code = response.status().as_u16();

    crate::log::log_login_async(
        email_for_log.clone(),
        user_id,
        role.clone(),
        status_code,
        started_at,
        details.clone(),
    );

    crate::audit::record_http_audit_async(crate::audit::HttpAuditEvent {
        req: None,
        category: crate::audit::AuditCategory::Auth,
        action: crate::audit::AuditAction::Login,
        status_code,
        started_at: Some(started_at),
        resource_name: Some("UserResource".to_string()),
        resource_id: user_id.map(|id| id.to_string()),
        summary: details.unwrap_or_else(|| format!("Login attempt for {}", email_for_log)),
        actor_email_override: Some(email_for_log),
        actor_user_id_override: user_id,
        actor_role_override: role,
        method_override: Some("POST"),
        path_override: Some("/api/v1/auth/login"),
        ..Default::default()
    });

    response
}

async fn login_inner(
    credentials: LoginCredentials,
    s_data: web::Data<Arc<ServerData>>,
) -> (HttpResponse, Option<i32>, Option<String>, Option<String>) {
    //remove password from the credentials for logging purposes
    let mut credentials_for_logging = credentials.clone();
    credentials_for_logging.password = None;

    let all_users = match s_data
        .db
        .read_all_resource(crate::resource::ResourceName::UserResource)
        .await
    {
        Ok(users) => users,
        Err(error) => {
            eprintln!("Error reading all users: {:?}", error);

            return (
                HttpResponse::InternalServerError().json(serde_json::json!({
                    "success": false,
                    "message": "Database error"
                })),
                None,
                None,
                Some("Database error reading users".to_string()),
            );
        }
    };

    let user = match all_users.iter().find(|user| {
        user.inner_content
            .as_ref()
            .and_then(|content| {
                content
                    .get("email")
                    .and_then(|email| email.as_str().map(|s| s.to_lowercase()))
            })
            == Some(credentials.email.as_str().to_lowercase())
    }) {
        Some(user) => user,
        None => {
            return (
                HttpResponse::NotFound().json(serde_json::json!({
                    "success": false,
                    "message": "User not found in all users"
                })),
                None,
                None,
                Some("User not found".to_string()),
            );
        }
    };

    let found_user_id = user.id;

    let user_resource = match &user.inner_content {
        Some(inner_content) => inner_content,
        None => {
            return (
                HttpResponse::InternalServerError().json(serde_json::json!({
                    "success": false,
                    "message": "User resource is missing inner content"
                })),
                found_user_id,
                None,
                Some("User resource missing inner content".to_string()),
            );
        }
    };

    let supplied_password = match credentials.password.as_deref() {
        Some(password) if !password.is_empty() => password,
        _ => {
            return (
                HttpResponse::BadRequest().json(serde_json::json!({
                    "success": false,
                    "message": "Password is required"
                })),
                found_user_id,
                None,
                Some("Password missing".to_string()),
            );
        }
    };

    let stored_password = match user_resource
        .get("password")
        .and_then(serde_json::Value::as_str)
    {
        Some(password) => password,
        None => {
            return (
                HttpResponse::Unauthorized().json(serde_json::json!({
                    "success": false,
                    "message": "Invalid credentials"
                })),
                found_user_id,
                None,
                Some("Invalid credentials (no stored password)".to_string()),
            );
        }
    };

    if supplied_password != stored_password {
        return (
            HttpResponse::Unauthorized().json(serde_json::json!({
                "success": false,
                "message": "Invalid credentials mismatch"
            })),
            found_user_id,
            None,
            Some("Invalid credentials mismatch".to_string()),
        );
    }

    let mut response_data = match user_resource.as_object() {
        Some(object) => object.clone(),
        None => {
            return (
                HttpResponse::InternalServerError().json(serde_json::json!({
                    "success": false,
                    "message": "Invalid user data"
                })),
                found_user_id,
                None,
                Some("Invalid user data".to_string()),
            );
        }
    };

    // Remove the password from the response data to avoid sending it back to the client
    response_data.remove("password");

    let raw_role_str = response_data
        .get("role")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);

    let Some(raw_role_name) = raw_role_str else {
        return (
            HttpResponse::InternalServerError().json(serde_json::json!({
                "success": false,
                "message": "User role is missing"
            })),
            found_user_id,
            None,
            Some("User role missing".to_string()),
        );
    };

    let (role, is_builtin_legacy_role) = if raw_role_name == "Admin" {
        (roles::Roles::Admin, true)
    } else if raw_role_name == "Operator" {
        (roles::Roles::Operator, true)
    } else if raw_role_name == "User" {
        (roles::Roles::User, true)
    } else if crate::permissions::is_super_admin_role(&raw_role_name) {
        (roles::Roles::Admin, false)
    } else {
        // Custom v1.0 role names (e.g. Cashier, Viewer) map to base Roles::User for route scopes
        // while preserving exact role name in Claims.role_name.
        (roles::Roles::User, false)
    };

    let role_str = raw_role_name.clone();

    let Some(user_id) = user.id else {
        return (
            HttpResponse::InternalServerError().json(serde_json::json!({
                "success": false,
                "message": "User resource is missing id"
            })),
            None,
            Some(role_str),
            Some("User resource missing id".to_string()),
        );
    };

    let raw_tenant = user_resource
        .get("tenantId")
        .or_else(|| user_resource.get("tenant_id"))
        .and_then(serde_json::Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| crate::access::extract_sangh_branch(user_resource));
    let setting_branch = if raw_tenant.is_none() {
        crate::access::get_setting_sangh_branch(&s_data).await
    } else {
        None
    };
    let tenant_id = raw_tenant
        .or(setting_branch)
        .map(|t| crate::permissions::normalize_tenant_id(&t))
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "default".to_string());

    let is_super_admin = crate::permissions::is_super_admin_role(&raw_role_name);
    if is_super_admin {
        response_data.insert("isSuperAdmin".to_owned(), serde_json::json!(true));
    } else {
        let cached_policy = {
            let cache = s_data.cache.read().await;
            cache
                .get_role_policy(&tenant_id, &raw_role_name)
                .cloned()
        };
        let role_policy = match cached_policy {
            Some(p) => Some(p),
            None => match crate::permissions::find_role_policy_in_db(
                &s_data.db,
                &tenant_id,
                &raw_role_name,
            )
            .await
            {
                Ok(Some(p)) => {
                    s_data.cache.write().await.upsert_role_policy(p.clone());
                    Some(p)
                }
                _ => None,
            },
        };

        if let Some(policy) = role_policy {
            if let Ok(pol_json) = serde_json::to_value(&policy) {
                response_data.insert("permissionPolicy".to_owned(), pol_json);
            }
            if let Ok(pr_json) = serde_json::to_value(policy.to_database_resource()) {
                response_data.insert("permissionResource".to_owned(), pr_json);
            }
        } else if is_builtin_legacy_role {
            let permission_resource =
                match crate::access::load_permission_on_login(&s_data, user, &role).await {
                    Ok(perm) => perm,
                    Err(error) => {
                        eprintln!("Failed to load PermissionResource on login: {}", error);
                        return (
                            HttpResponse::InternalServerError().json(serde_json::json!({
                                "success": false,
                                "message": "Failed to load user permissions"
                            })),
                            Some(user_id),
                            Some(role_str),
                            Some("Failed to load user permissions".to_string()),
                        );
                    }
                };
            if let Ok(perm_json) = serde_json::to_value(&permission_resource) {
                response_data.insert("permissionResource".to_owned(), perm_json);
            }
        }
    }

    let token = match auth::create_jwt_with_context(
        &credentials.email,
        &role,
        Some(user_id),
        Some(raw_role_name),
        Some(tenant_id.clone()),
    ) {
        Ok(token) => token,
        Err(error) => {
            eprintln!("Failed to create RS512 JWT: {:?}", error);

            return (
                HttpResponse::InternalServerError().json(serde_json::json!({
                    "success": false,
                    "message": "Failed to create JWT"
                })),
                Some(user_id),
                Some(role_str),
                Some("Failed to create JWT".to_string()),
            );
        }
    };

    response_data
        .entry("id".to_owned())
        .or_insert_with(|| serde_json::json!(user_id));
    response_data
        .entry("tenantId".to_owned())
        .or_insert_with(|| serde_json::json!(tenant_id));
    response_data.insert(
        "token".to_owned(),
        serde_json::Value::String(token),
    );

    (
        HttpResponse::Ok().json(response_data),
        Some(user_id),
        Some(role_str),
        Some("Login succeeded".to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_login_credentials_deserialization_with_password() {
        let json = r#"{"email": "admin@example.com", "password": "password123"}"#;
        let creds: LoginCredentials = serde_json::from_str(json).expect("Deserialization failed");
        assert_eq!(creds.email, "admin@example.com");
        assert_eq!(creds.password, Some("password123".to_string()));
    }

    #[test]
    fn test_login_credentials_deserialization_without_password() {
        let json = r#"{"email": "admin@example.com"}"#;
        let creds: LoginCredentials = serde_json::from_str(json).expect("Deserialization failed");
        assert_eq!(creds.email, "admin@example.com");
        assert_eq!(creds.password, None);
    }

    #[test]
    fn test_login_credentials_deserialization_null_password() {
        let json = r#"{"email": "admin@example.com", "password": null}"#;
        let creds: LoginCredentials = serde_json::from_str(json).expect("Deserialization failed");
        assert_eq!(creds.email, "admin@example.com");
        assert_eq!(creds.password, None);
    }

    #[test]
    fn test_login_credentials_clone() {
        let creds = LoginCredentials {
            email: "user@example.com".to_string(),
            password: Some("secret".to_string()),
        };
        let creds_clone = creds.clone();
        assert_eq!(creds.email, creds_clone.email);
        assert_eq!(creds.password, creds_clone.password);
    }

    #[actix_web::test]
    async fn test_login_emits_async_audit_logs_for_success_and_failure() {
        let mut state = crate::access::test_helpers::MockDbState::default();
        let unique_email = format!("login-audit-{}@test.com", crate::log::generate_action_log_id());
        let secret_password = "LoginSecretPass!888";
        state.seed(
            crate::resource::ResourceName::UserResource,
            crate::resource::DatabaseResource {
                id: Some(715),
                resource_name: Some(crate::resource::ResourceName::UserResource),
                inner_content: Some(serde_json::json!({
                    "email": unique_email,
                    "password": secret_password,
                    "role": "Operator",
                    "SanghBranch": "Loadboard sthaniya"
                })),
                external_content: None,
            },
        );
        let (s_data, _) =
            crate::access::test_helpers::create_mock_server_data_with_state(state).await;

        let missing_email = format!("missing-{}@test.com", crate::log::generate_action_log_id());
        let resp_missing = login(
            web::Json(LoginCredentials {
                email: missing_email.clone(),
                password: Some("whatever".to_string()),
            }),
            s_data.clone(),
        )
        .await;
        assert_eq!(resp_missing.status(), actix_web::http::StatusCode::NOT_FOUND);

        let resp_bad_pw = login(
            web::Json(LoginCredentials {
                email: unique_email.clone(),
                password: Some("WrongPassword".to_string()),
            }),
            s_data.clone(),
        )
        .await;
        assert_eq!(resp_bad_pw.status(), actix_web::http::StatusCode::UNAUTHORIZED);

        let resp_ok = login(
            web::Json(LoginCredentials {
                email: unique_email.clone(),
                password: Some(secret_password.to_string()),
            }),
            s_data.clone(),
        )
        .await;
        assert_eq!(resp_ok.status(), actix_web::http::StatusCode::OK);

        crate::log::flush_action_logs().await;

        let all = crate::log::get_action_logs_by_action(crate::log::ResourceActionKind::Login);

        let missing_log = all
            .iter()
            .find(|e| e.performed_by.as_deref() == Some(missing_email.as_str()))
            .expect("missing user login attempt should be logged");
        assert_eq!(missing_log.status_code, 404);
        assert!(!missing_log.success);

        let bad_pw_log = all
            .iter()
            .find(|e| {
                e.performed_by.as_deref() == Some(unique_email.as_str()) && e.status_code == 401
            })
            .expect("wrong password login attempt should be logged");
        assert!(!bad_pw_log.success);
        assert_eq!(bad_pw_log.user_id, Some(715));

        let ok_log = all
            .iter()
            .find(|e| {
                e.performed_by.as_deref() == Some(unique_email.as_str()) && e.status_code == 200
            })
            .expect("successful login should be logged");
        assert!(ok_log.success);
        assert_eq!(ok_log.user_id, Some(715));
        assert_eq!(ok_log.role.as_deref(), Some("Operator"));

        for entry in [missing_log, bad_pw_log, ok_log] {
            let serialized = serde_json::to_string(entry).unwrap();
            assert!(!serialized.contains(secret_password));
        }
    }

    #[actix_web::test]
    async fn test_login_supports_custom_cashier_and_super_admin_roles_with_tenant_policy() {
        let mut state = crate::access::test_helpers::MockDbState::default();
        state.seed(
            crate::resource::ResourceName::SettingResource,
            crate::resource::DatabaseResource {
                id: Some(1),
                resource_name: Some(crate::resource::ResourceName::SettingResource),
                inner_content: Some(serde_json::json!({
                    "integrations": { "tqms": { "SanghBranch": "Loadboard sthaniya" } }
                })),
                external_content: None,
            },
        );
        state.seed(
            crate::resource::ResourceName::UserResource,
            crate::resource::DatabaseResource {
                id: Some(901),
                resource_name: Some(crate::resource::ResourceName::UserResource),
                inner_content: Some(serde_json::json!({
                    "email": "cashier901@test.com",
                    "password": "pw",
                    "role": "Cashier",
                    "SanghBranch": "Loadboard sthaniya"
                })),
                external_content: None,
            },
        );
        state.seed(
            crate::resource::ResourceName::UserResource,
            crate::resource::DatabaseResource {
                id: Some(902),
                resource_name: Some(crate::resource::ResourceName::UserResource),
                inner_content: Some(serde_json::json!({
                    "email": "root902@platform.com",
                    "password": "pw",
                    "role": "SuperAdmin"
                })),
                external_content: None,
            },
        );
        let (s_data, _) =
            crate::access::test_helpers::create_mock_server_data_with_state(state).await;
        {
            let mut cache = s_data.cache.write().await;
            cache.load_from_db(&s_data.db).await;
        }

        crate::permissions::create_role_policy_in_db(
            &s_data,
            crate::permissions::default_cashier_policy("loadboard-sthaniya", vec![1, 2], Some(1)),
            None,
        )
        .await
        .unwrap();

        let resp_cashier = login(
            web::Json(LoginCredentials {
                email: "cashier901@test.com".to_string(),
                password: Some("pw".to_string()),
            }),
            s_data.clone(),
        )
        .await;
        assert_eq!(resp_cashier.status(), actix_web::http::StatusCode::OK);
        let body_cashier: serde_json::Value = serde_json::from_slice(
            &actix_web::body::to_bytes(resp_cashier.into_body())
                .await
                .unwrap(),
        )
        .unwrap();
        let token_cashier = body_cashier["token"].as_str().unwrap();
        let claims_cashier = auth::verify_jwt(token_cashier).unwrap();
        assert_eq!(claims_cashier.effective_role_name(), "Cashier");
        assert_eq!(
            claims_cashier.tenant_id.as_deref(),
            Some("loadboard-sthaniya")
        );
        assert_eq!(
            body_cashier["permissionPolicy"]["inner_content"]["role"],
            "Cashier"
        );

        let resp_super = login(
            web::Json(LoginCredentials {
                email: "root902@platform.com".to_string(),
                password: Some("pw".to_string()),
            }),
            s_data,
        )
        .await;
        assert_eq!(resp_super.status(), actix_web::http::StatusCode::OK);
        let body_super: serde_json::Value = serde_json::from_slice(
            &actix_web::body::to_bytes(resp_super.into_body())
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(body_super["isSuperAdmin"], true);
        let claims_super = auth::verify_jwt(body_super["token"].as_str().unwrap()).unwrap();
        assert_eq!(claims_super.effective_role_name(), "SuperAdmin");
    }
}


