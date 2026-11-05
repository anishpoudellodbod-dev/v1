use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Roles {
    User,
    Admin,
    Operator,
}

impl From<std::string::String> for Roles {
    fn from(s: std::string::String) -> Self {
        match s.as_str() {
            "User" => Roles::User,
            "Admin" => Roles::Admin,
            "Operator" => Roles::Operator,
            _ => panic!("Unknown role: {}", s),
        }
    }
}

impl From<Roles> for std::string::String {
    fn from(role: Roles) -> Self {
        match role {
            Roles::User => "User".to_string(),
            Roles::Admin => "Admin".to_string(),
            Roles::Operator => "Operator".to_string(),
        }
    }
}

//Read jwt secret from environment variable based on role
impl Roles {
    pub fn jwt_secret(&self) -> String {
        match self {
            Roles::User => std::env::var("JWT_USER_SECRET").expect("JWT_USER_SECRET must be set"),
            Roles::Admin => {
                std::env::var("JWT_ADMIN_SECRET").expect("JWT_ADMIN_SECRET must be set")
            }
            Roles::Operator => {
                std::env::var("JWT_OPERATOR_SECRET").expect("JWT_OPERATOR_SECRET must be set")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn test_roles_from_string_valid() {
        assert_eq!(Roles::from("User".to_string()), Roles::User);
        assert_eq!(Roles::from("Admin".to_string()), Roles::Admin);
        assert_eq!(Roles::from("Operator".to_string()), Roles::Operator);
    }

    #[test]
    #[should_panic(expected = "Unknown role: Unknown")]
    fn test_roles_from_string_invalid_panics() {
        let _ = Roles::from("Unknown".to_string());
    }

    #[test]
    #[should_panic(expected = "Unknown role: ")]
    fn test_roles_from_string_empty_panics() {
        let _ = Roles::from("".to_string());
    }

    #[test]
    #[should_panic(expected = "Unknown role: user")]
    fn test_roles_from_string_case_sensitive_panics() {
        let _ = Roles::from("user".to_string());
    }

    #[test]
    fn test_roles_into_string() {
        let s_user: String = Roles::User.into();
        assert_eq!(s_user, "User");

        let s_admin: String = Roles::Admin.into();
        assert_eq!(s_admin, "Admin");

        let s_operator: String = Roles::Operator.into();
        assert_eq!(s_operator, "Operator");
    }

    #[test]
    fn test_roles_jwt_secret_success() {
        let _lock = lock_env();
        std::env::set_var("JWT_USER_SECRET", "user_secret_123");
        std::env::set_var("JWT_ADMIN_SECRET", "admin_secret_456");
        std::env::set_var("JWT_OPERATOR_SECRET", "operator_secret_789");

        assert_eq!(Roles::User.jwt_secret(), "user_secret_123");
        assert_eq!(Roles::Admin.jwt_secret(), "admin_secret_456");
        assert_eq!(Roles::Operator.jwt_secret(), "operator_secret_789");
    }

    #[test]
    #[should_panic(expected = "JWT_USER_SECRET must be set")]
    fn test_roles_jwt_secret_user_missing_panics() {
        let _lock = lock_env();
        std::env::remove_var("JWT_USER_SECRET");
        let _ = Roles::User.jwt_secret();
    }

    #[test]
    #[should_panic(expected = "JWT_ADMIN_SECRET must be set")]
    fn test_roles_jwt_secret_admin_missing_panics() {
        let _lock = lock_env();
        std::env::remove_var("JWT_ADMIN_SECRET");
        let _ = Roles::Admin.jwt_secret();
    }

    #[test]
    #[should_panic(expected = "JWT_OPERATOR_SECRET must be set")]
    fn test_roles_jwt_secret_operator_missing_panics() {
        let _lock = lock_env();
        std::env::remove_var("JWT_OPERATOR_SECRET");
        let _ = Roles::Operator.jwt_secret();
    }

    #[test]
    fn test_roles_serde_roundtrip() {
        for role in [Roles::User, Roles::Admin, Roles::Operator] {
            let serialized = serde_json::to_string(&role).expect("serialization failed");
            let deserialized: Roles = serde_json::from_str(&serialized).expect("deserialization failed");
            assert_eq!(role, deserialized);
        }
    }

    #[test]
    fn test_roles_clone_and_equality() {
        let role = Roles::Admin;
        let role_clone = role.clone();
        assert_eq!(role, role_clone);
        assert_ne!(Roles::Admin, Roles::User);
        assert_ne!(Roles::Admin, Roles::Operator);
    }
}
