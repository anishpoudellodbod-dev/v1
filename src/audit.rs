use crate::resource::{DatabaseResource, ResourceName};
use actix_web::{web, HttpMessage, HttpRequest, HttpResponse};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
use tokio::sync::mpsc;

pub const AUDIT_SCHEMA_VERSION: u32 = 1;
const MAX_IN_MEMORY_AUDIT_ENTRIES: usize = 50_000;
const DB_PERSIST_BATCH_SIZE: usize = 32;
const MAX_DRAIN_PER_CYCLE: usize = 256;
const MAX_DIFF_DEPTH: usize = 12;
const MAX_DIFF_CHANGES: usize = 200;
const DEFAULT_QUERY_LIMIT: usize = 100;
const MAX_QUERY_LIMIT: usize = 1000;
const MAX_USER_AGENT_LEN: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AuditCategory {
    Resource,
    Billing,
    Auth,
    AccessControl,
    Settings,
    Notification,
    Integration,
}

impl AuditCategory {
    /// Maps a `ResourceName` to the domain `AuditCategory` used for generic resource mutations.
    pub fn for_resource(resource_name: ResourceName) -> Self {
        match resource_name {
            ResourceName::BillResource => Self::Billing,
            ResourceName::SettingResource => Self::Settings,
            ResourceName::UserResource | ResourceName::PermissionResource => Self::AccessControl,
            _ => Self::Resource,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Resource => "RESOURCE",
            Self::Billing => "BILLING",
            Self::Auth => "AUTH",
            Self::AccessControl => "ACCESS_CONTROL",
            Self::Settings => "SETTINGS",
            Self::Notification => "NOTIFICATION",
            Self::Integration => "INTEGRATION",
        }
    }

    /// Parses a category string case-insensitively, tolerating underscores, hyphens, or spaces.
    pub fn from_str_loose(s: &str) -> Option<Self> {
        let norm: String = s
            .trim()
            .chars()
            .filter(|c| !matches!(c, '_' | '-' | ' '))
            .flat_map(|c| c.to_lowercase())
            .collect();
        match norm.as_str() {
            "resource" => Some(Self::Resource),
            "billing" => Some(Self::Billing),
            "auth" => Some(Self::Auth),
            "accesscontrol" => Some(Self::AccessControl),
            "settings" => Some(Self::Settings),
            "notification" => Some(Self::Notification),
            "integration" => Some(Self::Integration),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AuditAction {
    Create,
    Update,
    Delete,
    Modify,
    Rollback,
    Payment,
    Login,
    Logout,
    PermissionGrant,
    PermissionUpdate,
    SmsSend,
    AccessDenied,
}

impl AuditAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Create => "CREATE",
            Self::Update => "UPDATE",
            Self::Delete => "DELETE",
            Self::Modify => "MODIFY",
            Self::Rollback => "ROLLBACK",
            Self::Payment => "PAYMENT",
            Self::Login => "LOGIN",
            Self::Logout => "LOGOUT",
            Self::PermissionGrant => "PERMISSION_GRANT",
            Self::PermissionUpdate => "PERMISSION_UPDATE",
            Self::SmsSend => "SMS_SEND",
            Self::AccessDenied => "ACCESS_DENIED",
        }
    }

    /// Parses an action string case-insensitively, tolerating underscores, hyphens, or spaces.
    pub fn from_str_loose(s: &str) -> Option<Self> {
        let norm: String = s
            .trim()
            .chars()
            .filter(|c| !matches!(c, '_' | '-' | ' '))
            .flat_map(|c| c.to_lowercase())
            .collect();
        match norm.as_str() {
            "create" => Some(Self::Create),
            "update" => Some(Self::Update),
            "delete" => Some(Self::Delete),
            "modify" => Some(Self::Modify),
            "rollback" => Some(Self::Rollback),
            "payment" => Some(Self::Payment),
            "login" => Some(Self::Login),
            "logout" => Some(Self::Logout),
            "permissiongrant" => Some(Self::PermissionGrant),
            "permissionupdate" => Some(Self::PermissionUpdate),
            "smssend" => Some(Self::SmsSend),
            "accessdenied" => Some(Self::AccessDenied),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AuditOutcome {
    Success,
    Denied,
    Failure,
}

impl AuditOutcome {
    pub fn from_status_code(status_code: u16) -> Self {
        if (200..300).contains(&status_code) {
            Self::Success
        } else if status_code == 401 || status_code == 403 {
            Self::Denied
        } else {
            Self::Failure
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "SUCCESS",
            Self::Denied => "DENIED",
            Self::Failure => "FAILURE",
        }
    }

    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "success" => Some(Self::Success),
            "denied" => Some(Self::Denied),
            "failure" => Some(Self::Failure),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FieldChangeType {
    Added,
    Removed,
    Modified,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldChange {
    pub path: String,
    pub change_type: FieldChangeType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_value: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_value: Option<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditActor {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditRequestContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditTrailEntry {
    pub schema_version: u32,
    pub audit_id: String,
    pub timestamp: String,
    pub category: AuditCategory,
    pub action: AuditAction,
    pub outcome: AuditOutcome,
    pub status_code: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<String>,
    pub actor: AuditActor,
    pub request: AuditRequestContext,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_state: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_state: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<FieldChange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

static AUDIT_ID_COUNTER: OnceLock<AtomicU64> = OnceLock::new();

fn audit_id_counter() -> &'static AtomicU64 {
    AUDIT_ID_COUNTER.get_or_init(|| {
        let initial = u64::try_from(Utc::now().timestamp_micros())
            .unwrap_or(1000)
            .max(1000);
        AtomicU64::new(initial)
    })
}

pub fn generate_audit_id() -> String {
    let seq = audit_id_counter().fetch_add(1, Ordering::Relaxed);
    format!("AUD-{}", seq)
}

fn advance_audit_counter_past(entries: &[AuditTrailEntry]) {
    let max_existing = entries
        .iter()
        .filter_map(|e| e.audit_id.strip_prefix("AUD-"))
        .filter_map(|num| num.parse::<u64>().ok())
        .max();
    if let Some(max_id) = max_existing {
        audit_id_counter().fetch_max(max_id.saturating_add(1), Ordering::Relaxed);
    }
}

/// Returns true if a JSON field name represents a credential or secret that must be redacted.
pub fn is_sensitive_key(key: &str) -> bool {
    let lower = key.trim().to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "password"
            | "passwd"
            | "token"
            | "secret"
            | "apikey"
            | "api_key"
            | "apisecret"
            | "api_secret"
            | "authorization"
            | "cookie"
            | "private_key"
            | "privatekey"
            | "access_token"
            | "refresh_token"
            | "client_secret"
    ) || lower.ends_with("password")
        || lower.ends_with("secret")
        || lower.ends_with("token")
        || lower.ends_with("apikey")
        || lower.ends_with("api_key")
}

/// Recursively redacts sensitive fields (`password`, `token`, `secret`, `apiKey`, etc.) in JSON.
pub fn sanitize_audit_value(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut sanitized = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                if is_sensitive_key(k) {
                    sanitized.insert(k.clone(), Value::String("[REDACTED]".to_string()));
                } else {
                    sanitized.insert(k.clone(), sanitize_audit_value(v));
                }
            }
            Value::Object(sanitized)
        }
        Value::Array(items) => Value::Array(items.iter().map(sanitize_audit_value).collect()),
        _ => value.clone(),
    }
}

/// Computes field-level differences between `before` and `after` JSON values.
pub fn compute_json_diff(before: Option<&Value>, after: Option<&Value>) -> Vec<FieldChange> {
    let (Some(b), Some(a)) = (before, after) else {
        return Vec::new();
    };
    let mut changes = Vec::new();
    diff_recursive("", b, a, 0, &mut changes);
    changes
}

fn diff_recursive(
    prefix: &str,
    before: &Value,
    after: &Value,
    depth: usize,
    out: &mut Vec<FieldChange>,
) {
    if out.len() >= MAX_DIFF_CHANGES || before == after {
        return;
    }

    if let (Value::Object(b_map), Value::Object(a_map)) = (before, after) {
        if depth < MAX_DIFF_DEPTH {
            let mut keys: BTreeSet<&str> = BTreeSet::new();
            for k in b_map.keys() {
                keys.insert(k.as_str());
            }
            for k in a_map.keys() {
                keys.insert(k.as_str());
            }

            for k in keys {
                if out.len() >= MAX_DIFF_CHANGES {
                    break;
                }
                let path = if prefix.is_empty() {
                    k.to_string()
                } else {
                    format!("{prefix}.{k}")
                };
                match (b_map.get(k), a_map.get(k)) {
                    (Some(bv), Some(av)) => diff_recursive(&path, bv, av, depth + 1, out),
                    (None, Some(av)) => out.push(FieldChange {
                        path,
                        change_type: FieldChangeType::Added,
                        old_value: None,
                        new_value: Some(av.clone()),
                    }),
                    (Some(bv), None) => out.push(FieldChange {
                        path,
                        change_type: FieldChangeType::Removed,
                        old_value: Some(bv.clone()),
                        new_value: None,
                    }),
                    (None, None) => {}
                }
            }
            return;
        }
    }

    let path = if prefix.is_empty() {
        "$".to_string()
    } else {
        prefix.to_string()
    };
    out.push(FieldChange {
        path,
        change_type: FieldChangeType::Modified,
        old_value: Some(before.clone()),
        new_value: Some(after.clone()),
    });
}

fn redact_field_change(change: &mut FieldChange) {
    let path_is_sensitive = change.path.split('.').any(is_sensitive_key);
    if path_is_sensitive {
        if change.old_value.is_some() {
            change.old_value = Some(Value::String("[REDACTED]".to_string()));
        }
        if change.new_value.is_some() {
            change.new_value = Some(Value::String("[REDACTED]".to_string()));
        }
    } else {
        change.old_value = change.old_value.as_ref().map(sanitize_audit_value);
        change.new_value = change.new_value.as_ref().map(sanitize_audit_value);
    }
}

/// Computes diff before redaction, fills inferred branch metadata, and redacts sensitive fields
/// in `changes`, `before_state`, `after_state`, and `metadata`.
pub fn prepare_and_sanitize_entry(entry: &mut AuditTrailEntry) {
    if entry.changes.is_empty() {
        entry.changes = compute_json_diff(entry.before_state.as_ref(), entry.after_state.as_ref());
    }

    if entry.actor.branch.is_none() {
        entry.actor.branch = entry
            .after_state
            .as_ref()
            .and_then(crate::access::extract_sangh_branch)
            .or_else(|| {
                entry
                    .before_state
                    .as_ref()
                    .and_then(crate::access::extract_sangh_branch)
            });
    }

    for change in &mut entry.changes {
        redact_field_change(change);
    }

    entry.before_state = entry.before_state.as_ref().map(sanitize_audit_value);
    entry.after_state = entry.after_state.as_ref().map(sanitize_audit_value);
    entry.metadata = entry.metadata.as_ref().map(sanitize_audit_value);
}

fn extract_header_trimmed(req: &HttpRequest, name: &str) -> Option<String> {
    req.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

fn extract_client_ip(req: &HttpRequest) -> Option<String> {
    if let Some(xff) = extract_header_trimmed(req, "X-Forwarded-For") {
        if let Some(first) = xff.split(',').next().map(str::trim).filter(|s| !s.is_empty()) {
            return Some(first.to_string());
        }
    }
    if let Some(xri) = extract_header_trimmed(req, "X-Real-IP") {
        return Some(xri);
    }
    req.connection_info()
        .realip_remote_addr()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .or_else(|| req.peer_addr().map(|addr| addr.ip().to_string()))
}

/// Synchronously extracts actor identity and network metadata from an `HttpRequest` without holding
/// any `Ref` guards across `.await` points.
pub fn extract_actor_from_req(
    req: Option<&HttpRequest>,
    branch_override: Option<String>,
) -> AuditActor {
    let mut actor = AuditActor {
        branch: branch_override
            .map(|b| b.trim().to_string())
            .filter(|b| !b.is_empty()),
        ..Default::default()
    };

    let Some(r) = req else {
        return actor;
    };

    if let Some(claims) = r.extensions().get::<crate::auth::Claims>() {
        actor.email = Some(claims.sub.clone());
        actor.user_id = claims.user_id;
        actor.role = Some(String::from(claims.role.clone()));
    }

    actor.ip_address = extract_client_ip(r);
    actor.user_agent = extract_header_trimmed(r, "User-Agent").map(|ua| {
        if ua.chars().count() > MAX_USER_AGENT_LEN {
            ua.chars().take(MAX_USER_AGENT_LEN).collect()
        } else {
            ua
        }
    });

    actor
}

/// Synchronously extracts HTTP request context (`method`, `path`, `request_id`, `duration_ms`).
pub fn extract_request_context(
    req: Option<&HttpRequest>,
    started_at: Option<std::time::Instant>,
    request_id_override: Option<String>,
    fallback_method: Option<&str>,
    fallback_path: Option<&str>,
) -> AuditRequestContext {
    let request_id = request_id_override
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            req.and_then(|r| {
                extract_header_trimmed(r, "X-Request-Id")
                    .or_else(|| extract_header_trimmed(r, "X-Correlation-ID"))
            })
        });

    let method = req
        .map(|r| r.method().to_string())
        .or_else(|| fallback_method.map(String::from));
    let path = req
        .map(|r| r.path().to_string())
        .or_else(|| fallback_path.map(String::from));

    AuditRequestContext {
        method,
        path,
        request_id,
        duration_ms: started_at.map(|s| s.elapsed().as_millis() as u64),
    }
}

#[derive(Clone)]
pub struct AuditTrailDbSink {
    pub db: Arc<crate::db::Database>,
    pub cache: Option<Arc<tokio::sync::RwLock<crate::cache::Cache>>>,
}

enum AuditMsg {
    Entry(AuditTrailEntry),
    SetDbSink(Option<AuditTrailDbSink>),
    Flush(std::sync::mpsc::SyncSender<()>),
}

async fn persist_audit_batch_to_db(sink: &AuditTrailDbSink, batch: &[AuditTrailEntry]) {
    let now_str = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut draft_resources: Vec<DatabaseResource> = Vec::with_capacity(batch.len());

    for entry in batch {
        match serde_json::to_value(entry) {
            Ok(inner_val) => draft_resources.push(DatabaseResource {
                id: None,
                resource_name: Some(ResourceName::AuditTrailResource),
                inner_content: Some(inner_val),
                external_content: Some(serde_json::json!({
                    "createdTime": now_str,
                    "schemaVersion": entry.schema_version,
                    "category": entry.category.as_str(),
                    "action": entry.action.as_str(),
                    "outcome": entry.outcome.as_str(),
                })),
            }),
            Err(err) => {
                eprintln!(
                    "AsyncAuditTrailLogger DB serialization error for {}: {:?}",
                    entry.audit_id, err
                );
            }
        }
    }

    for chunk in draft_resources.chunks(DB_PERSIST_BATCH_SIZE) {
        let chunk_vec = chunk.to_vec();
        match sink
            .db
            .batch_create_resources(ResourceName::AuditTrailResource, chunk_vec.clone())
            .await
        {
            Ok(ids) => {
                if let Some(ref cache_lock) = sink.cache {
                    let mut cache = cache_lock.write().await;
                    for (id, mut res) in ids.into_iter().zip(chunk_vec.into_iter()) {
                        res.id = Some(id);
                        res.resource_name = Some(ResourceName::AuditTrailResource);
                        cache.write(ResourceName::AuditTrailResource, id, res);
                    }
                    cache.mark_fully_loaded(ResourceName::AuditTrailResource);
                }
            }
            Err(err) => {
                eprintln!(
                    "AsyncAuditTrailLogger batch DB persist failed ({:?}), retrying individually",
                    err
                );
                for mut res in chunk_vec {
                    match sink
                        .db
                        .create_resource(ResourceName::AuditTrailResource, res.clone())
                        .await
                    {
                        Ok(id) => {
                            res.id = Some(id);
                            res.resource_name = Some(ResourceName::AuditTrailResource);
                            if let Some(ref cache_lock) = sink.cache {
                                let mut cache = cache_lock.write().await;
                                cache.write(ResourceName::AuditTrailResource, id, res);
                            }
                        }
                        Err(item_err) => {
                            eprintln!(
                                "AsyncAuditTrailLogger single DB persist failed: {:?}",
                                item_err
                            );
                        }
                    }
                }
                if let Some(ref cache_lock) = sink.cache {
                    cache_lock
                        .write()
                        .await
                        .mark_fully_loaded(ResourceName::AuditTrailResource);
                }
            }
        }
    }
}

async fn process_audit_batch(
    batch: &mut [AuditTrailEntry],
    worker_store: &RwLock<Vec<AuditTrailEntry>>,
    active_sink: Option<&AuditTrailDbSink>,
) {
    for entry in batch.iter_mut() {
        prepare_and_sanitize_entry(entry);
    }

    {
        let mut guard = worker_store.write().unwrap_or_else(|e| e.into_inner());
        guard.extend(batch.iter().cloned());
        if guard.len() > MAX_IN_MEMORY_AUDIT_ENTRIES {
            let excess = guard.len() - MAX_IN_MEMORY_AUDIT_ENTRIES;
            guard.drain(..excess);
        }
    }

    if let Some(sink) = active_sink {
        persist_audit_batch_to_db(sink, batch).await;
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditTrailQuery {
    pub category: Option<String>,
    pub action: Option<String>,
    pub outcome: Option<String>,
    pub resource: Option<String>,
    pub resource_id: Option<String>,
    pub actor: Option<String>,
    pub role: Option<String>,
    pub branch: Option<String>,
    pub search: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditTrailPage {
    pub total: usize,
    pub limit: usize,
    pub offset: usize,
    pub items: Vec<AuditTrailEntry>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityHistoryQuery {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub order: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityAuditHistory {
    pub resource_name: String,
    pub resource_id: String,
    pub total: usize,
    pub limit: usize,
    pub offset: usize,
    pub items: Vec<AuditTrailEntry>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditSummaryQuery {
    pub from: Option<String>,
    pub to: Option<String>,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditTrailSummary {
    pub total_entries: usize,
    pub by_category: BTreeMap<String, usize>,
    pub by_action: BTreeMap<String, usize>,
    pub by_outcome: BTreeMap<String, usize>,
    pub by_resource: BTreeMap<String, usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_timestamp: Option<String>,
}

#[derive(Debug, Clone)]
struct ParsedAuditFilter {
    category: Option<AuditCategory>,
    action: Option<AuditAction>,
    outcome: Option<AuditOutcome>,
    resource: Option<String>,
    resource_id: Option<String>,
    actor: Option<String>,
    role: Option<String>,
    branch: Option<String>,
    search_lower: Option<String>,
    from: Option<String>,
    to: Option<String>,
}

fn non_empty_trimmed(opt: &Option<String>) -> Option<String> {
    opt.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

impl ParsedAuditFilter {
    fn from_query(q: &AuditTrailQuery) -> Result<Self, String> {
        let category = match non_empty_trimmed(&q.category) {
            Some(raw) => Some(
                AuditCategory::from_str_loose(&raw)
                    .ok_or_else(|| format!("Invalid category filter: {}", raw))?,
            ),
            None => None,
        };
        let action = match non_empty_trimmed(&q.action) {
            Some(raw) => Some(
                AuditAction::from_str_loose(&raw)
                    .ok_or_else(|| format!("Invalid action filter: {}", raw))?,
            ),
            None => None,
        };
        let outcome = match non_empty_trimmed(&q.outcome) {
            Some(raw) => Some(
                AuditOutcome::from_str_loose(&raw)
                    .ok_or_else(|| format!("Invalid outcome filter: {}", raw))?,
            ),
            None => None,
        };
        let resource = non_empty_trimmed(&q.resource).map(|r| {
            ResourceName::from_str_case_insensitive(&r)
                .map(|rn| rn.to_string())
                .unwrap_or(r)
        });

        Ok(Self {
            category,
            action,
            outcome,
            resource,
            resource_id: non_empty_trimmed(&q.resource_id),
            actor: non_empty_trimmed(&q.actor),
            role: non_empty_trimmed(&q.role),
            branch: non_empty_trimmed(&q.branch),
            search_lower: non_empty_trimmed(&q.search).map(|s| s.to_lowercase()),
            from: non_empty_trimmed(&q.from),
            to: non_empty_trimmed(&q.to),
        })
    }

    fn matches(&self, entry: &AuditTrailEntry) -> bool {
        if let Some(cat) = self.category {
            if entry.category != cat {
                return false;
            }
        }
        if let Some(act) = self.action {
            if entry.action != act {
                return false;
            }
        }
        if let Some(out) = self.outcome {
            if entry.outcome != out {
                return false;
            }
        }
        if let Some(ref res) = self.resource {
            if !entry
                .resource_name
                .as_deref()
                .is_some_and(|r| r.eq_ignore_ascii_case(res))
            {
                return false;
            }
        }
        if let Some(ref rid) = self.resource_id {
            if entry.resource_id.as_deref() != Some(rid.as_str()) {
                return false;
            }
        }
        if let Some(ref actor_q) = self.actor {
            let matches_email = entry
                .actor
                .email
                .as_deref()
                .is_some_and(|e| e.eq_ignore_ascii_case(actor_q));
            let matches_uid = entry
                .actor
                .user_id
                .is_some_and(|id| id.to_string() == *actor_q);
            if !matches_email && !matches_uid {
                return false;
            }
        }
        if let Some(ref role_q) = self.role {
            if !entry
                .actor
                .role
                .as_deref()
                .is_some_and(|r| r.eq_ignore_ascii_case(role_q))
            {
                return false;
            }
        }
        if let Some(ref branch_q) = self.branch {
            if !entry
                .actor
                .branch
                .as_deref()
                .is_some_and(|b| b.eq_ignore_ascii_case(branch_q))
            {
                return false;
            }
        }
        if !timestamp_in_range(&entry.timestamp, self.from.as_deref(), self.to.as_deref()) {
            return false;
        }
        if let Some(ref needle) = self.search_lower {
            let hit = entry.audit_id.to_lowercase().contains(needle)
                || entry.summary.to_lowercase().contains(needle)
                || entry
                    .resource_name
                    .as_deref()
                    .is_some_and(|s| s.to_lowercase().contains(needle))
                || entry
                    .resource_id
                    .as_deref()
                    .is_some_and(|s| s.to_lowercase().contains(needle))
                || entry
                    .actor
                    .email
                    .as_deref()
                    .is_some_and(|s| s.to_lowercase().contains(needle))
                || entry
                    .actor
                    .branch
                    .as_deref()
                    .is_some_and(|s| s.to_lowercase().contains(needle))
                || entry
                    .request
                    .request_id
                    .as_deref()
                    .is_some_and(|s| s.to_lowercase().contains(needle));
            if !hit {
                return false;
            }
        }
        true
    }
}

fn timestamp_in_range(ts: &str, from: Option<&str>, to: Option<&str>) -> bool {
    let parsed_ts = chrono::DateTime::parse_from_rfc3339(ts).ok();

    if let Some(from_raw) = from {
        let ok = match (parsed_ts, chrono::DateTime::parse_from_rfc3339(from_raw).ok()) {
            (Some(t), Some(f)) => t >= f,
            _ => ts >= from_raw,
        };
        if !ok {
            return false;
        }
    }

    if let Some(to_raw) = to {
        let ok = match (parsed_ts, chrono::DateTime::parse_from_rfc3339(to_raw).ok()) {
            (Some(t), Some(u)) => t <= u,
            _ => ts <= to_raw,
        };
        if !ok {
            return false;
        }
    }

    true
}

/// Asynchronous lock-free audit trail logger backed by an unbounded MPSC channel and persistent
/// `ResourceName::AuditTrailResource` storage.
pub struct AsyncAuditTrailLogger {
    tx: mpsc::UnboundedSender<AuditMsg>,
    store: Arc<RwLock<Vec<AuditTrailEntry>>>,
}

impl AsyncAuditTrailLogger {
    pub fn spawn() -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<AuditMsg>();
        let store = Arc::new(RwLock::new(Vec::new()));
        let worker_store = store.clone();

        std::thread::Builder::new()
            .name("async-audit-trail".to_string())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("Failed to create async-audit-trail Tokio runtime");

                rt.block_on(async move {
                    let mut active_sink: Option<AuditTrailDbSink> = None;

                    while let Some(first) = rx.recv().await {
                        let mut batch: Vec<AuditTrailEntry> = Vec::new();
                        let mut flush_waiters: Vec<std::sync::mpsc::SyncSender<()>> = Vec::new();
                        let mut next_sink: Option<Option<AuditTrailDbSink>> = None;

                        match first {
                            AuditMsg::Entry(e) => batch.push(e),
                            AuditMsg::Flush(waiter) => flush_waiters.push(waiter),
                            AuditMsg::SetDbSink(new_sink) => next_sink = Some(new_sink),
                        }

                        if next_sink.is_none() {
                            for _ in 0..MAX_DRAIN_PER_CYCLE {
                                match rx.try_recv() {
                                    Ok(AuditMsg::Entry(e)) => batch.push(e),
                                    Ok(AuditMsg::Flush(waiter)) => flush_waiters.push(waiter),
                                    Ok(AuditMsg::SetDbSink(new_sink)) => {
                                        next_sink = Some(new_sink);
                                        break;
                                    }
                                    Err(_) => break,
                                }
                            }
                        }

                        if !batch.is_empty() {
                            process_audit_batch(&mut batch, &worker_store, active_sink.as_ref())
                                .await;
                        }

                        if let Some(new_sink) = next_sink {
                            active_sink = new_sink;
                        }

                        for waiter in flush_waiters {
                            let _ = waiter.send(());
                        }
                    }
                });
            })
            .expect("Failed to spawn async-audit-trail thread");

        Self { tx, store }
    }

    pub async fn spawn_persistent(
        db: Arc<crate::db::Database>,
        cache: Option<Arc<tokio::sync::RwLock<crate::cache::Cache>>>,
    ) -> Self {
        let logger = Self::spawn();
        logger.attach_and_hydrate(db, cache).await;
        logger
    }

    /// Hydrates historical `AuditTrailResource` rows from PostgreSQL, advances `AUDIT_ID_COUNTER`,
    /// syncs `cache`, and attaches the DB persistence sink.
    pub async fn attach_and_hydrate(
        &self,
        db: Arc<crate::db::Database>,
        cache: Option<Arc<tokio::sync::RwLock<crate::cache::Cache>>>,
    ) {
        self.flush().await;

        match db.read_all_resource(ResourceName::AuditTrailResource).await {
            Ok(mut rows) => {
                rows.sort_by_key(|r| r.id.unwrap_or(0));

                let mut persisted: Vec<AuditTrailEntry> = Vec::with_capacity(rows.len());
                for row in &rows {
                    if let Some(ref inner) = row.inner_content {
                        if let Ok(entry) = serde_json::from_value::<AuditTrailEntry>(inner.clone()) {
                            persisted.push(entry);
                        }
                    }
                }

                advance_audit_counter_past(&persisted);

                {
                    let mut seen_ids: HashSet<String> =
                        persisted.iter().map(|e| e.audit_id.clone()).collect();
                    let mut guard = self.store.write().unwrap_or_else(|e| e.into_inner());
                    let existing = std::mem::take(&mut *guard);
                    let mut merged = persisted;
                    for entry in existing {
                        if seen_ids.insert(entry.audit_id.clone()) {
                            merged.push(entry);
                        }
                    }
                    if merged.len() > MAX_IN_MEMORY_AUDIT_ENTRIES {
                        let excess = merged.len() - MAX_IN_MEMORY_AUDIT_ENTRIES;
                        merged.drain(..excess);
                    }
                    *guard = merged;
                }

                if let Some(ref cache_lock) = cache {
                    let mut c = cache_lock.write().await;
                    for mut row in rows {
                        if let Some(id) = row.id {
                            row.resource_name = Some(ResourceName::AuditTrailResource);
                            c.write(ResourceName::AuditTrailResource, id, row);
                        }
                    }
                    c.mark_fully_loaded(ResourceName::AuditTrailResource);
                }
            }
            Err(err) => {
                eprintln!(
                    "AsyncAuditTrailLogger failed to hydrate AuditTrailResource from DB: {:?}",
                    err
                );
            }
        }

        let _ = self.tx.send(AuditMsg::SetDbSink(Some(AuditTrailDbSink {
            db,
            cache,
        })));
        self.flush().await;
    }

    /// Lock-free, non-blocking enqueue onto the background audit worker channel.
    pub fn record(&self, entry: AuditTrailEntry) {
        let _ = self.tx.send(AuditMsg::Entry(entry));
    }

    pub async fn flush(&self) {
        let tx = self.tx.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let (waiter_tx, waiter_rx) = std::sync::mpsc::sync_channel::<()>(1);
            if tx.send(AuditMsg::Flush(waiter_tx)).is_ok() {
                let _ = waiter_rx.recv();
            }
        })
        .await;
    }

    pub fn all(&self) -> Vec<AuditTrailEntry> {
        self.store
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Queries audit entries (newest first) with structured filters and pagination.
    pub fn query(&self, params: &AuditTrailQuery) -> Result<AuditTrailPage, String> {
        let filter = ParsedAuditFilter::from_query(params)?;
        let limit = params
            .limit
            .unwrap_or(DEFAULT_QUERY_LIMIT)
            .min(MAX_QUERY_LIMIT);
        let offset = params.offset.unwrap_or(0);

        let guard = self.store.read().unwrap_or_else(|e| e.into_inner());
        let matched: Vec<AuditTrailEntry> = guard
            .iter()
            .rev()
            .filter(|e| filter.matches(e))
            .cloned()
            .collect();
        let total = matched.len();
        let items = matched.into_iter().skip(offset).take(limit).collect();

        Ok(AuditTrailPage {
            total,
            limit,
            offset,
            items,
        })
    }

    /// Returns the audit history timeline for a specific `(resource_name, resource_id)` entity.
    pub fn entity_history(
        &self,
        resource_name: ResourceName,
        resource_id: &str,
        limit: usize,
        offset: usize,
        ascending: bool,
    ) -> EntityAuditHistory {
        let canonical_name = resource_name.to_string();
        let target_id = resource_id.trim().to_string();
        let effective_limit = limit.min(MAX_QUERY_LIMIT);

        let guard = self.store.read().unwrap_or_else(|e| e.into_inner());
        let matches_entity = |e: &&AuditTrailEntry| {
            e.resource_name
                .as_deref()
                .is_some_and(|r| r.eq_ignore_ascii_case(&canonical_name))
                && e.resource_id.as_deref() == Some(target_id.as_str())
        };

        let matched: Vec<AuditTrailEntry> = if ascending {
            guard.iter().filter(matches_entity).cloned().collect()
        } else {
            guard.iter().rev().filter(matches_entity).cloned().collect()
        };
        let total = matched.len();
        let items = matched
            .into_iter()
            .skip(offset)
            .take(effective_limit)
            .collect();

        EntityAuditHistory {
            resource_name: canonical_name,
            resource_id: target_id,
            total,
            limit: effective_limit,
            offset,
            items,
        }
    }

    /// Computes aggregate counts across matching audit trail entries.
    pub fn summarize(&self, params: &AuditSummaryQuery) -> AuditTrailSummary {
        let from = non_empty_trimmed(&params.from);
        let to = non_empty_trimmed(&params.to);
        let branch = non_empty_trimmed(&params.branch);

        let mut total_entries = 0usize;
        let mut by_category: BTreeMap<String, usize> = BTreeMap::new();
        let mut by_action: BTreeMap<String, usize> = BTreeMap::new();
        let mut by_outcome: BTreeMap<String, usize> = BTreeMap::new();
        let mut by_resource: BTreeMap<String, usize> = BTreeMap::new();
        let mut latest_timestamp: Option<String> = None;

        let guard = self.store.read().unwrap_or_else(|e| e.into_inner());
        for entry in guard.iter() {
            if let Some(ref bq) = branch {
                if !entry
                    .actor
                    .branch
                    .as_deref()
                    .is_some_and(|b| b.eq_ignore_ascii_case(bq))
                {
                    continue;
                }
            }
            if !timestamp_in_range(&entry.timestamp, from.as_deref(), to.as_deref()) {
                continue;
            }
            total_entries += 1;
            *by_category
                .entry(entry.category.as_str().to_string())
                .or_insert(0) += 1;
            *by_action
                .entry(entry.action.as_str().to_string())
                .or_insert(0) += 1;
            *by_outcome
                .entry(entry.outcome.as_str().to_string())
                .or_insert(0) += 1;
            if let Some(ref res) = entry.resource_name {
                *by_resource.entry(res.clone()).or_insert(0) += 1;
            }
            latest_timestamp = Some(entry.timestamp.clone());
        }

        AuditTrailSummary {
            total_entries,
            by_category,
            by_action,
            by_outcome,
            by_resource,
            latest_timestamp,
        }
    }
}

static GLOBAL_AUDIT_LOGGER: OnceLock<AsyncAuditTrailLogger> = OnceLock::new();

pub fn global_audit_logger() -> &'static AsyncAuditTrailLogger {
    GLOBAL_AUDIT_LOGGER.get_or_init(AsyncAuditTrailLogger::spawn)
}

pub async fn init_persistent_audit_trail(
    db: Arc<crate::db::Database>,
    cache: Option<Arc<tokio::sync::RwLock<crate::cache::Cache>>>,
) {
    global_audit_logger().attach_and_hydrate(db, cache).await;
}

pub async fn flush_audit_trail() {
    global_audit_logger().flush().await;
}

pub fn record_audit_async(entry: AuditTrailEntry) {
    global_audit_logger().record(entry);
}

/// Ergonomic input struct for emitting structured HTTP-triggered audit events without `.await`.
pub struct HttpAuditEvent<'a> {
    pub req: Option<&'a HttpRequest>,
    pub category: AuditCategory,
    pub action: AuditAction,
    pub outcome_override: Option<AuditOutcome>,
    pub status_code: u16,
    pub started_at: Option<std::time::Instant>,
    pub resource_name: Option<String>,
    pub resource_id: Option<String>,
    pub summary: String,
    pub before_state: Option<Value>,
    pub after_state: Option<Value>,
    pub metadata: Option<Value>,
    pub actor_email_override: Option<String>,
    pub actor_user_id_override: Option<i32>,
    pub actor_role_override: Option<String>,
    pub branch_override: Option<String>,
    pub request_id_override: Option<String>,
    pub method_override: Option<&'a str>,
    pub path_override: Option<&'a str>,
}

impl Default for HttpAuditEvent<'_> {
    fn default() -> Self {
        Self {
            req: None,
            category: AuditCategory::Resource,
            action: AuditAction::Create,
            outcome_override: None,
            status_code: 200,
            started_at: None,
            resource_name: None,
            resource_id: None,
            summary: String::new(),
            before_state: None,
            after_state: None,
            metadata: None,
            actor_email_override: None,
            actor_user_id_override: None,
            actor_role_override: None,
            branch_override: None,
            request_id_override: None,
            method_override: None,
            path_override: None,
        }
    }
}

/// Builds an `AuditTrailEntry` from `HttpAuditEvent` and enqueues it lock-free onto the
/// background audit trail pipeline. Diffing and redaction run asynchronously off the request thread.
pub fn record_http_audit_async(ev: HttpAuditEvent<'_>) {
    let mut actor = extract_actor_from_req(ev.req, ev.branch_override);
    if actor.email.is_none() {
        actor.email = ev.actor_email_override;
    }
    if actor.user_id.is_none() {
        actor.user_id = ev.actor_user_id_override;
    }
    if actor.role.is_none() {
        actor.role = ev.actor_role_override;
    }

    let request = extract_request_context(
        ev.req,
        ev.started_at,
        ev.request_id_override,
        ev.method_override,
        ev.path_override,
    );

    let outcome = ev
        .outcome_override
        .unwrap_or_else(|| AuditOutcome::from_status_code(ev.status_code));

    record_audit_async(AuditTrailEntry {
        schema_version: AUDIT_SCHEMA_VERSION,
        audit_id: generate_audit_id(),
        timestamp: Utc::now().to_rfc3339(),
        category: ev.category,
        action: ev.action,
        outcome,
        status_code: ev.status_code,
        resource_name: ev.resource_name,
        resource_id: ev.resource_id,
        actor,
        request,
        summary: ev.summary,
        before_state: ev.before_state,
        after_state: ev.after_state,
        changes: Vec::new(),
        metadata: ev.metadata,
    });
}

/// `GET /api/v1/audit` — Admin-only structured query over the audit trail (newest first).
pub async fn get_audit_trail(query: web::Query<AuditTrailQuery>) -> HttpResponse {
    match global_audit_logger().query(&query.into_inner()) {
        Ok(page) => HttpResponse::Ok().json(page),
        Err(msg) => HttpResponse::BadRequest().json(serde_json::json!({
            "success": false,
            "message": msg
        })),
    }
}

/// `GET /api/v1/audit/entity/{resource_name}/{resource_id}` — Admin-only entity audit timeline.
pub async fn get_entity_audit_history(
    path: web::Path<(String, String)>,
    query: web::Query<EntityHistoryQuery>,
) -> HttpResponse {
    let (resource_raw, resource_id) = path.into_inner();
    let Some(resource_name) = ResourceName::from_str_case_insensitive(&resource_raw) else {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "success": false,
            "message": format!("Invalid resource name: {}", resource_raw)
        }));
    };

    let q = query.into_inner();
    let ascending = q
        .order
        .as_deref()
        .is_some_and(|o| o.trim().eq_ignore_ascii_case("asc"));
    let limit = q.limit.unwrap_or(DEFAULT_QUERY_LIMIT);
    let offset = q.offset.unwrap_or(0);

    let history = global_audit_logger().entity_history(
        resource_name,
        &resource_id,
        limit,
        offset,
        ascending,
    );
    HttpResponse::Ok().json(history)
}

/// `GET /api/v1/audit/summary` — Admin-only aggregate counts across the audit trail.
pub async fn get_audit_summary(query: web::Query<AuditSummaryQuery>) -> HttpResponse {
    let summary = global_audit_logger().summarize(&query.into_inner());
    HttpResponse::Ok().json(summary)
}

pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.route("", web::get().to(get_audit_trail))
        .route("/summary", web::get().to(get_audit_summary))
        .route(
            "/entity/{resource_name}/{resource_id}",
            web::get().to(get_entity_audit_history),
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry(
        audit_id: &str,
        ts: &str,
        category: AuditCategory,
        action: AuditAction,
        outcome: AuditOutcome,
        resource_name: Option<&str>,
        resource_id: Option<&str>,
    ) -> AuditTrailEntry {
        AuditTrailEntry {
            schema_version: AUDIT_SCHEMA_VERSION,
            audit_id: audit_id.to_string(),
            timestamp: ts.to_string(),
            category,
            action,
            outcome,
            status_code: match outcome {
                AuditOutcome::Success => 200,
                AuditOutcome::Denied => 403,
                AuditOutcome::Failure => 500,
            },
            resource_name: resource_name.map(String::from),
            resource_id: resource_id.map(String::from),
            actor: AuditActor {
                email: Some("admin@narayani.com".to_string()),
                user_id: Some(1),
                role: Some("Admin".to_string()),
                branch: Some("Loadboard sthaniya".to_string()),
                ip_address: Some("127.0.0.1".to_string()),
                user_agent: Some("test-agent".to_string()),
            },
            request: AuditRequestContext {
                method: Some("PUT".to_string()),
                path: Some("/api/v1/test".to_string()),
                request_id: Some("req-test-1".to_string()),
                duration_ms: Some(4),
            },
            summary: format!("{} event", action.as_str()),
            before_state: None,
            after_state: None,
            changes: Vec::new(),
            metadata: None,
        }
    }

    #[test]
    fn test_compute_json_diff_nested_add_remove_modify() {
        let before = serde_json::json!({
            "status": "ACTIVE",
            "billing": {
                "vat": { "rate": 13, "enabled": true },
                "legacyKey": "old"
            }
        });
        let after = serde_json::json!({
            "status": "INACTIVE",
            "billing": {
                "vat": { "rate": 15, "enabled": true },
                "newFlag": false
            }
        });

        let diff = compute_json_diff(Some(&before), Some(&after));
        assert_eq!(diff.len(), 4);
        assert!(diff.iter().any(|c| {
            c.path == "billing.legacyKey"
                && c.change_type == FieldChangeType::Removed
                && c.old_value == Some(serde_json::json!("old"))
        }));
        assert!(diff.iter().any(|c| {
            c.path == "billing.newFlag"
                && c.change_type == FieldChangeType::Added
                && c.new_value == Some(serde_json::json!(false))
        }));
        assert!(diff.iter().any(|c| {
            c.path == "billing.vat.rate"
                && c.change_type == FieldChangeType::Modified
                && c.old_value == Some(serde_json::json!(13))
                && c.new_value == Some(serde_json::json!(15))
        }));
        assert!(diff.iter().any(|c| {
            c.path == "status"
                && c.change_type == FieldChangeType::Modified
                && c.old_value == Some(serde_json::json!("ACTIVE"))
                && c.new_value == Some(serde_json::json!("INACTIVE"))
        }));

        assert!(compute_json_diff(Some(&before), Some(&before)).is_empty());
        assert!(compute_json_diff(None, Some(&after)).is_empty());
        assert!(compute_json_diff(Some(&before), None).is_empty());
    }

    #[test]
    fn test_prepare_and_sanitize_detects_secret_change_without_leaking_values() {
        let mut entry = sample_entry(
            "AUD-TEST-REDACT",
            "2026-11-24T10:00:00Z",
            AuditCategory::AccessControl,
            AuditAction::Update,
            AuditOutcome::Success,
            Some("UserResource"),
            Some("42"),
        );
        entry.actor.branch = None;
        entry.before_state = Some(serde_json::json!({
            "email": "user@test.com",
            "password": "old-secret-password",
            "notifications": { "sms": { "apiKey": "old-key-1" } },
            "SanghBranch": "Loadboard sthaniya"
        }));
        entry.after_state = Some(serde_json::json!({
            "email": "user@test.com",
            "password": "new-secret-password",
            "notifications": { "sms": { "apiKey": "new-key-2" } },
            "SanghBranch": "Loadboard sthaniya"
        }));
        entry.metadata = Some(serde_json::json!({
            "nested": { "apiSecret": "super-secret", "token": "jwt-123" },
            "note": "password reset"
        }));

        prepare_and_sanitize_entry(&mut entry);

        // Branch inferred from payload before/after
        assert_eq!(entry.actor.branch.as_deref(), Some("Loadboard sthaniya"));

        // Diff detected both secret field updates WITHOUT exposing plaintext secrets
        assert_eq!(entry.changes.len(), 2);
        let pw_change = entry
            .changes
            .iter()
            .find(|c| c.path == "password")
            .expect("password change recorded");
        assert_eq!(pw_change.change_type, FieldChangeType::Modified);
        assert_eq!(pw_change.old_value, Some(serde_json::json!("[REDACTED]")));
        assert_eq!(pw_change.new_value, Some(serde_json::json!("[REDACTED]")));

        let key_change = entry
            .changes
            .iter()
            .find(|c| c.path == "notifications.sms.apiKey")
            .expect("apiKey change recorded");
        assert_eq!(key_change.old_value, Some(serde_json::json!("[REDACTED]")));
        assert_eq!(key_change.new_value, Some(serde_json::json!("[REDACTED]")));

        // Full before/after/metadata snapshots are redacted too
        assert_eq!(entry.before_state.as_ref().unwrap()["password"], "[REDACTED]");
        assert_eq!(
            entry.before_state.as_ref().unwrap()["notifications"]["sms"]["apiKey"],
            "[REDACTED]"
        );
        assert_eq!(entry.after_state.as_ref().unwrap()["password"], "[REDACTED]");
        assert_eq!(
            entry.after_state.as_ref().unwrap()["notifications"]["sms"]["apiKey"],
            "[REDACTED]"
        );
        assert_eq!(
            entry.metadata.as_ref().unwrap()["nested"]["apiSecret"],
            "[REDACTED]"
        );
        assert_eq!(
            entry.metadata.as_ref().unwrap()["nested"]["token"],
            "[REDACTED]"
        );
        assert_eq!(entry.metadata.as_ref().unwrap()["note"], "password reset");
    }

    #[actix_web::test]
    async fn test_persistent_audit_trail_writes_db_updates_cache_and_survives_restart() {
        let mut initial_state = crate::access::test_helpers::MockDbState::default();
        let seeded_id = "AUD-9000000000000001".to_string();
        let seeded_entry = sample_entry(
            &seeded_id,
            "2026-11-24T05:00:00Z",
            AuditCategory::Auth,
            AuditAction::Login,
            AuditOutcome::Success,
            Some("UserResource"),
            Some("1"),
        );
        initial_state.seed(
            ResourceName::AuditTrailResource,
            DatabaseResource {
                id: None,
                resource_name: Some(ResourceName::AuditTrailResource),
                inner_content: Some(serde_json::to_value(&seeded_entry).unwrap()),
                external_content: Some(serde_json::json!({ "createdTime": "2026-11-24T05:00:00Z" })),
            },
        );

        let (s_data_1, db_state_1) =
            crate::access::test_helpers::create_mock_server_data_with_state(initial_state).await;

        let logger_1 =
            AsyncAuditTrailLogger::spawn_persistent(s_data_1.db.clone(), Some(s_data_1.cache.clone()))
                .await;

        assert_eq!(logger_1.all().len(), 1);
        assert_eq!(logger_1.all()[0].audit_id, seeded_id);

        let post_boot_ids: Vec<String> = (0..3).map(|_| generate_audit_id()).collect();
        for (idx, action) in [
            AuditAction::Create,
            AuditAction::Update,
            AuditAction::Delete,
        ]
        .into_iter()
        .enumerate()
        {
            let mut e = sample_entry(
                &post_boot_ids[idx],
                "2026-11-24T06:00:00Z",
                AuditCategory::Resource,
                action,
                AuditOutcome::Success,
                Some("BranchResource"),
                Some(&(10 + idx).to_string()),
            );
            e.before_state = Some(serde_json::json!({ "name": "Old", "apiSecret": "sec-1" }));
            e.after_state = Some(serde_json::json!({ "name": "New", "apiSecret": "sec-2" }));
            logger_1.record(e);
        }
        logger_1.flush().await;

        {
            let cache = s_data_1.cache.read().await;
            assert!(cache.is_fully_loaded(ResourceName::AuditTrailResource));
            assert_eq!(
                cache
                    .read_all(ResourceName::AuditTrailResource)
                    .unwrap()
                    .len(),
                4
            );
        }

        let restart_snapshot = {
            let guard = db_state_1.lock().await;
            let table_name = ResourceName::AuditTrailResource.to_string().to_lowercase();
            let rows = guard
                .tables
                .get(&table_name)
                .expect("audittrailresource table populated");
            assert_eq!(rows.len(), 4);
            let last_inner = rows.last().unwrap().inner_content.as_ref().unwrap();
            assert_eq!(last_inner["beforeState"]["apiSecret"], "[REDACTED]");
            assert_eq!(last_inner["afterState"]["apiSecret"], "[REDACTED]");

            crate::access::test_helpers::MockDbState {
                tables: guard.tables.clone(),
                next_ids: guard.next_ids.clone(),
            }
        };
        drop(logger_1);

        // Full server restart simulation with fresh Cache + new logger backed by DB snapshot
        let (s_data_2, _) =
            crate::access::test_helpers::create_mock_server_data_with_state(restart_snapshot)
                .await;
        {
            let mut cache = s_data_2.cache.write().await;
            cache.load_from_db(&s_data_2.db).await;
        }
        let logger_2 =
            AsyncAuditTrailLogger::spawn_persistent(s_data_2.db.clone(), Some(s_data_2.cache.clone()))
                .await;

        let hydrated = logger_2.all();
        assert_eq!(hydrated.len(), 4);
        assert_eq!(hydrated[0].audit_id, seeded_id);
        for (i, id) in post_boot_ids.iter().enumerate() {
            assert_eq!(hydrated[i + 1].audit_id, *id);
            assert!(!hydrated[i + 1].changes.is_empty());
        }

        let next_id = generate_audit_id();
        let seeded_num: u64 = seeded_id.strip_prefix("AUD-").unwrap().parse().unwrap();
        let next_num: u64 = next_id.strip_prefix("AUD-").unwrap().parse().unwrap();
        assert!(next_num > seeded_num);
    }

    #[actix_web::test]
    async fn test_persistent_audit_trail_handles_burst_batching_across_chunks() {
        let state = crate::access::test_helpers::MockDbState::default();
        let (s_data, db_state) =
            crate::access::test_helpers::create_mock_server_data_with_state(state).await;

        let logger =
            AsyncAuditTrailLogger::spawn_persistent(s_data.db.clone(), Some(s_data.cache.clone()))
                .await;

        for i in 0..80 {
            logger.record(sample_entry(
                &format!("AUD-BURST-{i}"),
                "2026-11-24T07:00:00Z",
                AuditCategory::Billing,
                AuditAction::Payment,
                AuditOutcome::Success,
                Some("BillResource"),
                Some(&(500 + i).to_string()),
            ));
        }
        logger.flush().await;

        assert_eq!(logger.all().len(), 80);
        {
            let guard = db_state.lock().await;
            let table_name = ResourceName::AuditTrailResource.to_string().to_lowercase();
            assert_eq!(guard.tables.get(&table_name).unwrap().len(), 80);
        }
        {
            let cache = s_data.cache.read().await;
            assert_eq!(
                cache
                    .read_all(ResourceName::AuditTrailResource)
                    .unwrap()
                    .len(),
                80
            );
        }
    }

    #[actix_web::test]
    async fn test_audit_query_filtering_pagination_entity_history_and_summary() {
        let logger = AsyncAuditTrailLogger::spawn();

        let mut e1 = sample_entry(
            "AUD-Q-1",
            "2026-11-24T08:00:00Z",
            AuditCategory::Billing,
            AuditAction::Create,
            AuditOutcome::Success,
            Some("BillResource"),
            Some("501"),
        );
        e1.summary = "Created bill 501".to_string();

        let mut e2 = sample_entry(
            "AUD-Q-2",
            "2026-11-24T09:00:00Z",
            AuditCategory::Billing,
            AuditAction::Modify,
            AuditOutcome::Success,
            Some("BillResource"),
            Some("501"),
        );
        e2.actor.email = Some("operator@narayani.com".to_string());
        e2.actor.role = Some("Operator".to_string());
        e2.summary = "Modified bill 501".to_string();

        let mut e3 = sample_entry(
            "AUD-Q-3",
            "2026-11-24T10:00:00Z",
            AuditCategory::AccessControl,
            AuditAction::AccessDenied,
            AuditOutcome::Denied,
            Some("PermissionResource"),
            Some("12"),
        );
        e3.actor.email = Some("user@narayani.com".to_string());
        e3.actor.role = Some("User".to_string());
        e3.actor.branch = Some("Other Branch".to_string());
        e3.summary = "Denied PermissionResource update".to_string();

        logger.record(e1);
        logger.record(e2);
        logger.record(e3);
        logger.flush().await;

        let page = logger
            .query(&AuditTrailQuery {
                category: Some("billing".to_string()),
                limit: Some(1),
                offset: Some(0),
                ..Default::default()
            })
            .expect("valid query");
        assert_eq!(page.total, 2);
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].audit_id, "AUD-Q-2");

        let denied = logger
            .query(&AuditTrailQuery {
                outcome: Some("DENIED".to_string()),
                ..Default::default()
            })
            .expect("valid query");
        assert_eq!(denied.total, 1);
        assert_eq!(denied.items[0].audit_id, "AUD-Q-3");

        let invalid = logger.query(&AuditTrailQuery {
            category: Some("bogus".to_string()),
            ..Default::default()
        });
        assert!(invalid.is_err());

        let timeline_asc = logger.entity_history(ResourceName::BillResource, "501", 10, 0, true);
        assert_eq!(timeline_asc.total, 2);
        assert_eq!(timeline_asc.items[0].audit_id, "AUD-Q-1");
        assert_eq!(timeline_asc.items[1].audit_id, "AUD-Q-2");

        let summary = logger.summarize(&AuditSummaryQuery {
            branch: Some("Loadboard sthaniya".to_string()),
            ..Default::default()
        });
        assert_eq!(summary.total_entries, 2);
        assert_eq!(summary.by_category.get("BILLING"), Some(&2));
        assert_eq!(summary.by_action.get("CREATE"), Some(&1));
        assert_eq!(summary.by_action.get("MODIFY"), Some(&1));
        assert_eq!(summary.latest_timestamp.as_deref(), Some("2026-11-24T09:00:00Z"));
    }

    #[actix_web::test]
    async fn test_audit_http_endpoints_return_filtered_pages_entity_history_and_summary() {
        let unique_branch = format!(
            "AuditHttpBranch-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let mut entry = sample_entry(
            &generate_audit_id(),
            &Utc::now().to_rfc3339(),
            AuditCategory::Settings,
            AuditAction::Update,
            AuditOutcome::Success,
            Some("SettingResource"),
            Some("1"),
        );
        entry.actor.branch = Some(unique_branch.clone());
        entry.summary = format!("HTTP test audit for {}", unique_branch);

        record_audit_async(entry.clone());
        flush_audit_trail().await;

        let app = actix_web::test::init_service(
            actix_web::App::new().service(web::scope("/api/v1/audit").configure(configure_routes)),
        )
        .await;

        let req_list = actix_web::test::TestRequest::get()
            .uri(&format!("/api/v1/audit?branch={}&category=SETTINGS", unique_branch))
            .to_request();
        let page: AuditTrailPage = actix_web::test::call_and_read_body_json(&app, req_list).await;
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].audit_id, entry.audit_id);

        let req_entity = actix_web::test::TestRequest::get()
            .uri("/api/v1/audit/entity/settingresource/1?order=desc")
            .to_request();
        let hist: EntityAuditHistory =
            actix_web::test::call_and_read_body_json(&app, req_entity).await;
        assert_eq!(hist.resource_name, "SettingResource");
        assert!(hist.items.iter().any(|i| i.audit_id == entry.audit_id));

        let req_bad_entity = actix_web::test::TestRequest::get()
            .uri("/api/v1/audit/entity/NotAResource/1")
            .to_request();
        let bad_resp = actix_web::test::call_service(&app, req_bad_entity).await;
        assert_eq!(
            bad_resp.status(),
            actix_web::http::StatusCode::BAD_REQUEST
        );

        let req_summary = actix_web::test::TestRequest::get()
            .uri(&format!("/api/v1/audit/summary?branch={}", unique_branch))
            .to_request();
        let summary: AuditTrailSummary =
            actix_web::test::call_and_read_body_json(&app, req_summary).await;
        assert_eq!(summary.total_entries, 1);
        assert_eq!(summary.by_category.get("SETTINGS"), Some(&1));
    }
}
