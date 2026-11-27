use chrono::{Duration, Utc};
use jsonwebtoken::{
    decode, decode_header, encode, errors::Error as JwtError, Algorithm, DecodingKey, EncodingKey,
    Header, Validation,
};
use narayani_core::Roles;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};

static JTI_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String, // Subject (user id)
    pub exp: i64,    // Expiration time
    pub iat: i64,    // Issued at
    pub role: Roles, // User role (admin or user)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jti: Option<String>,
    /// Preserves dynamic/custom role names (e.g. "Cashier", "Viewer", "SuperAdmin") alongside base `role`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<String>,
}

impl Claims {
    pub fn effective_role_name(&self) -> String {
        self.role_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .unwrap_or_else(|| String::from(self.role.clone()))
    }
}

/// TTL-pruned revoked token store mapping revoked JWT strings (or `jti`s) to their unix `exp`.
#[derive(Debug, Default)]
pub struct RevokedTokenStore {
    entries: RwLock<HashMap<Box<str>, i64>>,
}

impl RevokedTokenStore {
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
        }
    }

    pub fn revoke_with_exp(&self, token: &str, exp: Option<i64>) {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            return;
        }
        let now = Utc::now().timestamp();
        let fallback_exp = now + 86_400;
        let expiry = exp.unwrap_or(fallback_exp);
        let mut guard = self.entries.write().unwrap_or_else(|e| e.into_inner());
        guard.retain(|_, v| *v > now);
        guard.insert(trimmed.into(), expiry);
    }

    pub fn revoke(&self, token: &str) {
        let exp = decode_unverified_exp(token);
        self.revoke_with_exp(token, exp);
    }

    pub fn is_revoked(&self, token: &str) -> bool {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            return false;
        }
        let now = Utc::now().timestamp();
        let guard = self.entries.read().unwrap_or_else(|e| e.into_inner());
        guard.get(trimmed).is_some_and(|exp| *exp > now)
    }

    pub fn len(&self) -> usize {
        self.entries.read().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Deserialize)]
struct ExpOnly {
    exp: i64,
}

fn decode_unverified_exp(token: &str) -> Option<i64> {
    let payload_b64 = token.trim().split('.').nth(1)?;
    let bytes = jsonwebtoken::decode_header(token).ok().and_then(|_| {
        let mut s = payload_b64.replace('-', "+").replace('_', "/");
        while s.len() % 4 != 0 {
            s.push('=');
        }
        base64_decode_simple(&s)
    })?;
    serde_json::from_slice::<ExpOnly>(&bytes).ok().map(|e| e.exp)
}

fn base64_decode_simple(input: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut rev = [255u8; 256];
    for (idx, &ch) in TABLE.iter().enumerate() {
        rev[ch as usize] = idx as u8;
    }
    let bytes = input.as_bytes();
    if bytes.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity((bytes.len() / 4) * 3);
    for chunk in bytes.chunks_exact(4) {
        let a = rev[chunk[0] as usize];
        let b = rev[chunk[1] as usize];
        if a == 255 || b == 255 {
            return None;
        }
        let c = if chunk[2] == b'=' { 0 } else { rev[chunk[2] as usize] };
        let d = if chunk[3] == b'=' { 0 } else { rev[chunk[3] as usize] };
        if (chunk[2] != b'=' && c == 255) || (chunk[3] != b'=' && d == 255) {
            return None;
        }
        out.push((a << 2) | (b >> 4));
        if chunk[2] != b'=' {
            out.push((b << 4) | (c >> 2));
        }
        if chunk[3] != b'=' {
            out.push((c << 6) | d);
        }
    }
    Some(out)
}

fn read_pem_file(file_name: &str) -> Vec<u8> {
    let candidates = [
        file_name.to_string(),
        format!("../../{file_name}"),
        format!("{}/../../{file_name}", env!("CARGO_MANIFEST_DIR")),
    ];
    for path in &candidates {
        if let Ok(bytes) = std::fs::read(path) {
            return bytes;
        }
    }
    panic!("Failed to read RSA key {file_name} from {candidates:?}");
}

static ENCODING_KEY: OnceLock<EncodingKey> = OnceLock::new();
static DECODING_KEY: OnceLock<DecodingKey> = OnceLock::new();

fn get_encoding_key() -> &'static EncodingKey {
    ENCODING_KEY.get_or_init(|| {
        let private_key = read_pem_file("private.pem");
        EncodingKey::from_rsa_pem(&private_key).expect("Invalid RSA private.pem format")
    })
}

fn get_decoding_key() -> &'static DecodingKey {
    DECODING_KEY.get_or_init(|| {
        let public_key = read_pem_file("public.pem");
        DecodingKey::from_rsa_pem(&public_key).expect("Invalid RSA public.pem format")
    })
}

pub fn create_jwt_with_context(
    user_email: &str,
    role: &Roles,
    user_id: Option<i32>,
    role_name: Option<String>,
    tenant_id: Option<String>,
) -> Result<String, JwtError> {
    let now = Utc::now();
    let expiration = now
        .checked_add_signed(Duration::hours(24))
        .expect("valid timestamp")
        .timestamp();
    let jti_seq = JTI_COUNTER.fetch_add(1, Ordering::Relaxed);
    let jti = format!("{}-{}", now.timestamp_nanos_opt().unwrap_or(0), jti_seq);

    let claims = Claims {
        sub: user_email.to_owned(),
        exp: expiration,
        iat: now.timestamp(),
        role: role.to_owned(),
        user_id,
        jti: Some(jti),
        role_name,
        tenant_id,
    };

    encode(&Header::new(Algorithm::RS512), &claims, get_encoding_key())
}

#[allow(dead_code)]
pub fn create_jwt_with_role_and_user_id(
    user_email: &str,
    role: &Roles,
    user_id: Option<i32>,
) -> Result<String, JwtError> {
    create_jwt_with_context(user_email, role, user_id, None, None)
}

#[allow(dead_code)]
pub fn create_jwt_with_role(user_id: &str, role: &Roles) -> Result<String, JwtError> {
    create_jwt_with_role_and_user_id(user_id, role, None)
}

pub fn verify_jwt(token: &str) -> Result<Claims, JwtError> {
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
    validation.algorithms = vec![Algorithm::RS512, Algorithm::RS384, Algorithm::RS256];

    let token_data = decode::<Claims>(token, get_decoding_key(), &validation)?;

    Ok(token_data.claims)
}

#[allow(dead_code)]
#[derive(Deserialize, Debug)]
pub struct LoginCredentials {
    pub email: String,
    pub password: String,
}

#[allow(dead_code)]
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
        let token =
            create_jwt_with_role("user@example.com", &Roles::User).expect("Token creation failed");
        let claims = verify_jwt(&token).expect("Token verification failed");

        assert_eq!(claims.sub, "user@example.com");
        assert_eq!(claims.role, Roles::User);
        assert!(claims.exp > claims.iat);
    }

    #[test]
    fn test_create_and_verify_jwt_admin_role() {
        let token =
            create_jwt_with_role("admin@example.com", &Roles::Admin).expect("Token creation failed");
        let claims = verify_jwt(&token).expect("Token verification failed");

        assert_eq!(claims.sub, "admin@example.com");
        assert_eq!(claims.role, Roles::Admin);
    }

    #[test]
    fn test_create_and_verify_jwt_operator_role() {
        let token = create_jwt_with_role("operator@example.com", &Roles::Operator)
            .expect("Token creation failed");
        let claims = verify_jwt(&token).expect("Token verification failed");

        assert_eq!(claims.sub, "operator@example.com");
        assert_eq!(claims.role, Roles::Operator);
    }

    #[test]
    fn test_verify_jwt_tampered_token_fails() {
        let token =
            create_jwt_with_role("user@example.com", &Roles::User).expect("Token creation failed");
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);

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
            user_id: None,
            jti: None,
            role_name: None,
            tenant_id: None,
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

    #[test]
    fn test_create_jwt_with_role_and_user_id_preserves_user_id_and_unique_jti() {
        let token_1 = create_jwt_with_role_and_user_id("uid@example.com", &Roles::Operator, Some(77))
            .expect("Token 1 creation failed");
        let token_2 = create_jwt_with_role_and_user_id("uid@example.com", &Roles::Operator, Some(77))
            .expect("Token 2 creation failed");

        assert_ne!(token_1, token_2);

        let claims_1 = verify_jwt(&token_1).expect("Token 1 verification failed");
        let claims_2 = verify_jwt(&token_2).expect("Token 2 verification failed");

        assert_eq!(claims_1.sub, "uid@example.com");
        assert_eq!(claims_1.role, Roles::Operator);
        assert_eq!(claims_1.user_id, Some(77));
        assert!(claims_1.jti.is_some());
        assert_ne!(claims_1.jti, claims_2.jti);
    }

    #[test]
    fn revoked_token_store_prunes_expired_entries() {
        let store = RevokedTokenStore::new();
        store.revoke_with_exp("old-tok", Some(1));
        assert!(!store.is_revoked("old-tok"));
        let future = Utc::now().timestamp() + 3600;
        store.revoke_with_exp("fresh-tok", Some(future));
        assert!(store.is_revoked("fresh-tok"));
        assert_eq!(store.len(), 1);
    }
}
