use crate::actions::is_admin;
use crate::auth;
use crate::cache::Cache;
use crate::db::Database;
use crate::resource::{DatabaseResource, ResourceName};
use crate::roles::Roles;
use crate::ServerData;
use actix_web::http::header::AUTHORIZATION;
use actix_web::{web, HttpMessage, HttpRequest, HttpResponse};
use serde::de::{self, Deserializer, SeqAccess, Visitor};
use serde::ser::SerializeSeq;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
use std::collections::HashSet;
use std::fmt;
use std::sync::{Arc, OnceLock, RwLock as StdRwLock};

static REVOKED_TOKENS: OnceLock<StdRwLock<HashSet<String>>> = OnceLock::new();

fn revoked_tokens_store() -> &'static StdRwLock<HashSet<String>> {
    REVOKED_TOKENS.get_or_init(|| StdRwLock::new(HashSet::new()))
}

/// Marks a JWT as logged out / revoked so subsequent authenticated requests reject it.
pub fn revoke_token(token: &str) {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return;
    }
    let mut guard = revoked_tokens_store()
        .write()
        .unwrap_or_else(|e| e.into_inner());
    guard.insert(trimmed.to_string());
}

/// Returns true if the supplied JWT was revoked on logout.
pub fn is_token_revoked(token: &str) -> bool {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return false;
    }
    let guard = revoked_tokens_store()
        .read()
        .unwrap_or_else(|e| e.into_inner());
    guard.contains(trimmed)
}

/// Extracts the bearer token from an Authorization header if present.
pub fn extract_bearer_token(req: &HttpRequest) -> Option<String> {
    req.headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| {
            let trimmed = h.trim();
            if trimmed.len() >= 7 && trimmed[..7].eq_ignore_ascii_case("Bearer ") {
                Some(trimmed[7..].trim())
            } else {
                None
            }
        })
        .filter(|t| !t.is_empty())
        .map(String::from)
}

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

impl PermissionError {
    pub fn to_http_response(&self) -> HttpResponse {
        match self {
            PermissionError::NotFound(msg) => HttpResponse::NotFound().json(serde_json::json!({
                "success": false,
                "message": msg
            })),
            PermissionError::Conflict(msg) => HttpResponse::Conflict().json(serde_json::json!({
                "success": false,
                "message": msg
            })),
            PermissionError::BadRequest(msg) => HttpResponse::BadRequest().json(serde_json::json!({
                "success": false,
                "message": msg
            })),
            PermissionError::Internal(msg) => {
                HttpResponse::InternalServerError().json(serde_json::json!({
                    "success": false,
                    "message": msg
                }))
            }
        }
    }
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
    crate::settings::resolve_tqms_sangh_branch_from_value(value)
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

/// Returns the active logged-in `PermissionResource` for `user_id` currently loaded in `Cache`.
pub fn get_active_permission_from_cache(cache: &Cache, user_id: i32) -> Option<PermissionResource> {
    let entries = cache.cache.get(&ResourceName::PermissionResource)?;
    for resource in entries.values() {
        let Some(inner_val) = &resource.inner_content else {
            continue;
        };
        if extract_permission_user_id(inner_val) == Some(user_id) {
            if let Ok(perm) = PermissionResource::from_database_resource(resource) {
                return Some(perm);
            }
        }
    }
    None
}

pub fn is_user_logged_in_cache(cache: &Cache, user_id: i32) -> bool {
    get_active_permission_from_cache(cache, user_id).is_some()
}

/// Evicts any cached `PermissionResource` entries mapped to `user_id`.
pub fn remove_user_permissions_from_cache(cache: &mut Cache, user_id: i32) -> bool {
    let Some(entries) = cache.cache.get_mut(&ResourceName::PermissionResource) else {
        return false;
    };
    let keys_to_remove: Vec<i32> = entries
        .iter()
        .filter_map(|(&perm_id, resource)| {
            let inner = resource.inner_content.as_ref()?;
            if extract_permission_user_id(inner) == Some(user_id) {
                Some(perm_id)
            } else {
                None
            }
        })
        .collect();

    let removed_any = !keys_to_remove.is_empty();
    for key in keys_to_remove {
        entries.remove(&key);
    }
    removed_any
}

pub async fn get_setting_sangh_branch_from_db(db: &Database) -> Option<String> {
    let settings = db
        .read_all_resource(ResourceName::SettingResource)
        .await
        .ok()?;
    for setting in settings {
        if let Some(inner) = &setting.inner_content {
            if let Some(branch) = extract_sangh_branch(inner) {
                return Some(branch);
            }
        }
    }
    None
}

pub async fn get_setting_sangh_branch(s_data: &ServerData) -> Option<String> {
    {
        let cache = s_data.cache.read().await;
        if let Ok(settings) = cache.read_all(ResourceName::SettingResource) {
            for setting in settings {
                if let Some(inner) = &setting.inner_content {
                    if let Some(branch) = extract_sangh_branch(inner) {
                        return Some(branch);
                    }
                }
            }
        }
    }
    get_setting_sangh_branch_from_db(&s_data.db).await
}

pub async fn find_user_by_id_in_db(
    db: &Database,
    user_id: i32,
) -> Result<Option<DatabaseResource>, tokio_postgres::Error> {
    let res = db.read_resource(ResourceName::UserResource, user_id).await?;
    if res.id.is_some() {
        Ok(Some(res))
    } else {
        Ok(None)
    }
}

pub async fn find_user_by_email_in_db(
    db: &Database,
    email: &str,
) -> Result<Option<DatabaseResource>, tokio_postgres::Error> {
    let target = email.trim().to_lowercase();
    let users = db.read_all_resource(ResourceName::UserResource).await?;
    Ok(users.into_iter().find(|user| {
        user.inner_content
            .as_ref()
            .and_then(|c| c.get("email"))
            .and_then(Value::as_str)
            .map(|e| e.trim().to_lowercase() == target)
            .unwrap_or(false)
    }))
}

pub async fn find_permission_by_user_id_in_db(
    db: &Database,
    user_id: i32,
) -> Result<Option<PermissionResource>, String> {
    let all_perms = db
        .read_all_resource(ResourceName::PermissionResource)
        .await
        .map_err(|e| format!("Failed to read PermissionResource table: {e:?}"))?;

    for resource in all_perms {
        let Some(inner) = &resource.inner_content else {
            continue;
        };
        if extract_permission_user_id(inner) == Some(user_id) {
            let parsed = PermissionResource::from_database_resource(&resource)?;
            return Ok(Some(parsed));
        }
    }
    Ok(None)
}

pub async fn resolve_user_id_from_claims(db: &Database, claims: &auth::Claims) -> Option<i32> {
    if let Some(uid) = claims.user_id {
        return Some(uid);
    }
    find_user_by_email_in_db(db, &claims.sub)
        .await
        .ok()
        .flatten()
        .and_then(|u| u.id)
}

/// Resolves the creator's own `SanghBranch` / `sthaniyaBranch` using claims from the request.
pub async fn resolve_creator_own_sangh_branch(
    req: &HttpRequest,
    s_data: &ServerData,
) -> Option<String> {
    let claims = {
        let ext = req.extensions();
        ext.get::<auth::Claims>().cloned()?
    };

    let creator_id = resolve_user_id_from_claims(&s_data.db, &claims).await;

    if let Some(uid) = creator_id {
        // 1. Creator's active loaded PermissionResource in cache
        {
            let cache = s_data.cache.read().await;
            if let Some(active_perm) = get_active_permission_from_cache(&cache, uid) {
                if let Some(branch) = normalize_optional_branch(active_perm.inner_content.sthaniya_branch) {
                    return Some(branch);
                }
            }
        }

        // 2. Creator's 1:1 PermissionResource in DB
        if let Ok(Some(perm)) = find_permission_by_user_id_in_db(&s_data.db, uid).await {
            if let Some(branch) = normalize_optional_branch(perm.inner_content.sthaniya_branch) {
                return Some(branch);
            }
        }

        // 3. Creator's UserResource in DB
        if let Ok(Some(user)) = find_user_by_id_in_db(&s_data.db, uid).await {
            if let Some(inner) = &user.inner_content {
                if let Some(branch) = extract_sangh_branch(inner) {
                    return Some(branch);
                }
            }
        }
    } else if let Ok(Some(user)) = find_user_by_email_in_db(&s_data.db, &claims.sub).await {
        if let Some(inner) = &user.inner_content {
            if let Some(branch) = extract_sangh_branch(inner) {
                return Some(branch);
            }
        }
    }

    None
}

/// On new creation, follows the creator's `SanghBranch` first; if the creator has none,
/// falls back to the supplied payload branch, then `SettingResource.SanghBranch`.
pub async fn resolve_branch_for_new_creation(
    req: &HttpRequest,
    s_data: &ServerData,
    payload_branch: Option<String>,
) -> Option<String> {
    if let Some(creator_branch) = resolve_creator_own_sangh_branch(req, s_data).await {
        return Some(creator_branch);
    }
    if let Some(explicit) = normalize_optional_branch(payload_branch) {
        return Some(explicit);
    }
    get_setting_sangh_branch(s_data).await
}

/// Creates a `PermissionResource` in DB enforcing strict 1:1 uniqueness on `userId`.
pub async fn create_permission_in_db_strict(
    db: &Database,
    mut inner: PermissionInnerContent,
    external_content: Option<Value>,
) -> Result<PermissionResource, PermissionError> {
    let user = find_user_by_id_in_db(db, inner.user_id)
        .await
        .map_err(|e| PermissionError::Internal(format!("Database error reading user: {e:?}")))?;
    if user.is_none() {
        return Err(PermissionError::NotFound(format!(
            "UserResource with id {} not found",
            inner.user_id
        )));
    }

    if let Some(existing) = find_permission_by_user_id_in_db(db, inner.user_id)
        .await
        .map_err(PermissionError::Internal)?
    {
        return Err(PermissionError::Conflict(format!(
            "PermissionResource already exists for userId {} (permission id: {:?}); 1:1 mapping enforced",
            inner.user_id, existing.id
        )));
    }

    inner.sthaniya_branch = normalize_optional_branch(inner.sthaniya_branch);

    let draft = PermissionResource::new(None, inner, external_content);
    let perm_id = db
        .create_resource(ResourceName::PermissionResource, draft.to_database_resource())
        .await
        .map_err(|e| PermissionError::Internal(format!("Failed to create PermissionResource: {e:?}")))?;

    Ok(PermissionResource::new(
        Some(perm_id),
        draft.inner_content,
        draft.external_content,
    ))
}

/// Ensures the user has a 1:1 `PermissionResource` in DB (creating one if absent, or syncing
/// drifted role/missing branch if present). Does NOT load into `Cache`.
pub async fn ensure_permission_in_db_for_user(
    db: &Database,
    user_id: i32,
    user_inner: &Value,
    branch_override: Option<String>,
) -> Result<PermissionResource, String> {
    let role = parse_role_from_inner(user_inner)
        .ok_or_else(|| format!("UserResource {} is missing a valid role", user_id))?;
    let branch_candidate =
        normalize_optional_branch(branch_override).or_else(|| extract_sangh_branch(user_inner));

    if let Some(mut existing) = find_permission_by_user_id_in_db(db, user_id).await? {
        let mut changed = false;
        if existing.inner_content.user_role != role {
            existing.inner_content.user_role = role;
            changed = true;
        }
        if existing.inner_content.sthaniya_branch.is_none() {
            if let Some(branch) = branch_candidate {
                existing.inner_content.sthaniya_branch = Some(branch);
                changed = true;
            }
        }
        if changed {
            if let Some(perm_id) = existing.id {
                db.update_resource(
                    ResourceName::PermissionResource,
                    perm_id,
                    existing.to_database_resource(),
                )
                .await
                .map_err(|e| format!("Failed to update PermissionResource {perm_id}: {e:?}"))?;
            }
        }
        return Ok(existing);
    }

    let user_permissions = user_inner
        .get("userPermissions")
        .or_else(|| user_inner.get("user_permissions"))
        .cloned();
    let allowed = user_inner
        .get("Allowed")
        .or_else(|| user_inner.get("allowed"))
        .and_then(|v| serde_json::from_value::<Vec<AllowedRule>>(v.clone()).ok());

    let inner = PermissionInnerContent::new_for_user(
        user_id,
        role,
        branch_candidate,
        user_permissions,
        allowed,
    );
    let draft = PermissionResource::new(None, inner, None);
    let perm_id = db
        .create_resource(ResourceName::PermissionResource, draft.to_database_resource())
        .await
        .map_err(|e| format!("Failed to create PermissionResource for user {user_id}: {e:?}"))?;

    Ok(PermissionResource::new(
        Some(perm_id),
        draft.inner_content,
        draft.external_content,
    ))
}

/// Loads (or creates if missing for legacy users) the user's 1:1 `PermissionResource` into
/// `s_data.cache` when the user logs in.
pub async fn load_permission_on_login(
    s_data: &ServerData,
    user: &DatabaseResource,
    role: &Roles,
) -> Result<PermissionResource, String> {
    let user_id = user
        .id
        .ok_or_else(|| "UserResource is missing id".to_string())?;
    let user_inner = user
        .inner_content
        .as_ref()
        .ok_or_else(|| "UserResource is missing inner_content".to_string())?;

    let setting_branch = get_setting_sangh_branch(s_data).await;
    let fallback_branch = extract_sangh_branch(user_inner).or(setting_branch);

    let mut perm =
        ensure_permission_in_db_for_user(&s_data.db, user_id, user_inner, fallback_branch).await?;

    if perm.inner_content.user_role != *role {
        perm.inner_content.user_role = role.clone();
        if let Some(perm_id) = perm.id {
            s_data
                .db
                .update_resource(
                    ResourceName::PermissionResource,
                    perm_id,
                    perm.to_database_resource(),
                )
                .await
                .map_err(|e| format!("Failed to sync PermissionResource role on login: {e:?}"))?;
        }
    }

    let perm_id = perm
        .id
        .ok_or_else(|| "Loaded PermissionResource missing id".to_string())?;

    {
        let mut cache = s_data.cache.write().await;
        remove_user_permissions_from_cache(&mut cache, user_id);
        cache.write(
            ResourceName::PermissionResource,
            perm_id,
            perm.to_database_resource(),
        );
    }

    Ok(perm)
}

/// Destroys the user's loaded `PermissionResource` from `s_data.cache` on logout and revokes
/// the caller's Bearer JWT if supplied.
pub async fn destroy_permission_on_logout(
    s_data: &ServerData,
    user_id: Option<i32>,
    token: Option<&str>,
) -> bool {
    if let Some(raw_token) = token {
        revoke_token(raw_token);
    }

    if let Some(uid) = user_id {
        let mut cache = s_data.cache.write().await;
        remove_user_permissions_from_cache(&mut cache, uid)
    } else {
        false
    }
}

/// Syncs `UserResource` role / `SanghBranch` updates to its 1:1 `PermissionResource` in DB,
/// refreshing `s_data.cache` ONLY if that user is currently logged in.
pub async fn sync_permission_on_user_update(
    s_data: &ServerData,
    user_id: i32,
    updated_user_inner: &Value,
) -> Result<PermissionResource, String> {
    let explicit_branch = extract_sangh_branch(updated_user_inner);
    let mut perm = if let Some(mut existing) =
        find_permission_by_user_id_in_db(&s_data.db, user_id).await?
    {
        let mut changed = false;
        if let Some(role) = parse_role_from_inner(updated_user_inner) {
            if existing.inner_content.user_role != role {
                existing.inner_content.user_role = role;
                changed = true;
            }
        }
        if let Some(branch) = explicit_branch {
            if existing.inner_content.sthaniya_branch.as_deref() != Some(branch.as_str()) {
                existing.inner_content.sthaniya_branch = Some(branch);
                changed = true;
            }
        }
        if changed {
            if let Some(perm_id) = existing.id {
                s_data
                    .db
                    .update_resource(
                        ResourceName::PermissionResource,
                        perm_id,
                        existing.to_database_resource(),
                    )
                    .await
                    .map_err(|e| format!("Failed to update PermissionResource {perm_id}: {e:?}"))?;
            }
        }
        existing
    } else {
        let setting_branch = get_setting_sangh_branch(s_data).await;
        ensure_permission_in_db_for_user(
            &s_data.db,
            user_id,
            updated_user_inner,
            explicit_branch.or(setting_branch),
        )
        .await?
    };

    if let Some(perm_id) = perm.id {
        let mut cache = s_data.cache.write().await;
        if is_user_logged_in_cache(&cache, user_id) {
            remove_user_permissions_from_cache(&mut cache, user_id);
            perm.id = Some(perm_id);
            cache.write(
                ResourceName::PermissionResource,
                perm_id,
                perm.to_database_resource(),
            );
        }
    }

    Ok(perm)
}

/// Deletes the 1:1 `PermissionResource` mapped to `user_id` from DB and evicts it from cache.
pub async fn delete_permission_for_user(
    s_data: &ServerData,
    user_id: i32,
) -> Result<bool, String> {
    let existing = find_permission_by_user_id_in_db(&s_data.db, user_id).await?;
    let mut deleted_in_db = false;

    if let Some(perm) = existing {
        if let Some(perm_id) = perm.id {
            s_data
                .db
                .delete_resource(ResourceName::PermissionResource, perm_id)
                .await
                .map_err(|e| format!("Failed to delete PermissionResource {perm_id}: {e:?}"))?;
            deleted_in_db = true;
        }
    }

    let removed_from_cache = {
        let mut cache = s_data.cache.write().await;
        remove_user_permissions_from_cache(&mut cache, user_id)
    };

    Ok(deleted_in_db || removed_from_cache)
}

/// `POST /api/v1/auth/logout` — verifies Bearer token, destroys cached `PermissionResource`,
/// and revokes the Bearer token.
pub async fn logout(req: HttpRequest, s_data: web::Data<Arc<ServerData>>) -> HttpResponse {
    let started_at = std::time::Instant::now();
    let (response, email, user_id, role, details) = logout_inner(&req, &s_data).await;
    let status_code = response.status().as_u16();

    crate::log::log_logout_async(
        email.clone(),
        user_id,
        role.clone(),
        status_code,
        started_at,
        details.clone(),
    );

    crate::audit::record_http_audit_async(crate::audit::HttpAuditEvent {
        req: Some(&req),
        category: crate::audit::AuditCategory::Auth,
        action: crate::audit::AuditAction::Logout,
        status_code,
        started_at: Some(started_at),
        resource_name: Some("UserResource".to_string()),
        resource_id: user_id.map(|id| id.to_string()),
        summary: details.unwrap_or_else(|| "User logout".to_string()),
        actor_email_override: email,
        actor_user_id_override: user_id,
        actor_role_override: role,
        ..Default::default()
    });

    response
}

async fn logout_inner(
    req: &HttpRequest,
    s_data: &Arc<ServerData>,
) -> (
    HttpResponse,
    Option<String>,
    Option<i32>,
    Option<String>,
    Option<String>,
) {
    let Some(token) = extract_bearer_token(req) else {
        return (
            HttpResponse::Unauthorized().json(serde_json::json!({
                "success": false,
                "message": "Missing or invalid Authorization header"
            })),
            None,
            None,
            None,
            Some("Missing or invalid Authorization header".to_string()),
        );
    };

    if is_token_revoked(&token) {
        return (
            HttpResponse::Unauthorized().json(serde_json::json!({
                "success": false,
                "message": "Token has already been logged out"
            })),
            None,
            None,
            None,
            Some("Token already logged out".to_string()),
        );
    }

    let claims = match auth::verify_jwt(&token) {
        Ok(claims) => claims,
        Err(error) => {
            eprintln!("Logout JWT verification failed: {error:?}");
            return (
                HttpResponse::Unauthorized().json(serde_json::json!({
                    "success": false,
                    "message": "Invalid or expired token"
                })),
                None,
                None,
                None,
                Some("Invalid or expired token".to_string()),
            );
        }
    };

    let email = Some(claims.sub.clone());
    let role = Some(String::from(claims.role.clone()));
    let user_id = resolve_user_id_from_claims(&s_data.db, &claims).await;
    let destroyed = destroy_permission_on_logout(s_data, user_id, Some(&token)).await;

    (
        HttpResponse::Ok().json(serde_json::json!({
            "success": true,
            "message": "Logged out successfully",
            "userId": user_id,
            "permissionDestroyed": destroyed
        })),
        email,
        user_id,
        role,
        Some(format!("Logged out (permissionDestroyed={destroyed})")),
    )
}

/// `GET /api/v1/access/me` — returns the caller's active loaded `PermissionResource` from cache.
pub async fn get_my_permission(
    req: HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let Some(claims) = req.extensions().get::<auth::Claims>().cloned() else {
        return HttpResponse::Unauthorized().json(serde_json::json!({
            "success": false,
            "message": "Missing authenticated claims"
        }));
    };

    let Some(user_id) = resolve_user_id_from_claims(&s_data.db, &claims).await else {
        return HttpResponse::NotFound().json(serde_json::json!({
            "success": false,
            "message": format!("User not found for {}", claims.sub)
        }));
    };

    let cache = s_data.cache.read().await;
    match get_active_permission_from_cache(&cache, user_id) {
        Some(perm) => HttpResponse::Ok().json(perm),
        None => HttpResponse::Unauthorized().json(serde_json::json!({
            "success": false,
            "message": "No active PermissionResource loaded for this session; please log in"
        })),
    }
}

/// `GET /api/v1/access/user/{user_id}` — Admin-only lookup from DB (does not pollute login cache).
pub async fn get_permission_by_user_id(
    path: web::Path<i32>,
    req: HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    if !is_admin(req) {
        return HttpResponse::Forbidden().json(serde_json::json!({
            "success": false,
            "message": "Admin access required"
        }));
    }

    let target_user_id = path.into_inner();
    match find_permission_by_user_id_in_db(&s_data.db, target_user_id).await {
        Ok(Some(perm)) => HttpResponse::Ok().json(perm),
        Ok(None) => HttpResponse::NotFound().json(serde_json::json!({
            "success": false,
            "message": format!("PermissionResource not found for userId {}", target_user_id)
        })),
        Err(msg) => HttpResponse::InternalServerError().json(serde_json::json!({
            "success": false,
            "message": msg
        })),
    }
}

/// `POST /api/v1/access/permission` — Admin-only 1:1 creation; follows creator's `SanghBranch`.
pub async fn create_permission(
    payload: web::Json<CreatePermissionRequest>,
    req: HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let started_at = std::time::Instant::now();
    let assign_audit_required = match crate::permissions::enforce_request_permission(
        &req,
        &s_data,
        "PermissionResource",
        crate::permissions::action::ASSIGN,
        crate::permissions::extract_branch_id_from_req(&req),
    )
    .await
    {
        crate::permissions::PermissionCheckOutcome::Denied { message, .. } => {
            return HttpResponse::Forbidden().json(serde_json::json!({
                "success": false,
                "message": message
            }));
        }
        crate::permissions::PermissionCheckOutcome::Allowed { audit_required, .. } => {
            audit_required
        }
        crate::permissions::PermissionCheckOutcome::FallbackLegacy => {
            if !is_admin(req.clone()) {
                crate::audit::record_http_audit_async(crate::audit::HttpAuditEvent {
                    req: Some(&req),
                    category: crate::audit::AuditCategory::AccessControl,
                    action: crate::audit::AuditAction::AccessDenied,
                    status_code: actix_web::http::StatusCode::FORBIDDEN.as_u16(),
                    started_at: Some(started_at),
                    resource_name: Some("PermissionResource".to_string()),
                    summary: "Non-admin create_permission rejected".to_string(),
                    ..Default::default()
                });
                return HttpResponse::Forbidden().json(serde_json::json!({
                    "success": false,
                    "message": "Admin access required"
                }));
            }
            true
        }
    };

    let body = payload.into_inner();
    let user = match find_user_by_id_in_db(&s_data.db, body.user_id).await {
        Ok(Some(u)) => u,
        Ok(None) => {
            return HttpResponse::NotFound().json(serde_json::json!({
                "success": false,
                "message": format!("UserResource with id {} not found", body.user_id)
            }));
        }
        Err(e) => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "success": false,
                "message": format!("Database error reading UserResource: {e:?}")
            }));
        }
    };

    let user_inner = user.inner_content.as_ref();
    let user_role = body
        .user_role
        .or_else(|| user_inner.and_then(parse_role_from_inner));
    let Some(role) = user_role else {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "success": false,
            "message": "userRole is required and could not be inferred from UserResource"
        }));
    };

    let fallback_payload_branch = normalize_optional_branch(body.sthaniya_branch)
        .or_else(|| user_inner.and_then(extract_sangh_branch));
    let effective_branch =
        resolve_branch_for_new_creation(&req, &s_data, fallback_payload_branch).await;

    let inner = PermissionInnerContent::new_for_user(
        body.user_id,
        role,
        effective_branch,
        body.user_permissions,
        body.allowed,
    );

    match create_permission_in_db_strict(&s_data.db, inner, body.external_content).await {
        Ok(created) => {
            if let Some(perm_id) = created.id {
                let mut cache = s_data.cache.write().await;
                if is_user_logged_in_cache(&cache, created.inner_content.user_id) {
                    remove_user_permissions_from_cache(&mut cache, created.inner_content.user_id);
                    cache.write(
                        ResourceName::PermissionResource,
                        perm_id,
                        created.to_database_resource(),
                    );
                }
            }
            crate::audit::record_http_audit_async(crate::audit::HttpAuditEvent {
                req: Some(&req),
                category: crate::audit::AuditCategory::AccessControl,
                action: crate::audit::AuditAction::PermissionGrant,
                status_code: actix_web::http::StatusCode::CREATED.as_u16(),
                started_at: Some(started_at),
                resource_name: Some("PermissionResource".to_string()),
                resource_id: created.id.map(|i| i.to_string()),
                summary: format!(
                    "Granted PermissionResource {:?} for userId {}",
                    created.id, created.inner_content.user_id
                ),
                after_state: serde_json::to_value(&created.inner_content).ok(),
                branch_override: created.inner_content.sthaniya_branch.clone(),
                metadata: Some(serde_json::json!({
                    "targetUserId": created.inner_content.user_id,
                    "auditRequired": assign_audit_required,
                })),
                ..Default::default()
            });
            HttpResponse::Created().json(created)
        }
        Err(err) => err.to_http_response(),
    }
}

/// `PUT /api/v1/access/user/{user_id}` — Admin-only update of a user's 1:1 `PermissionResource`.
pub async fn update_permission_by_user_id(
    path: web::Path<i32>,
    payload: web::Json<UpdatePermissionRequest>,
    req: HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let started_at = std::time::Instant::now();
    let target_user_id = path.into_inner();
    match crate::permissions::enforce_request_permission(
        &req,
        &s_data,
        "PermissionResource",
        crate::permissions::action::ASSIGN,
        crate::permissions::extract_branch_id_from_req(&req),
    )
    .await
    {
        crate::permissions::PermissionCheckOutcome::Denied { message, .. } => {
            return HttpResponse::Forbidden().json(serde_json::json!({
                "success": false,
                "message": message
            }));
        }
        crate::permissions::PermissionCheckOutcome::Allowed { .. } => {}
        crate::permissions::PermissionCheckOutcome::FallbackLegacy => {
            if !is_admin(req.clone()) {
                crate::audit::record_http_audit_async(crate::audit::HttpAuditEvent {
                    req: Some(&req),
                    category: crate::audit::AuditCategory::AccessControl,
                    action: crate::audit::AuditAction::AccessDenied,
                    status_code: actix_web::http::StatusCode::FORBIDDEN.as_u16(),
                    started_at: Some(started_at),
                    resource_name: Some("PermissionResource".to_string()),
                    summary: format!(
                        "Non-admin update_permission_by_user_id rejected for userId {}",
                        target_user_id
                    ),
                    metadata: Some(serde_json::json!({ "targetUserId": target_user_id })),
                    ..Default::default()
                });
                return HttpResponse::Forbidden().json(serde_json::json!({
                    "success": false,
                    "message": "Admin access required"
                }));
            }
        }
    }

    let body = payload.into_inner();

    let mut existing = match find_permission_by_user_id_in_db(&s_data.db, target_user_id).await {
        Ok(Some(perm)) => perm,
        Ok(None) => {
            return HttpResponse::NotFound().json(serde_json::json!({
                "success": false,
                "message": format!("PermissionResource not found for userId {}", target_user_id)
            }));
        }
        Err(msg) => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "success": false,
                "message": msg
            }));
        }
    };

    let before_perm = serde_json::to_value(&existing.inner_content).ok();

    if let Some(role) = body.user_role {
        existing.inner_content.user_role = role;
    }
    if let Some(perms) = body.user_permissions {
        existing.inner_content.user_permissions = perms;
    }
    if let Some(branch) = normalize_optional_branch(body.sthaniya_branch) {
        existing.inner_content.sthaniya_branch = Some(branch);
    }
    if let Some(allowed) = body.allowed {
        existing.inner_content.allowed = allowed;
    }
    if body.external_content.is_some() {
        existing.external_content = body.external_content;
    }

    let Some(perm_id) = existing.id else {
        return HttpResponse::InternalServerError().json(serde_json::json!({
            "success": false,
            "message": "Existing PermissionResource is missing id"
        }));
    };

    if let Err(err) = s_data
        .db
        .update_resource(
            ResourceName::PermissionResource,
            perm_id,
            existing.to_database_resource(),
        )
        .await
    {
        return HttpResponse::InternalServerError().json(serde_json::json!({
            "success": false,
            "message": format!("Failed to update PermissionResource: {err:?}")
        }));
    }

    {
        let mut cache = s_data.cache.write().await;
        if is_user_logged_in_cache(&cache, target_user_id) {
            remove_user_permissions_from_cache(&mut cache, target_user_id);
            cache.write(
                ResourceName::PermissionResource,
                perm_id,
                existing.to_database_resource(),
            );
        }
    }

    crate::audit::record_http_audit_async(crate::audit::HttpAuditEvent {
        req: Some(&req),
        category: crate::audit::AuditCategory::AccessControl,
        action: crate::audit::AuditAction::PermissionUpdate,
        status_code: actix_web::http::StatusCode::OK.as_u16(),
        started_at: Some(started_at),
        resource_name: Some("PermissionResource".to_string()),
        resource_id: Some(perm_id.to_string()),
        summary: format!(
            "Updated PermissionResource {} for userId {}",
            perm_id, target_user_id
        ),
        before_state: before_perm,
        after_state: serde_json::to_value(&existing.inner_content).ok(),
        branch_override: existing.inner_content.sthaniya_branch.clone(),
        metadata: Some(serde_json::json!({ "targetUserId": target_user_id })),
        ..Default::default()
    });

    HttpResponse::Ok().json(existing)
}

pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.route("/me", web::get().to(get_my_permission))
        .route("/user/{user_id}", web::get().to(get_permission_by_user_id))
        .route("/permission", web::post().to(create_permission))
        .route(
            "/user/{user_id}",
            web::put().to(update_permission_by_user_id),
        );
}

#[cfg(test)]
pub(crate) mod test_helpers {
    use super::*;
    use std::collections::HashMap;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::Mutex as AsyncMutex;

    #[derive(Default)]
    pub struct MockDbState {
        pub tables: HashMap<String, Vec<DatabaseResource>>,
        pub next_ids: HashMap<String, i32>,
    }

    impl MockDbState {
        pub fn seed(&mut self, resource_name: ResourceName, resource: DatabaseResource) -> i32 {
            let table = resource_name.to_string().to_lowercase();
            let next = self.next_ids.entry(table.clone()).or_insert(1);
            let id = resource.id.unwrap_or_else(|| {
                let assigned = *next;
                *next += 1;
                assigned
            });
            if id >= *next {
                *next = id + 1;
            }
            let stored = DatabaseResource {
                id: Some(id),
                resource_name: Some(resource_name),
                inner_content: resource.inner_content,
                external_content: resource.external_content,
            };
            self.tables.entry(table).or_default().push(stored);
            id
        }
    }

    #[derive(Clone)]
    enum QueryKind {
        SelectAll(String),
        SelectById(String),
        Insert(String),
        Update(String),
        Delete(String),
    }

    fn classify_query(sql: &str) -> QueryKind {
        let lower = sql.trim().to_lowercase();
        if let Some(rest) = lower.strip_prefix("select id, inner_content, external_content from ") {
            let table = rest.trim_end_matches(';').trim().to_string();
            QueryKind::SelectAll(table)
        } else if lower.starts_with("select * from ") {
            let after = lower.strip_prefix("select * from ").unwrap();
            let table = after
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_end_matches(';')
                .to_string();
            QueryKind::SelectById(table)
        } else if lower.starts_with("insert into ") {
            let after = lower.strip_prefix("insert into ").unwrap();
            let table = after.split_whitespace().next().unwrap_or("").to_string();
            QueryKind::Insert(table)
        } else if lower.starts_with("update ") {
            let after = lower.strip_prefix("update ").unwrap();
            let table = after.split_whitespace().next().unwrap_or("").to_string();
            QueryKind::Update(table)
        } else if lower.starts_with("delete from ") {
            let after = lower.strip_prefix("delete from ").unwrap();
            let table = after
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_end_matches(';')
                .to_string();
            QueryKind::Delete(table)
        } else {
            panic!("Unsupported mock SQL: {sql}");
        }
    }

    fn read_cstr(buf: &[u8], pos: &mut usize) -> String {
        let start = *pos;
        while *pos < buf.len() && buf[*pos] != 0 {
            *pos += 1;
        }
        let s = String::from_utf8_lossy(&buf[start..*pos]).to_string();
        if *pos < buf.len() {
            *pos += 1;
        }
        s
    }

    fn read_i16(buf: &[u8], pos: &mut usize) -> i16 {
        let val = i16::from_be_bytes([buf[*pos], buf[*pos + 1]]);
        *pos += 2;
        val
    }

    fn read_i32(buf: &[u8], pos: &mut usize) -> i32 {
        let val = i32::from_be_bytes([buf[*pos], buf[*pos + 1], buf[*pos + 2], buf[*pos + 3]]);
        *pos += 4;
        val
    }

    fn push_pg_msg(out: &mut Vec<u8>, tag: u8, payload: &[u8]) {
        out.push(tag);
        let total_len = (4 + payload.len()) as i32;
        out.extend_from_slice(&total_len.to_be_bytes());
        out.extend_from_slice(payload);
    }

    fn build_param_desc(oids: &[u32]) -> Vec<u8> {
        let mut p = Vec::new();
        p.extend_from_slice(&(oids.len() as i16).to_be_bytes());
        for &oid in oids {
            p.extend_from_slice(&oid.to_be_bytes());
        }
        p
    }

    fn build_row_desc(cols: &[(&str, u32, i16)]) -> Vec<u8> {
        let mut p = Vec::new();
        p.extend_from_slice(&(cols.len() as i16).to_be_bytes());
        for &(name, oid, size) in cols {
            p.extend_from_slice(name.as_bytes());
            p.push(0);
            p.extend_from_slice(&0i32.to_be_bytes()); // table oid
            p.extend_from_slice(&0i16.to_be_bytes()); // col attr
            p.extend_from_slice(&oid.to_be_bytes());
            p.extend_from_slice(&size.to_be_bytes());
            p.extend_from_slice(&(-1i32).to_be_bytes()); // type mod
            p.extend_from_slice(&1i16.to_be_bytes()); // binary format
        }
        p
    }

    fn encode_jsonb_opt(val: &Option<Value>) -> Option<Vec<u8>> {
        val.as_ref().map(|v| {
            let mut bytes = vec![1u8];
            bytes.extend_from_slice(&serde_json::to_vec(v).expect("json serialize"));
            bytes
        })
    }

    fn decode_jsonb_opt(raw: &Option<Vec<u8>>) -> Option<Value> {
        let bytes = raw.as_ref()?;
        if bytes.is_empty() {
            return None;
        }
        let json_slice = if bytes[0] == 1 { &bytes[1..] } else { &bytes[..] };
        serde_json::from_slice(json_slice).ok()
    }

    fn build_data_row(cols: &[Option<Vec<u8>>]) -> Vec<u8> {
        let mut p = Vec::new();
        p.extend_from_slice(&(cols.len() as i16).to_be_bytes());
        for col in cols {
            match col {
                None => p.extend_from_slice(&(-1i32).to_be_bytes()),
                Some(bytes) => {
                    p.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
                    p.extend_from_slice(bytes);
                }
            }
        }
        p
    }

    pub async fn create_mock_server_data_with_state(
        initial_state: MockDbState,
    ) -> (web::Data<Arc<ServerData>>, Arc<AsyncMutex<MockDbState>>) {
        let db_state = Arc::new(AsyncMutex::new(initial_state));
        let server_state = db_state.clone();
        let (client_io, mut server_io) = tokio::io::duplex(262_144);

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

            let mut statements: HashMap<String, QueryKind> = HashMap::new();
            let mut portals: HashMap<String, (QueryKind, Vec<Option<Vec<u8>>>)> = HashMap::new();
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
                        let mut pos = 0;
                        let stmt_name = read_cstr(&body, &mut pos);
                        let sql = read_cstr(&body, &mut pos);
                        statements.insert(stmt_name, classify_query(&sql));
                        push_pg_msg(&mut resp_buf, b'1', &[]);
                    }
                    b'D' => {
                        if body.first() == Some(&b'S') {
                            let mut pos = 1;
                            let stmt_name = read_cstr(&body, &mut pos);
                            if let Some(kind) = statements.get(&stmt_name) {
                                match kind {
                                    QueryKind::SelectAll(_) => {
                                        push_pg_msg(&mut resp_buf, b't', &build_param_desc(&[]));
                                        push_pg_msg(
                                            &mut resp_buf,
                                            b'T',
                                            &build_row_desc(&[
                                                ("id", 23, 4),
                                                ("inner_content", 3802, -1),
                                                ("external_content", 3802, -1),
                                            ]),
                                        );
                                    }
                                    QueryKind::SelectById(_) => {
                                        push_pg_msg(&mut resp_buf, b't', &build_param_desc(&[23]));
                                        push_pg_msg(
                                            &mut resp_buf,
                                            b'T',
                                            &build_row_desc(&[
                                                ("id", 23, 4),
                                                ("inner_content", 3802, -1),
                                                ("external_content", 3802, -1),
                                            ]),
                                        );
                                    }
                                    QueryKind::Insert(_) => {
                                        push_pg_msg(
                                            &mut resp_buf,
                                            b't',
                                            &build_param_desc(&[3802, 3802]),
                                        );
                                        push_pg_msg(
                                            &mut resp_buf,
                                            b'T',
                                            &build_row_desc(&[("id", 23, 4)]),
                                        );
                                    }
                                    QueryKind::Update(_) => {
                                        push_pg_msg(
                                            &mut resp_buf,
                                            b't',
                                            &build_param_desc(&[3802, 3802, 23]),
                                        );
                                        push_pg_msg(&mut resp_buf, b'n', &[]);
                                    }
                                    QueryKind::Delete(_) => {
                                        push_pg_msg(&mut resp_buf, b't', &build_param_desc(&[23]));
                                        push_pg_msg(&mut resp_buf, b'n', &[]);
                                    }
                                }
                            }
                        }
                    }
                    b'B' => {
                        let mut pos = 0;
                        let portal_name = read_cstr(&body, &mut pos);
                        let stmt_name = read_cstr(&body, &mut pos);
                        let fmt_count = read_i16(&body, &mut pos) as usize;
                        for _ in 0..fmt_count {
                            let _ = read_i16(&body, &mut pos);
                        }
                        let param_count = read_i16(&body, &mut pos) as usize;
                        let mut params = Vec::with_capacity(param_count);
                        for _ in 0..param_count {
                            let plen = read_i32(&body, &mut pos);
                            if plen < 0 {
                                params.push(None);
                            } else {
                                let len = plen as usize;
                                params.push(Some(body[pos..pos + len].to_vec()));
                                pos += len;
                            }
                        }
                        if let Some(kind) = statements.get(&stmt_name).cloned() {
                            portals.insert(portal_name, (kind, params));
                        }
                        push_pg_msg(&mut resp_buf, b'2', &[]);
                    }
                    b'E' => {
                        let mut pos = 0;
                        let portal_name = read_cstr(&body, &mut pos);
                        if let Some((kind, params)) = portals.get(&portal_name).cloned() {
                            let mut st = server_state.lock().await;
                            match kind {
                                QueryKind::SelectAll(table) => {
                                    let rows = st.tables.get(&table).cloned().unwrap_or_default();
                                    let count = rows.len();
                                    for r in rows {
                                        let id_bytes = r.id.unwrap_or(0).to_be_bytes().to_vec();
                                        let inner_bytes = encode_jsonb_opt(&r.inner_content);
                                        let ext_bytes = encode_jsonb_opt(&r.external_content);
                                        push_pg_msg(
                                            &mut resp_buf,
                                            b'D',
                                            &build_data_row(&[
                                                Some(id_bytes),
                                                inner_bytes,
                                                ext_bytes,
                                            ]),
                                        );
                                    }
                                    let tag_str = format!("SELECT {count}\0");
                                    push_pg_msg(&mut resp_buf, b'C', tag_str.as_bytes());
                                }
                                QueryKind::SelectById(table) => {
                                    let target_id = params
                                        .first()
                                        .and_then(|p| p.as_ref())
                                        .map(|b| i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
                                        .unwrap_or(-1);
                                    let found = st
                                        .tables
                                        .get(&table)
                                        .and_then(|rows| rows.iter().find(|r| r.id == Some(target_id)))
                                        .cloned();
                                    if let Some(r) = found {
                                        let id_bytes = r.id.unwrap_or(0).to_be_bytes().to_vec();
                                        let inner_bytes = encode_jsonb_opt(&r.inner_content);
                                        let ext_bytes = encode_jsonb_opt(&r.external_content);
                                        push_pg_msg(
                                            &mut resp_buf,
                                            b'D',
                                            &build_data_row(&[
                                                Some(id_bytes),
                                                inner_bytes,
                                                ext_bytes,
                                            ]),
                                        );
                                        push_pg_msg(&mut resp_buf, b'C', b"SELECT 1\0");
                                    } else {
                                        push_pg_msg(&mut resp_buf, b'C', b"SELECT 0\0");
                                    }
                                }
                                QueryKind::Insert(table) => {
                                    let inner = decode_jsonb_opt(params.first().unwrap_or(&None));
                                    let ext = decode_jsonb_opt(params.get(1).unwrap_or(&None));
                                    let rname = ResourceName::from_str_case_insensitive(&table);
                                    let id = st.seed(
                                        rname.unwrap_or(ResourceName::PermissionResource),
                                        DatabaseResource {
                                            id: None,
                                            resource_name: rname,
                                            inner_content: inner,
                                            external_content: ext,
                                        },
                                    );
                                    push_pg_msg(
                                        &mut resp_buf,
                                        b'D',
                                        &build_data_row(&[Some(id.to_be_bytes().to_vec())]),
                                    );
                                    push_pg_msg(&mut resp_buf, b'C', b"INSERT 0 1\0");
                                }
                                QueryKind::Update(table) => {
                                    let inner = decode_jsonb_opt(params.first().unwrap_or(&None));
                                    let ext = decode_jsonb_opt(params.get(1).unwrap_or(&None));
                                    let target_id = params
                                        .get(2)
                                        .and_then(|p| p.as_ref())
                                        .map(|b| i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
                                        .unwrap_or(-1);
                                    let mut updated = 0;
                                    if let Some(rows) = st.tables.get_mut(&table) {
                                        if let Some(row) = rows.iter_mut().find(|r| r.id == Some(target_id)) {
                                            row.inner_content = inner;
                                            row.external_content = ext;
                                            updated = 1;
                                        }
                                    }
                                    let tag_str = format!("UPDATE {updated}\0");
                                    push_pg_msg(&mut resp_buf, b'C', tag_str.as_bytes());
                                }
                                QueryKind::Delete(table) => {
                                    let target_id = params
                                        .first()
                                        .and_then(|p| p.as_ref())
                                        .map(|b| i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
                                        .unwrap_or(-1);
                                    let mut deleted = 0;
                                    if let Some(rows) = st.tables.get_mut(&table) {
                                        let before = rows.len();
                                        rows.retain(|r| r.id != Some(target_id));
                                        deleted = before.saturating_sub(rows.len());
                                    }
                                    let tag_str = format!("DELETE {deleted}\0");
                                    push_pg_msg(&mut resp_buf, b'C', tag_str.as_bytes());
                                }
                            }
                        }
                    }
                    b'S' => {
                        push_pg_msg(&mut resp_buf, b'Z', &[b'I']);
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

        let s_data = web::Data::new(Arc::new(ServerData {
            db: Arc::new(Database::new(Arc::new(client))),
            tqms_client: Arc::new(crate::tqms_integration::TQMSClient::new().await),
            cache: Arc::new(tokio::sync::RwLock::new(Cache::new())),
        }));
        (s_data, db_state)
    }
}

#[cfg(test)]
mod tests {
    use super::test_helpers::{create_mock_server_data_with_state, MockDbState};
    use super::*;
    use actix_web::{test as web_test, App};

    fn make_setting(branch: &str) -> DatabaseResource {
        DatabaseResource {
            id: Some(1),
            resource_name: Some(ResourceName::SettingResource),
            inner_content: Some(serde_json::json!({ "SanghBranch": branch })),
            external_content: None,
        }
    }

    fn make_user(
        id: Option<i32>,
        email: &str,
        password: &str,
        role: &str,
        branch: Option<&str>,
    ) -> DatabaseResource {
        let mut map = serde_json::Map::new();
        map.insert("email".into(), Value::String(email.to_string()));
        map.insert("password".into(), Value::String(password.to_string()));
        map.insert("role".into(), Value::String(role.to_string()));
        if let Some(b) = branch {
            map.insert("SanghBranch".into(), Value::String(b.to_string()));
        }
        DatabaseResource {
            id,
            resource_name: Some(ResourceName::UserResource),
            inner_content: Some(Value::Object(map)),
            external_content: None,
        }
    }

    #[test]
    fn test_allowed_scope_all_and_ids_serde_and_allows() {
        let rule_all: AllowedRule =
            serde_json::from_str(r#"{"actions": "all", "resources": "ALL"}"#).unwrap();
        assert_eq!(rule_all.actions, AllowedScope::All);
        assert_eq!(rule_all.resources, AllowedScope::All);
        assert!(rule_all.allows("create", "BillResource"));
        assert!(rule_all.allows("99", "42"));

        let serialized = serde_json::to_value(&rule_all).unwrap();
        assert_eq!(serialized, serde_json::json!({"actions": "all", "resources": "all"}));

        let rule_ids: AllowedRule = serde_json::from_str(
            r#"{"actions": ["create", 10], "resources": ["BillResource", 24]}"#,
        )
        .unwrap();
        assert!(rule_ids.allows("CREATE", "billresource"));
        assert!(rule_ids.allows("10", "24"));
        assert!(!rule_ids.allows("delete", "24"));
        assert!(!rule_ids.allows("10", "VehicleResource"));

        let rule_arr_all: AllowedRule =
            serde_json::from_str(r#"{"actions": ["all"], "resources": ["*"]}"#).unwrap();
        assert_eq!(rule_arr_all.actions, AllowedScope::All);
        assert_eq!(rule_arr_all.resources, AllowedScope::All);

        let bad = serde_json::from_str::<AllowedRule>(r#"{"actions": "create", "resources": "all"}"#);
        assert!(bad.is_err());
    }

    #[test]
    fn test_permission_inner_content_json_roundtrip_and_is_allowed() {
        let json_payload = serde_json::json!({
            "userId": 12,
            "userRole": "Operator",
            "userPermissions": {
                "canCancelReceipt": false,
                "maxDiscount": 500
            },
            "sthaniyaBranch": "Loadboard sthaniya",
            "Allowed": [
                {
                    "actions": ["create", "read"],
                    "resources": ["BillResource", 105]
                },
                {
                    "actions": "all",
                    "resources": ["VehicleResource"]
                }
            ]
        });

        let inner: PermissionInnerContent = serde_json::from_value(json_payload.clone()).unwrap();
        assert_eq!(inner.user_id, 12);
        assert_eq!(inner.user_role, Roles::Operator);
        assert_eq!(inner.sthaniya_branch.as_deref(), Some("Loadboard sthaniya"));
        assert_eq!(inner.user_permissions["maxDiscount"], 500);
        assert!(inner.is_allowed("create", "BillResource"));
        assert!(inner.is_allowed("read", "105"));
        assert!(inner.is_allowed("anything", "VehicleResource"));
        assert!(!inner.is_allowed("delete", "BillResource"));

        let roundtrip = serde_json::to_value(&inner).unwrap();
        assert_eq!(roundtrip["userId"], 12);
        assert_eq!(roundtrip["userRole"], "Operator");
        assert_eq!(roundtrip["sthaniyaBranch"], "Loadboard sthaniya");
        assert_eq!(roundtrip["Allowed"][1]["actions"], "all");
    }

    #[actix_web::test]
    async fn test_cache_load_from_db_excludes_permission_and_user_resources() {
        let mut state = MockDbState::default();
        state.seed(
            ResourceName::UserResource,
            make_user(Some(1), "u1@test.com", "pw", "Admin", Some("Loadboard sthaniya")),
        );
        state.seed(
            ResourceName::PermissionResource,
            PermissionResource::new(
                Some(1),
                PermissionInnerContent::new_for_user(
                    1,
                    Roles::Admin,
                    Some("Loadboard sthaniya".into()),
                    None,
                    None,
                ),
                None,
            )
            .to_database_resource(),
        );
        state.seed(ResourceName::SettingResource, make_setting("Loadboard sthaniya"));

        let (s_data, _) = create_mock_server_data_with_state(state).await;
        {
            let mut cache = s_data.cache.write().await;
            cache.load_from_db(&s_data.db).await;
        }

        let cache = s_data.cache.read().await;
        assert!(cache.cache.get(&ResourceName::UserResource).is_none());
        assert!(cache.cache.get(&ResourceName::PermissionResource).is_none());
        assert!(cache.read(ResourceName::SettingResource, 1).is_some());
    }

    #[actix_web::test]
    async fn test_strict_one_to_one_permission_mapping_enforced() {
        let mut state = MockDbState::default();
        state.seed(
            ResourceName::UserResource,
            make_user(Some(101), "strict@test.com", "pw", "Operator", Some("Loadboard sthaniya")),
        );
        let (s_data, _) = create_mock_server_data_with_state(state).await;

        // Non-existent user -> NotFound
        let missing_err = create_permission_in_db_strict(
            &s_data.db,
            PermissionInnerContent::new_for_user(999, Roles::User, None, None, None),
            None,
        )
        .await
        .expect_err("expected NotFound");
        assert!(matches!(missing_err, PermissionError::NotFound(_)));

        // First creation for userId 101 -> succeeds
        let first = create_permission_in_db_strict(
            &s_data.db,
            PermissionInnerContent::new_for_user(
                101,
                Roles::Operator,
                Some("Loadboard sthaniya".into()),
                None,
                None,
            ),
            None,
        )
        .await
        .expect("first 1:1 creation should succeed");
        assert_eq!(first.inner_content.user_id, 101);
        assert!(first.id.is_some());

        // Second creation for same userId 101 -> Conflict
        let dup_err = create_permission_in_db_strict(
            &s_data.db,
            PermissionInnerContent::new_for_user(
                101,
                Roles::Operator,
                Some("Loadboard sthaniya".into()),
                None,
                None,
            ),
            None,
        )
        .await
        .expect_err("expected Conflict for duplicate userId");
        assert!(matches!(dup_err, PermissionError::Conflict(_)));
    }

    #[actix_web::test]
    async fn test_new_user_creation_inherits_creators_sangh_branch() {
        let mut state = MockDbState::default();
        state.seed(
            ResourceName::UserResource,
            make_user(
                Some(201),
                "creator@test.com",
                "pw",
                "Admin",
                Some("Creator Sangh Branch"),
            ),
        );
        state.seed(
            ResourceName::SettingResource,
            make_setting("Global Setting Branch"),
        );
        let (s_data, db_state) = create_mock_server_data_with_state(state).await;

        let app = web_test::init_service(
            App::new()
                .app_data(s_data.clone())
                .wrap(crate::middleware::AuthMiddleware::with_roles(&[
                    Roles::Admin,
                    Roles::Operator,
                    Roles::User,
                ]))
                .route("/api/v1/create/{resource_name}", web::post().to(crate::actions::write)),
        )
        .await;

        let creator_token =
            auth::create_jwt_with_role_and_user_id("creator@test.com", &Roles::Admin, Some(201))
                .unwrap();

        let new_user_payload = serde_json::json!({
            "inner_content": {
                "email": "subuser@test.com",
                "password": "secret",
                "role": "Operator"
            }
        });

        let req = web_test::TestRequest::post()
            .uri("/api/v1/create/UserResource")
            .insert_header((AUTHORIZATION, format!("Bearer {creator_token}")))
            .set_json(&new_user_payload)
            .to_request();
        let resp = web_test::call_service(&app, req).await;
        assert_eq!(resp.status(), actix_web::http::StatusCode::OK);

        let created_user: DatabaseResource = web_test::read_body_json(resp).await;
        let new_user_id = created_user.id.expect("created user must have id");
        assert_eq!(
            created_user
                .inner_content
                .as_ref()
                .and_then(|v| v.get("SanghBranch"))
                .and_then(Value::as_str),
            Some("Creator Sangh Branch")
        );

        let paired_perm = find_permission_by_user_id_in_db(&s_data.db, new_user_id)
            .await
            .unwrap()
            .expect("1:1 PermissionResource should be created in DB");
        assert_eq!(
            paired_perm.inner_content.sthaniya_branch.as_deref(),
            Some("Creator Sangh Branch")
        );
        assert_eq!(paired_perm.inner_content.user_role, Roles::Operator);

        // Must NOT be loaded into cache prior to the new user logging in!
        let cache = s_data.cache.read().await;
        assert!(!is_user_logged_in_cache(&cache, new_user_id));

        let guard = db_state.lock().await;
        assert_eq!(guard.tables.get("permissionresource").map(Vec::len), Some(1));
    }

    #[actix_web::test]
    async fn test_login_loads_permission_and_logout_destroys_it_and_revokes_token() {
        let mut state = MockDbState::default();
        state.seed(
            ResourceName::UserResource,
            make_user(
                Some(301),
                "session@test.com",
                "Secret@123",
                "Operator",
                Some("Loadboard sthaniya"),
            ),
        );
        state.seed(
            ResourceName::PermissionResource,
            PermissionResource::new(
                Some(50),
                PermissionInnerContent::new_for_user(
                    301,
                    Roles::Operator,
                    Some("Loadboard sthaniya".into()),
                    Some(serde_json::json!({ "canIssueBill": true })),
                    Some(vec![AllowedRule {
                        actions: AllowedScope::Ids(vec![ScopeId::Str("create".into())]),
                        resources: AllowedScope::Ids(vec![ScopeId::Str("BillResource".into())]),
                    }]),
                ),
                None,
            )
            .to_database_resource(),
        );

        let (s_data, _) = create_mock_server_data_with_state(state).await;

        // Before login: cache has no PermissionResource for 301
        {
            let cache = s_data.cache.read().await;
            assert!(get_active_permission_from_cache(&cache, 301).is_none());
        }

        let app = web_test::init_service(
            App::new()
                .app_data(s_data.clone())
                .service(
                    web::scope("/api/v1/auth")
                        .route("/login", web::post().to(crate::login::login))
                        .route("/logout", web::post().to(logout)),
                )
                .service(
                    web::scope("/api/v1/access")
                        .wrap(crate::middleware::AuthMiddleware::with_roles(&[
                            Roles::User,
                            Roles::Admin,
                            Roles::Operator,
                        ]))
                        .configure(configure_routes),
                ),
        )
        .await;

        // 1. Login
        let login_req = web_test::TestRequest::post()
            .uri("/api/v1/auth/login")
            .set_json(serde_json::json!({
                "email": "session@test.com",
                "password": "Secret@123"
            }))
            .to_request();
        let login_resp = web_test::call_service(&app, login_req).await;
        assert_eq!(login_resp.status(), actix_web::http::StatusCode::OK);
        let login_json: Value = web_test::read_body_json(login_resp).await;
        let token_1 = login_json["token"].as_str().unwrap().to_string();
        assert_eq!(login_json["permissionResource"]["inner_content"]["userId"], 301);

        // Cache now contains PermissionResource for 301
        {
            let cache = s_data.cache.read().await;
            let loaded = get_active_permission_from_cache(&cache, 301).expect("should be cached");
            assert!(loaded.inner_content.is_allowed("create", "BillResource"));
        }

        // 2. Access protected /me with active token
        let me_req = web_test::TestRequest::get()
            .uri("/api/v1/access/me")
            .insert_header((AUTHORIZATION, format!("Bearer {token_1}")))
            .to_request();
        let me_resp = web_test::call_service(&app, me_req).await;
        assert_eq!(me_resp.status(), actix_web::http::StatusCode::OK);
        let me_perm: PermissionResource = web_test::read_body_json(me_resp).await;
        assert_eq!(me_perm.inner_content.user_id, 301);

        // 3. Logout -> destroys cached PermissionResource & revokes token_1
        let logout_req = web_test::TestRequest::post()
            .uri("/api/v1/auth/logout")
            .insert_header((AUTHORIZATION, format!("Bearer {token_1}")))
            .to_request();
        let logout_resp = web_test::call_service(&app, logout_req).await;
        assert_eq!(logout_resp.status(), actix_web::http::StatusCode::OK);
        let logout_json: Value = web_test::read_body_json(logout_resp).await;
        assert_eq!(logout_json["permissionDestroyed"], true);

        // Cache no longer contains PermissionResource for 301
        {
            let cache = s_data.cache.read().await;
            assert!(get_active_permission_from_cache(&cache, 301).is_none());
        }

        // 4. Replaying revoked token_1 fails in AuthMiddleware (returns Err -> 401)
        let replay_req = web_test::TestRequest::get()
            .uri("/api/v1/access/me")
            .insert_header((AUTHORIZATION, format!("Bearer {token_1}")))
            .to_request();
        let replay_res = web_test::try_call_service(&app, replay_req).await;
        assert!(replay_res.is_err());

        // 5. Re-login issues a fresh token and reloads PermissionResource cleanly
        let login_req_2 = web_test::TestRequest::post()
            .uri("/api/v1/auth/login")
            .set_json(serde_json::json!({
                "email": "session@test.com",
                "password": "Secret@123"
            }))
            .to_request();
        let login_resp_2 = web_test::call_service(&app, login_req_2).await;
        assert_eq!(login_resp_2.status(), actix_web::http::StatusCode::OK);
        let login_json_2: Value = web_test::read_body_json(login_resp_2).await;
        let token_2 = login_json_2["token"].as_str().unwrap().to_string();
        assert_ne!(token_1, token_2);

        let me_req_2 = web_test::TestRequest::get()
            .uri("/api/v1/access/me")
            .insert_header((AUTHORIZATION, format!("Bearer {token_2}")))
            .to_request();
        let me_resp_2 = web_test::call_service(&app, me_req_2).await;
        assert_eq!(me_resp_2.status(), actix_web::http::StatusCode::OK);
    }

    #[actix_web::test]
    async fn test_user_update_and_delete_sync_paired_permission_resource() {
        let mut state = MockDbState::default();
        state.seed(
            ResourceName::UserResource,
            make_user(Some(401), "admin401@test.com", "pw", "Admin", Some("Branch A")),
        );
        state.seed(
            ResourceName::UserResource,
            make_user(Some(402), "target402@test.com", "pw", "User", Some("Branch A")),
        );
        state.seed(
            ResourceName::PermissionResource,
            PermissionResource::new(
                Some(60),
                PermissionInnerContent::new_for_user(
                    402,
                    Roles::User,
                    Some("Branch A".into()),
                    None,
                    None,
                ),
                None,
            )
            .to_database_resource(),
        );
        let (s_data, _) = create_mock_server_data_with_state(state).await;

        let admin_req = web_test::TestRequest::default().to_http_request();
        admin_req.extensions_mut().insert(auth::Claims {
            sub: "admin401@test.com".to_string(),
            exp: 9999999999,
            iat: 1000000000,
            role: Roles::Admin,
            user_id: Some(401),
            jti: None,
            role_name: None,
            tenant_id: None,
        });

        // Update logged-out user 402 -> DB PermissionResource syncs, cache stays empty
        let update_resp = crate::actions::update(
            web::Path::from((ResourceName::UserResource, 402)),
            web::Json(make_user(
                Some(402),
                "target402@test.com",
                "pw",
                "Operator",
                Some("Branch B"),
            )),
            admin_req.clone(),
            s_data.clone(),
        )
        .await;
        assert_eq!(update_resp.status(), actix_web::http::StatusCode::OK);

        let db_perm = find_permission_by_user_id_in_db(&s_data.db, 402)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(db_perm.inner_content.user_role, Roles::Operator);
        assert_eq!(db_perm.inner_content.sthaniya_branch.as_deref(), Some("Branch B"));
        {
            let cache = s_data.cache.read().await;
            assert!(!is_user_logged_in_cache(&cache, 402));
        }

        // Now log user 402 in, update again -> BOTH DB and cache update
        let user_402 = find_user_by_id_in_db(&s_data.db, 402)
            .await
            .unwrap()
            .unwrap();
        load_permission_on_login(&s_data, &user_402, &Roles::Operator)
            .await
            .unwrap();
        {
            let cache = s_data.cache.read().await;
            assert!(is_user_logged_in_cache(&cache, 402));
        }

        let update_resp_2 = crate::actions::update(
            web::Path::from((ResourceName::UserResource, 402)),
            web::Json(make_user(
                Some(402),
                "target402@test.com",
                "pw",
                "User",
                Some("Branch C"),
            )),
            admin_req.clone(),
            s_data.clone(),
        )
        .await;
        assert_eq!(update_resp_2.status(), actix_web::http::StatusCode::OK);
        {
            let cache = s_data.cache.read().await;
            let cached_perm = get_active_permission_from_cache(&cache, 402).unwrap();
            assert_eq!(cached_perm.inner_content.user_role, Roles::User);
            assert_eq!(
                cached_perm.inner_content.sthaniya_branch.as_deref(),
                Some("Branch C")
            );
        }

        // Delete user 402 -> deletes 1:1 PermissionResource from DB and cache
        let delete_resp = crate::actions::delete(
            web::Path::from((ResourceName::UserResource, 402)),
            s_data.clone(),
            admin_req,
        )
        .await;
        assert_eq!(delete_resp.status(), actix_web::http::StatusCode::OK);
        assert!(find_permission_by_user_id_in_db(&s_data.db, 402)
            .await
            .unwrap()
            .is_none());
        {
            let cache = s_data.cache.read().await;
            assert!(!is_user_logged_in_cache(&cache, 402));
        }
    }

    #[test]
    fn test_extract_sangh_branch_supports_nested_setting_resource_integrations_tqms() {
        let nested_setting = serde_json::json!({
            "integrations": {
                "tqms": {
                    "SanghBranch": "Loadboard sthaniya"
                }
            }
        });
        assert_eq!(
            extract_sangh_branch(&nested_setting).as_deref(),
            Some("Loadboard sthaniya")
        );
        assert_eq!(
            crate::settings::resolve_tqms_sangh_branch_from_value(&nested_setting).as_deref(),
            Some("Loadboard sthaniya")
        );

        let top_level_override = serde_json::json!({
            "sthaniyaBranch": "Top Branch",
            "integrations": {
                "tqms": {
                    "SanghBranch": "Loadboard sthaniya"
                }
            }
        });
        assert_eq!(
            extract_sangh_branch(&top_level_override).as_deref(),
            Some("Top Branch")
        );

        let blank_nested = serde_json::json!({
            "integrations": {
                "tqms": {
                    "SanghBranch": "   "
                }
            }
        });
        assert_eq!(extract_sangh_branch(&blank_nested), None);
    }

    #[actix_web::test]
    async fn test_permission_grant_and_update_emit_audit_trail_entries() {
        let mut state = test_helpers::MockDbState::default();
        state.seed(
            ResourceName::UserResource,
            make_user(
                Some(801),
                "target801@test.com",
                "pw",
                "User",
                Some("Loadboard sthaniya"),
            ),
        );
        let (s_data, _) = test_helpers::create_mock_server_data_with_state(state).await;

        let admin_email = format!("perm-audit-{}@test.com", crate::audit::generate_audit_id());
        let admin_req = actix_web::test::TestRequest::default().to_http_request();
        admin_req.extensions_mut().insert(crate::auth::Claims {
            sub: admin_email.clone(),
            exp: 9999999999,
            iat: 1000000000,
            role: Roles::Admin,
            user_id: Some(1),
            jti: None,
            role_name: None,
            tenant_id: None,
        });

        let create_resp = create_permission(
            web::Json(CreatePermissionRequest {
                user_id: 801,
                user_role: Some(Roles::User),
                sthaniya_branch: Some("Loadboard sthaniya".to_string()),
                user_permissions: Some(serde_json::json!({ "canBill": true })),
                allowed: Some(vec![AllowedRule {
                    actions: AllowedScope::Ids(vec![ScopeId::Str("read".to_string())]),
                    resources: AllowedScope::Ids(vec![ScopeId::Str("BillResource".to_string())]),
                }]),
                external_content: None,
            }),
            admin_req.clone(),
            s_data.clone(),
        )
        .await;
        assert_eq!(create_resp.status(), actix_web::http::StatusCode::CREATED);

        let upd_resp = update_permission_by_user_id(
            web::Path::from(801),
            web::Json(UpdatePermissionRequest {
                user_role: Some(Roles::Operator),
                user_permissions: None,
                sthaniya_branch: None,
                allowed: Some(vec![AllowedRule {
                    actions: AllowedScope::All,
                    resources: AllowedScope::All,
                }]),
                external_content: None,
            }),
            admin_req,
            s_data,
        )
        .await;
        assert_eq!(upd_resp.status(), actix_web::http::StatusCode::OK);

        crate::audit::flush_audit_trail().await;

        let page = crate::audit::global_audit_logger()
            .query(&crate::audit::AuditTrailQuery {
                actor: Some(admin_email),
                category: Some("ACCESS_CONTROL".to_string()),
                ..Default::default()
            })
            .unwrap();

        assert!(page.items.iter().any(|e| {
            e.action == crate::audit::AuditAction::PermissionGrant
                && e.resource_name.as_deref() == Some("PermissionResource")
        }));

        let upd_entry = page
            .items
            .iter()
            .find(|e| {
                e.action == crate::audit::AuditAction::PermissionUpdate
                    && e.resource_name.as_deref() == Some("PermissionResource")
            })
            .expect("PermissionUpdate audit entry");
        assert!(upd_entry.changes.iter().any(|c| c.path == "userRole"));
        assert!(upd_entry.changes.iter().any(|c| c.path == "Allowed"));
    }
}
