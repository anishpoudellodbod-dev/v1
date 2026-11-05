use crate::roles::Roles;
use chrono::{Duration, Utc};
use jsonwebtoken::{
    decode, decode_header, encode, errors::Error as JwtError, Algorithm, DecodingKey, EncodingKey,
    Header, Validation,
};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String, // Subject (user id)
    pub exp: i64,    // Expiration time
    pub iat: i64,    // Issued at
    pub role: Roles, // User role (admin or user)
}

static ENCODING_KEY: OnceLock<EncodingKey> = OnceLock::new();
static DECODING_KEY: OnceLock<DecodingKey> = OnceLock::new();

fn get_encoding_key() -> &'static EncodingKey {
    ENCODING_KEY.get_or_init(|| {
        let private_key = std::fs::read("private.pem").expect("Failed to read RSA private key");
        EncodingKey::from_rsa_pem(&private_key).expect("Invalid RSA private.pem format")
    })
}

fn get_decoding_key() -> &'static DecodingKey {
    DECODING_KEY.get_or_init(|| {
        let public_key = std::fs::read("public.pem").expect("Failed to read RSA public key");
        DecodingKey::from_rsa_pem(&public_key).expect("Invalid RSA public.pem format")
    })
}

pub fn create_jwt_with_role(user_id: &str, role: &Roles) -> Result<String, JwtError> {
    let now = Utc::now();
    let expiration = now
        .checked_add_signed(Duration::hours(24))
        .expect("valid timestamp")
        .timestamp();

    let claims = Claims {
        sub: user_id.to_owned(),
        exp: expiration,
        iat: now.timestamp(),
        role: role.to_owned(),
    };

    encode(&Header::new(Algorithm::RS512), &claims, get_encoding_key())
}

pub fn verify_jwt(token: &str) -> Result<Claims, JwtError> {
    // Optional diagnostic: log the exact algorithm if verification fails
    if let Ok(header) = decode_header(token) {
        if !matches!(
            header.alg,
            Algorithm::RS256 | Algorithm::RS384 | Algorithm::RS512
        ) {
            eprintln!(
                "Warning: Token header uses non-RSA algorithm: {:?} (did the client send an old HS256 token?)",
                header.alg
            );
        }
    }

    let mut validation = Validation::new(Algorithm::RS512);
    // Allow all standard RSA PKCS#1 v1.5 algorithms verified against public.pem
    validation.algorithms = vec![Algorithm::RS512, Algorithm::RS384, Algorithm::RS256];

    let token_data = decode::<Claims>(token, get_decoding_key(), &validation)?;

    Ok(token_data.claims)
}

#[derive(Deserialize, Debug)]
pub struct LoginCredentials {
    pub email: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub email: String,
    pub role: String,
    pub id: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_verify_jwt_user_role() {
        let token = create_jwt_with_role("user@example.com", &Roles::User).expect("Token creation failed");
        let claims = verify_jwt(&token).expect("Token verification failed");

        assert_eq!(claims.sub, "user@example.com");
        assert_eq!(claims.role, Roles::User);
        assert!(claims.exp > claims.iat);
    }

    #[test]
    fn test_create_and_verify_jwt_admin_role() {
        let token = create_jwt_with_role("admin@example.com", &Roles::Admin).expect("Token creation failed");
        let claims = verify_jwt(&token).expect("Token verification failed");

        assert_eq!(claims.sub, "admin@example.com");
        assert_eq!(claims.role, Roles::Admin);
    }

    #[test]
    fn test_create_and_verify_jwt_operator_role() {
        let token = create_jwt_with_role("operator@example.com", &Roles::Operator).expect("Token creation failed");
        let claims = verify_jwt(&token).expect("Token verification failed");

        assert_eq!(claims.sub, "operator@example.com");
        assert_eq!(claims.role, Roles::Operator);
    }

    #[test]
    fn test_verify_jwt_tampered_token_fails() {
        let token = create_jwt_with_role("user@example.com", &Roles::User).expect("Token creation failed");
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);

        // Tamper with payload
        let tampered_token = format!("{}.e30.{}", parts[0], parts[2]);
        assert!(verify_jwt(&tampered_token).is_err());
    }

    #[test]
    fn test_verify_jwt_invalid_garbage_fails() {
        assert!(verify_jwt("not-a-token").is_err());
        assert!(verify_jwt("a.b.c").is_err());
        assert!(verify_jwt("").is_err());
    }

    #[test]
    fn test_verify_jwt_expired_token_fails() {
        let past = Utc::now() - Duration::hours(2);
        let expired_claims = Claims {
            sub: "expired@example.com".to_string(),
            exp: (past + Duration::hours(1)).timestamp(),
            iat: past.timestamp(),
            role: Roles::User,
        };

        let token = encode(&Header::new(Algorithm::RS512), &expired_claims, get_encoding_key())
            .expect("Token encoding failed");
        assert!(verify_jwt(&token).is_err());
    }

    #[test]
    fn test_login_credentials_deserialization() {
        let json = r#"{"email": "admin@test.com", "password": "securepassword"}"#;
        let creds: LoginCredentials = serde_json::from_str(json).expect("Deserialization failed");
        assert_eq!(creds.email, "admin@test.com");
        assert_eq!(creds.password, "securepassword");
    }

    #[test]
    fn test_login_response_serialization() {
        let resp = LoginResponse {
            token: "jwt.token.here".to_string(),
            email: "test@narayani.com".to_string(),
            role: "Admin".to_string(),
            id: 10,
        };
        let serialized = serde_json::to_string(&resp).expect("Serialization failed");
        assert!(serialized.contains("jwt.token.here"));
        assert!(serialized.contains("test@narayani.com"));
        assert!(serialized.contains("Admin"));
        assert!(serialized.contains("10"));
    }
}
