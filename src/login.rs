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

    let credentials = credentials.into_inner();
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

            return HttpResponse::InternalServerError().json(serde_json::json!({
                "success": false,
                "message": "Database error"
            }));
        }
    };
    
    
    let user = match all_users.iter().find(|user| {
        user.inner_content
            .as_ref()
            .and_then(|content| content.get("email").and_then(|email| email.as_str().map(|s| s.to_lowercase())))
            == Some(credentials.email.as_str().to_lowercase())
    }) {
        Some(user) => user,
        None => {
            return HttpResponse::NotFound().json(serde_json::json!({
                "success": false,
                "message": "User not found in all users"
            }));
        }
    };


    let user_resource = match &user.inner_content {
        Some(inner_content) => inner_content,
        None => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "success": false,
                "message": "User resource is missing inner content"
            }));
        }
    };

    let supplied_password = match credentials.password.as_deref() {
        Some(password) if !password.is_empty() => password,
        _ => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "success": false,
                "message": "Password is required"
            }));
        }
    };

    let stored_password = match user_resource
        .get("password")
        .and_then(serde_json::Value::as_str)
    {
        Some(password) => password,
        None => {
            return HttpResponse::Unauthorized().json(serde_json::json!({
                "success": false,
                "message": "Invalid credentials"
            }));
        }
    };

    if supplied_password != stored_password {
        return HttpResponse::Unauthorized().json(serde_json::json!({
            "success": false,
            "message": "Invalid credentials mismatch"
        }));
    }

    let mut response_data = match user_resource.as_object() {
        Some(object) => object.clone(),
        None => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "success": false,
                "message": "Invalid user data"
            }));
        }
    };

    // Remove the password from the response data to avoid sending it back to the client
    response_data.remove("password");

    let role_value = match response_data.get("role") {
        Some(role) => role.clone(),
        None => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "success": false,
                "message": "User role is missing"
            }));
        }
    };

    let role: roles::Roles = match serde_json::from_value(role_value) {
        Ok(role) => role,
        Err(error) => {
            eprintln!("Invalid user role: {:?}", error);

            return HttpResponse::InternalServerError().json(serde_json::json!({
                "success": false,
                "message": "Invalid user role"
            }));
        }
    };

    let token = match auth::create_jwt_with_role(&credentials.email, &role) {
        Ok(token) => token,
        Err(error) => {
            eprintln!("Failed to create RS512 JWT: {:?}", error);

            return HttpResponse::InternalServerError().json(serde_json::json!({
                "success": false,
                "message": "Failed to create JWT"
            }));
        }
    };

    response_data.insert(
        "token".to_owned(),
        serde_json::Value::String(token),
    );

    HttpResponse::Ok().json(response_data)
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
}


