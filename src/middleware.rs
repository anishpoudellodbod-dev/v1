use crate::{auth, roles::Roles};

use actix_web::{
    dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform},
    error::{ErrorForbidden, ErrorUnauthorized},
    http::header::AUTHORIZATION,
    Error, HttpMessage,
};

use futures_util::future::{ready, LocalBoxFuture, Ready};

/// Authentication and role-authorization middleware.
#[derive(Clone)]
pub struct AuthMiddleware {
    required_roles: Option<Vec<Roles>>,
}

impl AuthMiddleware {
    /// Requires a valid JWT, but does not require a specific role.
    pub fn new() -> Self {
        Self {
            required_roles: None,
        }
    }

    /// Requires a valid JWT and one specific role.
    pub fn with_role(role: Roles) -> Self {
        Self {
            required_roles: Some(vec![role]),
        }
    }

    /// Requires a valid JWT and at least one of the supplied roles.
    pub fn with_roles(roles: &[Roles]) -> Self {
        Self {
            required_roles: Some(roles.to_vec()),
        }
    }
}

impl Default for AuthMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

/// Service created by `AuthMiddleware`.
pub struct AuthMiddlewareService<S> {
    service: S,
    required_roles: Option<Vec<Roles>>,
}

impl<S, B> Transform<S, ServiceRequest> for AuthMiddleware
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type InitError = ();
    type Transform = AuthMiddlewareService<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(AuthMiddlewareService {
            service,
            required_roles: self.required_roles.clone(),
        }))
    }
}

impl<S, B> Service<ServiceRequest> for AuthMiddlewareService<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        /*
        * Extract the Authorization header.
        *
        * Expected format:
        *
        * Authorization: Bearer <token>
        */
        let token = match req
            .headers()
            .get(AUTHORIZATION)
            .and_then(|header| header.to_str().ok())
            .and_then(|header| header.strip_prefix("Bearer "))
            .map(str::trim)
            .filter(|token| !token.is_empty())
        {
            Some(token) => token,

            None => {
                return Box::pin(async {
                    Err(ErrorUnauthorized("Missing or invalid Authorization header"))
                });
            }
        };

        /*
        * Verify the JWT.
        *
        * This expects:
        *
        * auth::verify_jwt(token: &str) -> Result<Claims, JwtError>
        */
        let claims = match auth::verify_jwt(token) {
            Ok(claims) => claims,

            Err(error) => {
                // Log detailed errors only on the server.
                eprintln!("JWT verification failed: {error:?}");

                return Box::pin(async { Err(ErrorUnauthorized("Invalid or expired token")) });
            }
        };

        /*
        * Check role authorization.
        *
        * If required_roles is None, any authenticated user is allowed.
        * If required_roles is Some(...), the user's role must match.
        */
        if let Some(required_roles) = &self.required_roles {
            if !required_roles.contains(&claims.role) {
                return Box::pin(async { Err(ErrorForbidden("Insufficient permissions")) });
            }
        }

        // Make verified claims available to downstream handlers.
        req.extensions_mut().insert(claims);

        // Forward the authenticated request to the wrapped service.
        let future = self.service.call(req);

        Box::pin(async move {
            let response = future.await?;
            Ok(response)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{test, web, App, HttpResponse, http::StatusCode};

    async fn test_handler(req: actix_web::HttpRequest) -> HttpResponse {
        if let Some(claims) = req.extensions().get::<auth::Claims>() {
            HttpResponse::Ok().body(format!("Hello {}", claims.sub))
        } else {
            HttpResponse::Ok().body("Hello anonymous")
        }
    }

    #[actix_web::test]
    async fn test_auth_middleware_allowed_any_authenticated() {
        let app = test::init_service(
            App::new()
                .wrap(AuthMiddleware::new())
                .route("/test", web::get().to(test_handler))
        ).await;

        let token = auth::create_jwt_with_role("user@test.com", &Roles::User).expect("token creation");
        let req = test::TestRequest::get()
            .uri("/test")
            .insert_header((AUTHORIZATION, format!("Bearer {token}")))
            .to_request();

        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[actix_web::test]
    async fn test_auth_middleware_with_role_admin_success() {
        let app = test::init_service(
            App::new()
                .wrap(AuthMiddleware::with_role(Roles::Admin))
                .route("/admin", web::get().to(test_handler))
        ).await;

        let token = auth::create_jwt_with_role("admin@test.com", &Roles::Admin).expect("token creation");
        let req = test::TestRequest::get()
            .uri("/admin")
            .insert_header((AUTHORIZATION, format!("Bearer {token}")))
            .to_request();

        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[actix_web::test]
    async fn test_auth_middleware_with_role_admin_forbidden_for_user() {
        let app = test::init_service(
            App::new()
                .wrap(AuthMiddleware::with_role(Roles::Admin))
                .route("/admin", web::get().to(test_handler))
        ).await;

        let token = auth::create_jwt_with_role("user@test.com", &Roles::User).expect("token creation");
        let req = test::TestRequest::get()
            .uri("/admin")
            .insert_header((AUTHORIZATION, format!("Bearer {token}")))
            .to_request();

        let err = test::try_call_service(&app, req).await.err().expect("expected forbidden error");
        assert_eq!(err.as_response_error().status_code(), StatusCode::FORBIDDEN);
    }

    #[actix_web::test]
    async fn test_auth_middleware_with_roles_operator_and_admin() {
        let app = test::init_service(
            App::new()
                .wrap(AuthMiddleware::with_roles(&[Roles::Admin, Roles::Operator]))
                .route("/staff", web::get().to(test_handler))
        ).await;

        // Operator allowed
        let op_token = auth::create_jwt_with_role("operator@test.com", &Roles::Operator).unwrap();
        let req_op = test::TestRequest::get()
            .uri("/staff")
            .insert_header((AUTHORIZATION, format!("Bearer {op_token}")))
            .to_request();
        assert_eq!(test::call_service(&app, req_op).await.status(), StatusCode::OK);

        // Regular user forbidden
        let user_token = auth::create_jwt_with_role("user@test.com", &Roles::User).unwrap();
        let req_user = test::TestRequest::get()
            .uri("/staff")
            .insert_header((AUTHORIZATION, format!("Bearer {user_token}")))
            .to_request();
        let err = test::try_call_service(&app, req_user).await.err().expect("expected forbidden error");
        assert_eq!(err.as_response_error().status_code(), StatusCode::FORBIDDEN);
    }

    #[actix_web::test]
    async fn test_auth_middleware_missing_header() {
        let app = test::init_service(
            App::new()
                .wrap(AuthMiddleware::new())
                .route("/test", web::get().to(test_handler))
        ).await;

        let req = test::TestRequest::get().uri("/test").to_request();
        let err = test::try_call_service(&app, req).await.err().expect("expected unauthorized error");
        assert_eq!(err.as_response_error().status_code(), StatusCode::UNAUTHORIZED);
    }

    #[actix_web::test]
    async fn test_auth_middleware_invalid_prefix() {
        let app = test::init_service(
            App::new()
                .wrap(AuthMiddleware::new())
                .route("/test", web::get().to(test_handler))
        ).await;

        let req = test::TestRequest::get()
            .uri("/test")
            .insert_header((AUTHORIZATION, "Basic 12345"))
            .to_request();
        let err = test::try_call_service(&app, req).await.err().expect("expected unauthorized error");
        assert_eq!(err.as_response_error().status_code(), StatusCode::UNAUTHORIZED);
    }

    #[actix_web::test]
    async fn test_auth_middleware_empty_token() {
        let app = test::init_service(
            App::new()
                .wrap(AuthMiddleware::new())
                .route("/test", web::get().to(test_handler))
        ).await;

        let req = test::TestRequest::get()
            .uri("/test")
            .insert_header((AUTHORIZATION, "Bearer    "))
            .to_request();
        let err = test::try_call_service(&app, req).await.err().expect("expected unauthorized error");
        assert_eq!(err.as_response_error().status_code(), StatusCode::UNAUTHORIZED);
    }

    #[actix_web::test]
    async fn test_auth_middleware_corrupted_token() {
        let app = test::init_service(
            App::new()
                .wrap(AuthMiddleware::new())
                .route("/test", web::get().to(test_handler))
        ).await;

        let req = test::TestRequest::get()
            .uri("/test")
            .insert_header((AUTHORIZATION, "Bearer invalid.jwt.string"))
            .to_request();
        let err = test::try_call_service(&app, req).await.err().expect("expected unauthorized error");
        assert_eq!(err.as_response_error().status_code(), StatusCode::UNAUTHORIZED);
    }

    #[actix_web::test]
    async fn test_auth_middleware_extensions_claims_injected() {
        let app = test::init_service(
            App::new()
                .wrap(AuthMiddleware::new())
                .route("/test", web::get().to(test_handler))
        ).await;

        let token = auth::create_jwt_with_role("special_user@test.com", &Roles::User).unwrap();
        let req = test::TestRequest::get()
            .uri("/test")
            .insert_header((AUTHORIZATION, format!("Bearer {token}")))
            .to_request();

        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(resp).await;
        assert_eq!(body, "Hello special_user@test.com");
    }
}
