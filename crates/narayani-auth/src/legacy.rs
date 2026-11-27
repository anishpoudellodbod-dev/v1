use narayani_core::{DatabaseResource, ResourceName, Roles};
use serde::de::{self, Deserializer, SeqAccess, Visitor};
use serde::ser::SerializeSeq;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScopeId {
    Int(i64),
    Str(String),
}

impl ScopeId {
    pub fn matches(&self, target: &str) -> bool {
        let needle = target.trim();
        match self {
            ScopeId::Int(n) => n.to_string() == needle,
            ScopeId::Str(s) => s.trim().eq_ignore_ascii_case(needle),
        }
    }
}

/// Scope list that supports either `"all"` or an explicit list of IDs (strings or integers).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllowedScope {
    All,
    Ids(Vec<ScopeId>),
}

impl AllowedScope {
    pub fn allows(&self, target: &str) -> bool {
        match self {
            AllowedScope::All => true,
            AllowedScope::Ids(ids) => ids.iter().any(|id| id.matches(target)),
        }
    }
}

impl Serialize for AllowedScope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            AllowedScope::All => serializer.serialize_str("all"),
            AllowedScope::Ids(ids) => {
                let mut seq = serializer.serialize_seq(Some(ids.len()))?;
                for item in ids {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for AllowedScope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct AllowedScopeVisitor;

        impl<'de> Visitor<'de> for AllowedScopeVisitor {
            type Value = AllowedScope;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str(r#""all" or an array of string/integer IDs"#)
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let trimmed = value.trim();
                if trimmed.eq_ignore_ascii_case("all") || trimmed == "*" {
                    Ok(AllowedScope::All)
                } else {
                    Err(E::custom(format!(
                        r#"expected "all" or a list of ids, got "{}""#,
                        value
                    )))
                }
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut ids = Vec::new();
                while let Some(elem) = seq.next_element::<Value>()? {
                    match elem {
                        Value::String(s) => {
                            let trimmed = s.trim();
                            if trimmed.eq_ignore_ascii_case("all") || trimmed == "*" {
                                // Drain remainder of sequence cleanly if needed
                                while seq.next_element::<Value>()?.is_some() {}
                                return Ok(AllowedScope::All);
                            }
                            if !trimmed.is_empty() {
                                ids.push(ScopeId::Str(trimmed.to_string()));
                            }
                        }
                        Value::Number(num) => {
                            if let Some(n) = num.as_i64() {
                                ids.push(ScopeId::Int(n));
                            } else if let Some(u) = num.as_u64() {
                                if let Ok(n) = i64::try_from(u) {
                                    ids.push(ScopeId::Int(n));
                                } else {
                                    ids.push(ScopeId::Str(u.to_string()));
                                }
                            } else {
                                return Err(de::Error::custom(
                                    "numeric scope id must be an integer",
                                ));
                            }
                        }
                        other => {
                            return Err(de::Error::custom(format!(
                                "invalid scope id element in array: {}",
                                other
                            )));
                        }
                    }
                }
                Ok(AllowedScope::Ids(ids))
            }
        }

        deserializer.deserialize_any(AllowedScopeVisitor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowedRule {
    pub actions: AllowedScope,
    pub resources: AllowedScope,
}

impl AllowedRule {
    pub fn allows(&self, action: &str, resource: &str) -> bool {
        self.actions.allows(action) && self.resources.allows(resource)
    }
}

fn default_user_permissions() -> Value {
    serde_json::json!({})
}

fn deserialize_allowed_rules<'de, D>(deserializer: D) -> Result<Vec<AllowedRule>, D::Error>
where
    D: Deserializer<'de>,
{
    let val = Value::deserialize(deserializer)?;
    match val {
        Value::Null => Ok(Vec::new()),
        Value::Array(items) => items
            .into_iter()
            .map(|item| serde_json::from_value(item).map_err(de::Error::custom))
            .collect(),
        Value::Object(_) => {
            let single: AllowedRule = serde_json::from_value(val).map_err(de::Error::custom)?;
            Ok(vec![single])
        }
        other => Err(de::Error::custom(format!(
            "Allowed must be an array of {{actions, resources}} rules, got {}",
            other
        ))),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PermissionInnerContent {
    #[serde(rename = "userId", alias = "user_id")]
    pub user_id: i32,
    #[serde(rename = "userRole", alias = "user_role")]
    pub user_role: Roles,
    #[serde(
        rename = "userPermissions",
        alias = "user_permissions",
        default = "default_user_permissions"
    )]
    pub user_permissions: Value,
    #[serde(
        rename = "sthaniyaBranch",
        alias = "sthaniya_branch",
        alias = "SanghBranch",
        alias = "sanghBranch",
        default
    )]
    pub sthaniya_branch: Option<String>,
    #[serde(
        rename = "Allowed",
        alias = "allowed",
        default,
        deserialize_with = "deserialize_allowed_rules"
    )]
    pub allowed: Vec<AllowedRule>,
}

impl PermissionInnerContent {
    pub fn default_allowed_for_role(role: &Roles) -> Vec<AllowedRule> {
        match role {
            Roles::Admin => vec![AllowedRule {
                actions: AllowedScope::All,
                resources: AllowedScope::All,
            }],
            Roles::Operator | Roles::User => Vec::new(),
        }
    }

    pub fn new_for_user(
        user_id: i32,
        user_role: Roles,
        sthaniya_branch: Option<String>,
        user_permissions: Option<Value>,
        allowed: Option<Vec<AllowedRule>>,
    ) -> Self {
        let allowed = allowed.unwrap_or_else(|| Self::default_allowed_for_role(&user_role));
        Self {
            user_id,
            user_role,
            user_permissions: user_permissions.unwrap_or_else(default_user_permissions),
            sthaniya_branch: normalize_optional_branch(sthaniya_branch),
            allowed,
        }
    }

    pub fn is_allowed(&self, action: &str, resource: &str) -> bool {
        self.allowed
            .iter()
            .any(|rule| rule.allows(action, resource))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PermissionResource {
    pub id: Option<i32>,
    pub resource_name: ResourceName,
    pub inner_content: PermissionInnerContent,
    pub external_content: Option<Value>,
}

impl PermissionResource {
    pub fn new(
        id: Option<i32>,
        inner_content: PermissionInnerContent,
        external_content: Option<Value>,
    ) -> Self {
        Self {
            id,
            resource_name: ResourceName::PermissionResource,
            inner_content,
            external_content,
        }
    }

    pub fn to_database_resource(&self) -> DatabaseResource {
        DatabaseResource {
            id: self.id,
            resource_name: Some(ResourceName::PermissionResource),
            inner_content: serde_json::to_value(&self.inner_content).ok(),
            external_content: self.external_content.clone(),
        }
    }

    pub fn from_database_resource(resource: &DatabaseResource) -> Result<Self, String> {
        let inner_val = resource
            .inner_content
            .as_ref()
            .ok_or_else(|| "PermissionResource missing inner_content".to_string())?;
        let inner: PermissionInnerContent = serde_json::from_value(inner_val.clone())
            .map_err(|e| format!("Invalid PermissionResource inner_content: {e}"))?;
        Ok(Self {
            id: resource.id,
            resource_name: ResourceName::PermissionResource,
            inner_content: inner,
            external_content: resource.external_content.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionError {
    NotFound(String),
    Conflict(String),
    BadRequest(String),
    Internal(String),
}


#[derive(Debug, Clone, Deserialize)]
pub struct CreatePermissionRequest {
    #[serde(rename = "userId", alias = "user_id")]
    pub user_id: i32,
    #[serde(rename = "userRole", alias = "user_role", default)]
    pub user_role: Option<Roles>,
    #[serde(rename = "userPermissions", alias = "user_permissions", default)]
    pub user_permissions: Option<Value>,
    #[serde(
        rename = "sthaniyaBranch",
        alias = "sthaniya_branch",
        alias = "SanghBranch",
        alias = "sanghBranch",
        default
    )]
    pub sthaniya_branch: Option<String>,
    #[serde(
        rename = "Allowed",
        alias = "allowed",
        default,
        deserialize_with = "deserialize_allowed_opt"
    )]
    pub allowed: Option<Vec<AllowedRule>>,
    #[serde(default)]
    pub external_content: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdatePermissionRequest {
    #[serde(rename = "userRole", alias = "user_role", default)]
    pub user_role: Option<Roles>,
    #[serde(rename = "userPermissions", alias = "user_permissions", default)]
    pub user_permissions: Option<Value>,
    #[serde(
        rename = "sthaniyaBranch",
        alias = "sthaniya_branch",
        alias = "SanghBranch",
        alias = "sanghBranch",
        default
    )]
    pub sthaniya_branch: Option<String>,
    #[serde(
        rename = "Allowed",
        alias = "allowed",
        default,
        deserialize_with = "deserialize_allowed_opt"
    )]
    pub allowed: Option<Vec<AllowedRule>>,
    #[serde(default)]
    pub external_content: Option<Value>,
}

fn deserialize_allowed_opt<'de, D>(deserializer: D) -> Result<Option<Vec<AllowedRule>>, D::Error>
where
    D: Deserializer<'de>,
{
    let val = Option::<Value>::deserialize(deserializer)?;
    match val {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => items
            .into_iter()
            .map(|item| serde_json::from_value(item).map_err(de::Error::custom))
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(Value::Object(obj)) => {
            let single: AllowedRule =
                serde_json::from_value(Value::Object(obj)).map_err(de::Error::custom)?;
            Ok(Some(vec![single]))
        }
        Some(other) => Err(de::Error::custom(format!(
            "Allowed must be an array of rules or rule object, got {}",
            other
        ))),
    }
}

pub fn normalize_optional_branch(branch: Option<String>) -> Option<String> {
    branch
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Extracts `SanghBranch` / `sthaniyaBranch` from a JSON resource payload.
pub fn extract_sangh_branch(value: &Value) -> Option<String> {
    for key in [
        "sthaniyaBranch",
        "SanghBranch",
        "sanghBranch",
        "sthaniya_branch",
        "sangh_branch",
    ] {
        if let Some(raw) = value.get(key).and_then(Value::as_str) {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    narayani_settings::resolve_tqms_sangh_branch_from_value(value)
}

pub fn extract_permission_user_id(inner: &Value) -> Option<i32> {
    for key in ["userId", "user_id"] {
        if let Some(val) = inner.get(key) {
            if let Some(n) = val.as_i64() {
                if let Ok(uid) = i32::try_from(n) {
                    return Some(uid);
                }
            } else if let Some(s) = val.as_str() {
                if let Ok(uid) = s.trim().parse::<i32>() {
                    return Some(uid);
                }
            }
        }
    }
    None
}

pub fn parse_role_from_inner(inner: &Value) -> Option<Roles> {
    inner
        .get("role")
        .or_else(|| inner.get("userRole"))
        .or_else(|| inner.get("user_role"))
        .and_then(|v| serde_json::from_value::<Roles>(v.clone()).ok())
}
