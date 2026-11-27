use crate::resource::{DatabaseResource, ResourceName};
use crate::ServerData;
use actix_web::{web, HttpMessage, HttpRequest, HttpResponse};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
use tokio::sync::mpsc;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionType {
    Create,
    Modify,
    Delete,
    Rollback,
}

impl std::fmt::Display for ActionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActionType::Create => write!(f, "CREATE"),
            ActionType::Modify => write!(f, "MODIFY"),
            ActionType::Delete => write!(f, "DELETE"),
            ActionType::Rollback => write!(f, "ROLLBACK"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BillLogEntry {
    pub id: String,
    pub action: ActionType,
    pub bill_id: i32,
    pub performed_by: Option<String>,
    pub performed_at: String,
    pub previous_state: Option<Value>,
    pub new_state: Option<Value>,
    pub details: Option<String>,
    pub is_rolled_back: bool,
    pub rolled_back_at: Option<String>,
    pub rolled_back_by: Option<String>,
}

static LOG_STORE: OnceLock<RwLock<narayani_storage::RingBuffer<BillLogEntry>>> = OnceLock::new();
static LOG_COUNTER: OnceLock<std::sync::atomic::AtomicU64> = OnceLock::new();

pub fn get_log_store() -> &'static RwLock<narayani_storage::RingBuffer<BillLogEntry>> {
    LOG_STORE.get_or_init(|| RwLock::new(narayani_storage::RingBuffer::with_capacity(narayani_storage::MAX_LOG_ENTRIES)))
}

pub fn generate_log_id() -> String {
    let counter = LOG_COUNTER.get_or_init(|| {
        let initial = Utc::now().timestamp_subsec_millis() as u64;
        std::sync::atomic::AtomicU64::new(initial.max(1000))
    });
    let seq = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    format!("LOG-{}", seq)
}

pub fn add_log(entry: BillLogEntry) {
    let mut store = get_log_store().write().unwrap();
    store.push(entry);
}

pub fn get_all_logs() -> Vec<BillLogEntry> {
    let store = get_log_store().read().unwrap();
    store.to_vec()
}

pub fn get_log_by_id(log_id: &str) -> Option<BillLogEntry> {
    let store = get_log_store().read().unwrap();
    store.iter().find(|l| l.id == log_id).cloned()
}

pub fn get_logs_for_bill(bill_id: i32) -> Vec<BillLogEntry> {
    let store = get_log_store().read().unwrap();
    store.iter().filter(|l| l.bill_id == bill_id).cloned().collect()
}

pub async fn rollback_action(
    log_id_or_bill_id: &str,
    admin_user: &str,
    s_data: &ServerData,
) -> Result<DatabaseResource, String> {
    let (target_log_id, target_log) = {
        let store = get_log_store().read().unwrap();
        // 1. Try matching by log ID directly
        if let Some(entry) = store.iter().find(|l| l.id == log_id_or_bill_id) {
            (entry.id.clone(), entry.clone())
        } else if let Ok(bill_id) = log_id_or_bill_id.parse::<i32>() {
            // 2. Try matching by bill ID -> find the latest non-rolled-back action
            let entry = store
                .iter()
                .rev()
                .find(|l| l.bill_id == bill_id && l.action != ActionType::Rollback && !l.is_rolled_back)
                .ok_or_else(|| format!("No eligible un-reverted action found for bill {}", bill_id))?;
            (entry.id.clone(), entry.clone())
        } else {
            return Err(format!("Log entry or bill '{}' not found", log_id_or_bill_id));
        }
    };

    if target_log.is_rolled_back {
        return Err(format!("Action '{}' has already been rolled back", target_log.id));
    }

    if target_log.action == ActionType::Rollback {
        return Err("Cannot rollback a rollback action".to_string());
    }

    let bill_id = target_log.bill_id;
    let now_str = Utc::now().to_rfc3339();

    // Perform rollback according to original action
    let restored_resource = match target_log.action {
        ActionType::Delete => {
            // Revert soft-delete: restore previous content or clear DELETED status
            let restored_inner = match target_log.previous_state.clone() {
                Some(mut prev) => {
                    if let Some(obj) = prev.as_object_mut() {
                        obj.insert("status".to_string(), Value::String("ACTIVE".to_string()));
                        obj.remove("deletedAt");
                        obj.insert("restoredAt".to_string(), Value::String(now_str.clone()));
                        obj.insert("restoredBy".to_string(), Value::String(admin_user.to_string()));
                    }
                    prev
                }
                None => {
                    // Fallback: read current and remove deleted status
                    let current = s_data.cache.read().await.read(ResourceName::BillResource, bill_id);
                    let mut inner = current.and_then(|r| r.inner_content).unwrap_or(serde_json::json!({}));
                    if let Some(obj) = inner.as_object_mut() {
                        obj.insert("status".to_string(), Value::String("ACTIVE".to_string()));
                        obj.remove("deletedAt");
                        obj.insert("restoredAt".to_string(), Value::String(now_str.clone()));
                        obj.insert("restoredBy".to_string(), Value::String(admin_user.to_string()));
                    }
                    inner
                }
            };

            let res = DatabaseResource {
                id: Some(bill_id),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(restored_inner),
                external_content: None,
            };

            s_data
                .db
                .update_resource(ResourceName::BillResource, bill_id, res.clone())
                .await
                .map_err(|e| format!("Failed to update database during rollback: {:?}", e))?;

            s_data.cache.write().await.write(ResourceName::BillResource, bill_id, res.clone());
            res
        }
        ActionType::Modify => {
            // Revert modification: restore original previous_state
            let previous_content = target_log
                .previous_state
                .clone()
                .ok_or_else(|| "No previous state available to restore".to_string())?;

            let mut restored_inner = previous_content;
            if let Some(obj) = restored_inner.as_object_mut() {
                // If it was marked AMENDED, restore it to PAID / ACTIVE
                if obj.get("status").and_then(Value::as_str) == Some("AMENDED") {
                    obj.insert("status".to_string(), Value::String("PAID".to_string()));
                }
                obj.insert("rolledBackAt".to_string(), Value::String(now_str.clone()));
                obj.insert("rolledBackBy".to_string(), Value::String(admin_user.to_string()));
            }

            let res = DatabaseResource {
                id: Some(bill_id),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(restored_inner),
                external_content: None,
            };

            s_data
                .db
                .update_resource(ResourceName::BillResource, bill_id, res.clone())
                .await
                .map_err(|e| format!("Failed to update database during rollback: {:?}", e))?;

            s_data.cache.write().await.write(ResourceName::BillResource, bill_id, res.clone());
            res
        }
        ActionType::Create => {
            // Revert creation: mark newly created bill as CANCELLED / ROLLED_BACK
            let current = s_data.cache.read().await.read(ResourceName::BillResource, bill_id);
            let mut inner = current.and_then(|r| r.inner_content).unwrap_or(serde_json::json!({}));
            if let Some(obj) = inner.as_object_mut() {
                obj.insert("status".to_string(), Value::String("ROLLED_BACK".to_string()));
                obj.insert("rolledBackAt".to_string(), Value::String(now_str.clone()));
                obj.insert("rolledBackBy".to_string(), Value::String(admin_user.to_string()));
            }

            let res = DatabaseResource {
                id: Some(bill_id),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(inner),
                external_content: None,
            };

            s_data
                .db
                .update_resource(ResourceName::BillResource, bill_id, res.clone())
                .await
                .map_err(|e| format!("Failed to update database during rollback: {:?}", e))?;

            s_data.cache.write().await.write(ResourceName::BillResource, bill_id, res.clone());
            res
        }
        ActionType::Rollback => unreachable!(),
    };

    // Mark the original log entry as rolled back
    {
        let mut store = get_log_store().write().unwrap();
        if let Some(entry) = store.iter_mut().find(|l| l.id == target_log_id) {
            entry.is_rolled_back = true;
            entry.rolled_back_at = Some(now_str.clone());
            entry.rolled_back_by = Some(admin_user.to_string());
        }
    }

    // Add a new Rollback log entry to preserve immutable audit trail
    let rollback_log = BillLogEntry {
        id: generate_log_id(),
        action: ActionType::Rollback,
        bill_id,
        performed_by: Some(admin_user.to_string()),
        performed_at: now_str,
        previous_state: target_log.new_state.clone(),
        new_state: restored_resource.inner_content.clone(),
        details: Some(format!(
            "Admin {} rolled back {} action (Log ID: {})",
            admin_user, target_log.action, target_log.id
        )),
        is_rolled_back: false,
        rolled_back_at: None,
        rolled_back_by: None,
    };
    add_log(rollback_log);

    Ok(restored_resource)
}

// =========================================================================
// Asynchronous Action & Login Audit Logging (Zero-Await Hot Path)
// =========================================================================

const MAX_IN_MEMORY_ACTION_LOGS: usize = 50_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceActionKind {
    Create,
    Read,
    Update,
    Delete,
    Search,
    Login,
    Logout,
}

impl ResourceActionKind {
    pub fn from_str_case_insensitive(s: &str) -> Option<Self> {
        let trimmed = s.trim();
        if trimmed.eq_ignore_ascii_case("CREATE") {
            Some(Self::Create)
        } else if trimmed.eq_ignore_ascii_case("READ") {
            Some(Self::Read)
        } else if trimmed.eq_ignore_ascii_case("UPDATE") {
            Some(Self::Update)
        } else if trimmed.eq_ignore_ascii_case("DELETE") {
            Some(Self::Delete)
        } else if trimmed.eq_ignore_ascii_case("SEARCH") {
            Some(Self::Search)
        } else if trimmed.eq_ignore_ascii_case("LOGIN") {
            Some(Self::Login)
        } else if trimmed.eq_ignore_ascii_case("LOGOUT") {
            Some(Self::Logout)
        } else {
            None
        }
    }
}

impl std::fmt::Display for ResourceActionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResourceActionKind::Create => write!(f, "CREATE"),
            ResourceActionKind::Read => write!(f, "READ"),
            ResourceActionKind::Update => write!(f, "UPDATE"),
            ResourceActionKind::Delete => write!(f, "DELETE"),
            ResourceActionKind::Search => write!(f, "SEARCH"),
            ResourceActionKind::Login => write!(f, "LOGIN"),
            ResourceActionKind::Logout => write!(f, "LOGOUT"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionLogEntry {
    pub id: String,
    pub action: ResourceActionKind,
    pub resource_name: Option<String>,
    pub resource_id: Option<String>,
    pub performed_by: Option<String>,
    pub user_id: Option<i32>,
    pub role: Option<String>,
    pub performed_at: String,
    pub status_code: u16,
    pub success: bool,
    pub duration_ms: u64,
    pub details: Option<String>,
    pub metadata: Option<Value>,
}

static ACTION_LOG_COUNTER: OnceLock<AtomicU64> = OnceLock::new();

fn action_log_counter() -> &'static AtomicU64 {
    ACTION_LOG_COUNTER.get_or_init(|| {
        let initial = u64::try_from(Utc::now().timestamp_micros())
            .unwrap_or(1000)
            .max(1000);
        AtomicU64::new(initial)
    })
}

pub fn generate_action_log_id() -> String {
    let seq = action_log_counter().fetch_add(1, Ordering::Relaxed);
    format!("ACTLOG-{}", seq)
}

fn advance_action_log_counter_past(entries: &[ActionLogEntry]) {
    let max_existing = entries
        .iter()
        .filter_map(|e| e.id.strip_prefix("ACTLOG-"))
        .filter_map(|num| num.parse::<u64>().ok())
        .max();
    if let Some(max_id) = max_existing {
        action_log_counter().fetch_max(max_id.saturating_add(1), Ordering::Relaxed);
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let lower = key.trim().to_ascii_lowercase();
    lower == "password"
        || lower == "token"
        || lower.ends_with("password")
        || lower.ends_with("token")
}

/// Recursively redacts sensitive fields (`password`, `token`) before persisting or exposing logs.
pub fn sanitize_json_for_log(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut sanitized = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                if is_sensitive_key(k) {
                    sanitized.insert(k.clone(), Value::String("[REDACTED]".to_string()));
                } else {
                    sanitized.insert(k.clone(), sanitize_json_for_log(v));
                }
            }
            Value::Object(sanitized)
        }
        Value::Array(items) => Value::Array(items.iter().map(sanitize_json_for_log).collect()),
        _ => value.clone(),
    }
}

const DB_PERSIST_BATCH_SIZE: usize = 32;
const MAX_DRAIN_PER_CYCLE: usize = 256;

#[derive(Clone)]
pub struct ActionLogDbSink {
    pub db: Arc<crate::db::Database>,
    pub cache: Option<Arc<tokio::sync::RwLock<crate::cache::Cache>>>,
}

enum AsyncLogMsg {
    Entry(ActionLogEntry),
    SetDbSink(Option<ActionLogDbSink>),
    Flush(std::sync::mpsc::SyncSender<()>),
}

async fn persist_batch_to_db(sink: &ActionLogDbSink, batch: &[ActionLogEntry]) {
    let now_str = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut draft_resources: Vec<DatabaseResource> = Vec::with_capacity(batch.len());

    for entry in batch {
        match serde_json::to_value(entry) {
            Ok(inner_val) => draft_resources.push(DatabaseResource {
                id: None,
                resource_name: Some(ResourceName::ActionLogsResource),
                inner_content: Some(inner_val),
                external_content: Some(serde_json::json!({ "createdTime": now_str })),
            }),
            Err(err) => {
                eprintln!(
                    "AsyncActionLogger DB serialization error for {}: {:?}",
                    entry.id, err
                );
            }
        }
    }

    for chunk in draft_resources.chunks(DB_PERSIST_BATCH_SIZE) {
        let chunk_vec = chunk.to_vec();
        match sink
            .db
            .batch_create_resources(ResourceName::ActionLogsResource, chunk_vec.clone())
            .await
        {
            Ok(ids) => {
                if let Some(ref cache_lock) = sink.cache {
                    let mut cache = cache_lock.write().await;
                    for (id, mut res) in ids.into_iter().zip(chunk_vec.into_iter()) {
                        res.id = Some(id);
                        res.resource_name = Some(ResourceName::ActionLogsResource);
                        cache.write(ResourceName::ActionLogsResource, id, res);
                    }
                }
            }
            Err(err) => {
                eprintln!(
                    "AsyncActionLogger batch DB persist failed ({:?}), retrying individually",
                    err
                );
                for mut res in chunk_vec {
                    match sink
                        .db
                        .create_resource(ResourceName::ActionLogsResource, res.clone())
                        .await
                    {
                        Ok(id) => {
                            res.id = Some(id);
                            res.resource_name = Some(ResourceName::ActionLogsResource);
                            if let Some(ref cache_lock) = sink.cache {
                                let mut cache = cache_lock.write().await;
                                cache.write(ResourceName::ActionLogsResource, id, res);
                            }
                        }
                        Err(item_err) => {
                            eprintln!(
                                "AsyncActionLogger single DB persist failed: {:?}",
                                item_err
                            );
                        }
                    }
                }
            }
        }
    }
}

fn persist_batch_to_ndjson(
    batch: &[ActionLogEntry],
    log_file_path: &Option<PathBuf>,
    writer: &mut Option<BufWriter<std::fs::File>>,
    file_init_failed: &mut bool,
) {
    let Some(ref path) = log_file_path else {
        return;
    };

    if writer.is_none() && !*file_init_failed {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                if let Err(err) = std::fs::create_dir_all(parent) {
                    eprintln!(
                        "AsyncActionLogger failed to create log directory {:?}: {:?}",
                        parent, err
                    );
                    *file_init_failed = true;
                }
            }
        }
        if !*file_init_failed {
            match std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                Ok(file) => {
                    *writer = Some(BufWriter::new(file));
                }
                Err(err) => {
                    eprintln!(
                        "AsyncActionLogger failed to open log file {:?}: {:?}",
                        path, err
                    );
                    *file_init_failed = true;
                }
            }
        }
    }

    if let Some(ref mut w) = writer {
        for entry in batch {
            if let Err(err) = serde_json::to_writer(&mut *w, entry) {
                eprintln!("AsyncActionLogger serialization error: {:?}", err);
                continue;
            }
            if let Err(err) = w.write_all(b"\n") {
                eprintln!("AsyncActionLogger write error: {:?}", err);
            }
        }
        if let Err(err) = w.flush() {
            eprintln!("AsyncActionLogger flush error: {:?}", err);
        }
    }
}

async fn persist_batch_all(
    batch: &mut [ActionLogEntry],
    worker_store: &RwLock<Vec<ActionLogEntry>>,
    active_sink: Option<&ActionLogDbSink>,
    log_file_path: &Option<PathBuf>,
    writer: &mut Option<BufWriter<std::fs::File>>,
    file_init_failed: &mut bool,
) {
    for entry in batch.iter_mut() {
        if let Some(meta) = entry.metadata.take() {
            entry.metadata = Some(sanitize_json_for_log(&meta));
        }
    }

    {
        let mut guard = worker_store.write().unwrap_or_else(|e| e.into_inner());
        guard.extend(batch.iter().cloned());
        if guard.len() > MAX_IN_MEMORY_ACTION_LOGS {
            let excess = guard.len() - MAX_IN_MEMORY_ACTION_LOGS;
            guard.drain(..excess);
        }
    }

    if let Some(sink) = active_sink {
        persist_batch_to_db(sink, batch).await;
    }

    persist_batch_to_ndjson(batch, log_file_path, writer, file_init_failed);
}

pub struct AsyncActionLogger {
    tx: mpsc::UnboundedSender<AsyncLogMsg>,
    store: Arc<RwLock<Vec<ActionLogEntry>>>,
}

impl AsyncActionLogger {
    pub fn spawn(log_file_path: Option<PathBuf>) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<AsyncLogMsg>();
        let store = Arc::new(RwLock::new(Vec::new()));
        let worker_store = store.clone();

        std::thread::Builder::new()
            .name("async-action-logger".to_string())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("Failed to create async-action-logger Tokio runtime");

                rt.block_on(async move {
                    let mut writer: Option<BufWriter<std::fs::File>> = None;
                    let mut file_init_failed = false;
                    let mut active_sink: Option<ActionLogDbSink> = None;

                    while let Some(first) = rx.recv().await {
                        let mut batch: Vec<ActionLogEntry> = Vec::new();
                        let mut flush_waiters: Vec<std::sync::mpsc::SyncSender<()>> = Vec::new();
                        let mut next_sink: Option<Option<ActionLogDbSink>> = None;

                        match first {
                            AsyncLogMsg::Entry(e) => batch.push(e),
                            AsyncLogMsg::Flush(waiter) => flush_waiters.push(waiter),
                            AsyncLogMsg::SetDbSink(new_sink) => next_sink = Some(new_sink),
                        }

                        if next_sink.is_none() {
                            for _ in 0..MAX_DRAIN_PER_CYCLE {
                                match rx.try_recv() {
                                    Ok(AsyncLogMsg::Entry(e)) => batch.push(e),
                                    Ok(AsyncLogMsg::Flush(waiter)) => flush_waiters.push(waiter),
                                    Ok(AsyncLogMsg::SetDbSink(new_sink)) => {
                                        next_sink = Some(new_sink);
                                        break;
                                    }
                                    Err(_) => break,
                                }
                            }
                        }

                        if !batch.is_empty() {
                            persist_batch_all(
                                &mut batch,
                                &worker_store,
                                active_sink.as_ref(),
                                &log_file_path,
                                &mut writer,
                                &mut file_init_failed,
                            )
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
            .expect("Failed to spawn async-action-logger thread");

        Self { tx, store }
    }

    pub async fn spawn_persistent(
        db: Arc<crate::db::Database>,
        cache: Option<Arc<tokio::sync::RwLock<crate::cache::Cache>>>,
        log_file_path: Option<PathBuf>,
    ) -> Self {
        let logger = Self::spawn(log_file_path);
        logger.attach_and_hydrate(db, cache).await;
        logger
    }

    /// Hydrates historical `ActionLogsResource` rows from PostgreSQL (preserving chronological
    /// order and advancing `ACTION_LOG_COUNTER`), populates `cache` if provided, and installs
    /// the DB persistence sink so all subsequent logs persist across restarts.
    pub async fn attach_and_hydrate(
        &self,
        db: Arc<crate::db::Database>,
        cache: Option<Arc<tokio::sync::RwLock<crate::cache::Cache>>>,
    ) {
        self.flush().await;

        match db.read_all_resource(ResourceName::ActionLogsResource).await {
            Ok(mut rows) => {
                rows.sort_by_key(|r| r.id.unwrap_or(0));

                let mut persisted: Vec<ActionLogEntry> = Vec::with_capacity(rows.len());
                for row in &rows {
                    if let Some(ref inner) = row.inner_content {
                        if let Ok(entry) = serde_json::from_value::<ActionLogEntry>(inner.clone()) {
                            persisted.push(entry);
                        }
                    }
                }

                advance_action_log_counter_past(&persisted);

                {
                    let mut seen_ids: HashSet<String> =
                        persisted.iter().map(|e| e.id.clone()).collect();
                    let mut guard = self.store.write().unwrap_or_else(|e| e.into_inner());
                    let existing = std::mem::take(&mut *guard);
                    let mut merged = persisted;
                    for entry in existing {
                        if seen_ids.insert(entry.id.clone()) {
                            merged.push(entry);
                        }
                    }
                    if merged.len() > MAX_IN_MEMORY_ACTION_LOGS {
                        let excess = merged.len() - MAX_IN_MEMORY_ACTION_LOGS;
                        merged.drain(..excess);
                    }
                    *guard = merged;
                }

                if let Some(ref cache_lock) = cache {
                    let mut c = cache_lock.write().await;
                    for mut row in rows {
                        if let Some(id) = row.id {
                            row.resource_name = Some(ResourceName::ActionLogsResource);
                            c.write(ResourceName::ActionLogsResource, id, row);
                        }
                    }
                }
            }
            Err(err) => {
                eprintln!(
                    "AsyncActionLogger failed to hydrate ActionLogsResource from DB: {:?}",
                    err
                );
            }
        }

        let _ = self.tx.send(AsyncLogMsg::SetDbSink(Some(ActionLogDbSink {
            db,
            cache,
        })));
        self.flush().await;
    }

    /// Non-blocking, lock-free enqueue onto the background logger channel.
    pub fn log(&self, entry: ActionLogEntry) {
        let _ = self.tx.send(AsyncLogMsg::Entry(entry));
    }

    pub fn all(&self) -> Vec<ActionLogEntry> {
        let guard = self.store.read().unwrap_or_else(|e| e.into_inner());
        guard.clone()
    }

    pub fn flush_sync(&self) {
        let (waiter_tx, waiter_rx) = std::sync::mpsc::sync_channel::<()>(1);
        if self.tx.send(AsyncLogMsg::Flush(waiter_tx)).is_ok() {
            let _ = waiter_rx.recv();
        }
    }

    pub async fn flush(&self) {
        let tx = self.tx.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let (waiter_tx, waiter_rx) = std::sync::mpsc::sync_channel::<()>(1);
            if tx.send(AsyncLogMsg::Flush(waiter_tx)).is_ok() {
                let _ = waiter_rx.recv();
            }
        })
        .await;
    }
}

pub async fn init_persistent_action_logger(
    db: Arc<crate::db::Database>,
    cache: Option<Arc<tokio::sync::RwLock<crate::cache::Cache>>>,
) {
    get_async_action_logger()
        .attach_and_hydrate(db, cache)
        .await;
}

static ASYNC_ACTION_LOGGER: OnceLock<AsyncActionLogger> = OnceLock::new();

fn resolve_default_log_file_path() -> Option<PathBuf> {
    let from_env = std::env::var("ACTION_LOG_FILE")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    if from_env.is_some() {
        return from_env;
    }

    #[cfg(not(test))]
    {
        Some(PathBuf::from("logs/actions.jsonl"))
    }
    #[cfg(test)]
    {
        None
    }
}

pub fn get_async_action_logger() -> &'static AsyncActionLogger {
    ASYNC_ACTION_LOGGER.get_or_init(|| AsyncActionLogger::spawn(resolve_default_log_file_path()))
}

/// Lock-free, zero-await asynchronous enqueue onto the background action logger.
pub fn log_action_async(entry: ActionLogEntry) {
    get_async_action_logger().log(entry);
}

#[allow(clippy::too_many_arguments)]
pub fn record_http_action_async(
    req: Option<&HttpRequest>,
    action: ResourceActionKind,
    resource_name: Option<String>,
    resource_id: Option<String>,
    status_code: u16,
    started_at: std::time::Instant,
    details: Option<String>,
    metadata: Option<Value>,
) {
    let (performed_by, user_id, role) = req
        .and_then(|r| {
            r.extensions().get::<crate::auth::Claims>().map(|c| {
                (
                    Some(c.sub.clone()),
                    c.user_id,
                    Some(String::from(c.role.clone())),
                )
            })
        })
        .unwrap_or((None, None, None));

    let duration_ms = started_at.elapsed().as_millis() as u64;
    let success = (200..300).contains(&status_code);

    log_action_async(ActionLogEntry {
        id: generate_action_log_id(),
        action,
        resource_name,
        resource_id,
        performed_by,
        user_id,
        role,
        performed_at: Utc::now().to_rfc3339(),
        status_code,
        success,
        duration_ms,
        details,
        metadata,
    });
}

pub fn log_login_async(
    email: String,
    user_id: Option<i32>,
    role: Option<String>,
    status_code: u16,
    started_at: std::time::Instant,
    details: Option<String>,
) {
    let duration_ms = started_at.elapsed().as_millis() as u64;
    let success = (200..300).contains(&status_code);

    log_action_async(ActionLogEntry {
        id: generate_action_log_id(),
        action: ResourceActionKind::Login,
        resource_name: Some("UserResource".to_string()),
        resource_id: user_id.map(|id| id.to_string()),
        performed_by: Some(email),
        user_id,
        role,
        performed_at: Utc::now().to_rfc3339(),
        status_code,
        success,
        duration_ms,
        details,
        metadata: None,
    });
}

pub fn log_logout_async(
    email: Option<String>,
    user_id: Option<i32>,
    role: Option<String>,
    status_code: u16,
    started_at: std::time::Instant,
    details: Option<String>,
) {
    let duration_ms = started_at.elapsed().as_millis() as u64;
    let success = (200..300).contains(&status_code);

    log_action_async(ActionLogEntry {
        id: generate_action_log_id(),
        action: ResourceActionKind::Logout,
        resource_name: Some("UserResource".to_string()),
        resource_id: user_id.map(|id| id.to_string()),
        performed_by: email,
        user_id,
        role,
        performed_at: Utc::now().to_rfc3339(),
        status_code,
        success,
        duration_ms,
        details,
        metadata: None,
    });
}

pub fn get_all_action_logs() -> Vec<ActionLogEntry> {
    get_async_action_logger().all()
}

pub fn get_action_logs_by_resource(resource_name: &str) -> Vec<ActionLogEntry> {
    let target = resource_name.trim();
    get_all_action_logs()
        .into_iter()
        .filter(|entry| {
            entry
                .resource_name
                .as_deref()
                .is_some_and(|r| r.eq_ignore_ascii_case(target))
        })
        .collect()
}

pub fn get_action_logs_by_action(action: ResourceActionKind) -> Vec<ActionLogEntry> {
    get_all_action_logs()
        .into_iter()
        .filter(|entry| entry.action == action)
        .collect()
}

pub fn flush_action_logs_sync() {
    get_async_action_logger().flush_sync();
}

pub async fn flush_action_logs() {
    get_async_action_logger().flush().await;
}

#[derive(Debug, Clone, Deserialize)]
pub struct ActionLogQuery {
    pub resource: Option<String>,
    pub action: Option<String>,
    pub user: Option<String>,
    pub limit: Option<usize>,
}

/// `GET /api/v1/logs/actions` — returns action & login audit logs (newest first), optionally
/// filtered by `resource`, `action`, `user`, and `limit`.
pub async fn get_action_logs_handler(query: web::Query<ActionLogQuery>) -> HttpResponse {
    let q = query.into_inner();
    let parsed_action = match q.action.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(raw) => match ResourceActionKind::from_str_case_insensitive(raw) {
            Some(kind) => Some(Some(kind)),
            None => Some(None),
        },
        None => None,
    };

    let resource_filter = q
        .resource
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let user_filter = q.user.as_deref().map(str::trim).filter(|s| !s.is_empty());

    let all = get_all_action_logs();
    let iter = all.into_iter().rev().filter(|entry| {
        if let Some(res_q) = resource_filter {
            if !entry
                .resource_name
                .as_deref()
                .is_some_and(|r| r.eq_ignore_ascii_case(res_q))
            {
                return false;
            }
        }
        if let Some(ref action_q) = parsed_action {
            match action_q {
                Some(kind) if &entry.action == kind => {}
                _ => return false,
            }
        }
        if let Some(user_q) = user_filter {
            let matches_email = entry
                .performed_by
                .as_deref()
                .is_some_and(|u| u.eq_ignore_ascii_case(user_q));
            let matches_uid = entry.user_id.is_some_and(|id| id.to_string() == user_q);
            if !matches_email && !matches_uid {
                return false;
            }
        }
        true
    });

    let results: Vec<ActionLogEntry> = match q.limit {
        Some(limit) => iter.take(limit).collect(),
        None => iter.collect(),
    };

    HttpResponse::Ok().json(results)
}

/// `GET /api/v1/logs/actions/{resource_name}` — returns action logs for a specific resource.
pub async fn get_action_logs_by_resource_handler(path: web::Path<String>) -> HttpResponse {
    let resource_name = path.into_inner();
    let logs: Vec<ActionLogEntry> = get_action_logs_by_resource(&resource_name)
        .into_iter()
        .rev()
        .collect();
    HttpResponse::Ok().json(logs)
}

pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.route("/actions", web::get().to(get_action_logs_handler))
        .route(
            "/actions/{resource_name}",
            web::get().to(get_action_logs_by_resource_handler),
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_type_display() {
        assert_eq!(ActionType::Create.to_string(), "CREATE");
        assert_eq!(ActionType::Modify.to_string(), "MODIFY");
        assert_eq!(ActionType::Delete.to_string(), "DELETE");
        assert_eq!(ActionType::Rollback.to_string(), "ROLLBACK");
    }

    #[test]
    fn test_generate_log_id() {
        let id1 = generate_log_id();
        let id2 = generate_log_id();
        assert!(id1.starts_with("LOG-"));
        assert!(id2.starts_with("LOG-"));
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_add_and_get_logs() {
        let entry = BillLogEntry {
            id: generate_log_id(),
            action: ActionType::Create,
            bill_id: 101,
            performed_by: Some("operator@test.com".to_string()),
            performed_at: Utc::now().to_rfc3339(),
            previous_state: None,
            new_state: Some(serde_json::json!({ "total": 500 })),
            details: Some("Created test bill".to_string()),
            is_rolled_back: false,
            rolled_back_at: None,
            rolled_back_by: None,
        };

        add_log(entry.clone());

        let all = get_all_logs();
        assert!(all.iter().any(|l| l.id == entry.id));

        let by_id = get_log_by_id(&entry.id);
        assert!(by_id.is_some());
        assert_eq!(by_id.unwrap().bill_id, 101);

        let for_bill = get_logs_for_bill(101);
        assert!(for_bill.iter().any(|l| l.id == entry.id));
    }

    #[actix_web::test]
    async fn test_async_action_logging_records_and_flushes_entries() {
        let unique_res = format!("TestLogResource_{}", generate_action_log_id());
        let kinds = [
            ResourceActionKind::Create,
            ResourceActionKind::Read,
            ResourceActionKind::Update,
            ResourceActionKind::Delete,
            ResourceActionKind::Search,
            ResourceActionKind::Login,
            ResourceActionKind::Logout,
        ];

        for (idx, kind) in kinds.iter().enumerate() {
            log_action_async(ActionLogEntry {
                id: generate_action_log_id(),
                action: kind.clone(),
                resource_name: Some(unique_res.clone()),
                resource_id: Some(idx.to_string()),
                performed_by: Some("tester@narayani.com".to_string()),
                user_id: Some(10),
                role: Some("Admin".to_string()),
                performed_at: Utc::now().to_rfc3339(),
                status_code: 200,
                success: true,
                duration_ms: 1,
                details: Some(format!("Action {}", kind)),
                metadata: None,
            });
        }

        flush_action_logs().await;

        let by_resource = get_action_logs_by_resource(&unique_res.to_lowercase());
        assert_eq!(by_resource.len(), kinds.len());

        let all = get_all_action_logs();
        for kind in &kinds {
            assert!(all
                .iter()
                .any(|e| e.resource_name.as_deref() == Some(unique_res.as_str()) && &e.action == kind));
        }

        let login_logs = get_action_logs_by_action(ResourceActionKind::Login);
        assert!(login_logs
            .iter()
            .any(|e| e.resource_name.as_deref() == Some(unique_res.as_str())));
    }

    #[actix_web::test]
    async fn test_async_logger_redacts_sensitive_metadata_in_background_worker() {
        let marker_id = generate_action_log_id();
        log_action_async(ActionLogEntry {
            id: marker_id.clone(),
            action: ResourceActionKind::Create,
            resource_name: Some("UserResource".to_string()),
            resource_id: Some("999".to_string()),
            performed_by: Some("admin@narayani.com".to_string()),
            user_id: Some(1),
            role: Some("Admin".to_string()),
            performed_at: Utc::now().to_rfc3339(),
            status_code: 200,
            success: true,
            duration_ms: 2,
            details: Some("Sensitive redaction check".to_string()),
            metadata: Some(serde_json::json!({
                "email": "user@test.com",
                "password": "SuperSecretPassword!",
                "nested": {
                    "token": "eyJhbGciOi...",
                    "accessToken": "secret_access",
                    "safeField": "visible"
                },
                "arr": [
                    { "newPassword": "abc", "note": "keep" }
                ]
            })),
        });

        flush_action_logs().await;

        let entry = get_all_action_logs()
            .into_iter()
            .find(|e| e.id == marker_id)
            .expect("log entry should exist after flush");
        let meta = entry.metadata.expect("metadata should exist");
        assert_eq!(meta["email"], "user@test.com");
        assert_eq!(meta["password"], "[REDACTED]");
        assert_eq!(meta["nested"]["token"], "[REDACTED]");
        assert_eq!(meta["nested"]["accessToken"], "[REDACTED]");
        assert_eq!(meta["nested"]["safeField"], "visible");
        assert_eq!(meta["arr"][0]["newPassword"], "[REDACTED]");
        assert_eq!(meta["arr"][0]["note"], "keep");
    }

    #[actix_web::test]
    async fn test_async_logger_persists_ndjson_to_file() {
        let temp_path = std::env::temp_dir().join(format!(
            "narayani_async_log_test_{}_{}.jsonl",
            std::process::id(),
            generate_action_log_id()
        ));
        let _ = std::fs::remove_file(&temp_path);

        let logger = AsyncActionLogger::spawn(Some(temp_path.clone()));
        for i in 1..=3 {
            logger.log(ActionLogEntry {
                id: format!("FILE-{i}"),
                action: ResourceActionKind::Update,
                resource_name: Some("SettingResource".to_string()),
                resource_id: Some(i.to_string()),
                performed_by: Some("admin@narayani.com".to_string()),
                user_id: Some(1),
                role: Some("Admin".to_string()),
                performed_at: Utc::now().to_rfc3339(),
                status_code: 200,
                success: true,
                duration_ms: 1,
                details: Some(format!("File test {i}")),
                metadata: Some(serde_json::json!({ "password": "secret", "branch": "Main" })),
            });
        }
        logger.flush().await;

        let contents = std::fs::read_to_string(&temp_path).expect("NDJSON file should exist");
        let _ = std::fs::remove_file(&temp_path);

        let parsed: Vec<ActionLogEntry> = contents
            .lines()
            .map(|line| serde_json::from_str::<ActionLogEntry>(line).expect("valid NDJSON line"))
            .collect();
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].id, "FILE-1");
        assert_eq!(parsed[2].id, "FILE-3");
        assert_eq!(
            parsed[0].metadata.as_ref().unwrap()["password"],
            "[REDACTED]"
        );
        assert_eq!(parsed[0].metadata.as_ref().unwrap()["branch"], "Main");
    }

    #[actix_web::test]
    async fn test_async_logging_burst_is_non_blocking_and_lossless() {
        let logger = AsyncActionLogger::spawn(None);
        for i in 0..200 {
            logger.log(ActionLogEntry {
                id: format!("BURST-{i}"),
                action: ResourceActionKind::Read,
                resource_name: Some("VehicleResource".to_string()),
                resource_id: Some(i.to_string()),
                performed_by: Some("operator@narayani.com".to_string()),
                user_id: Some(2),
                role: Some("Operator".to_string()),
                performed_at: Utc::now().to_rfc3339(),
                status_code: 200,
                success: true,
                duration_ms: 0,
                details: None,
                metadata: None,
            });
        }
        logger.flush().await;

        let entries = logger.all();
        assert_eq!(entries.len(), 200);
        assert_eq!(entries.first().unwrap().id, "BURST-0");
        assert_eq!(entries.last().unwrap().id, "BURST-199");
    }

    #[actix_web::test]
    async fn test_persistent_action_logger_persists_to_db_redacts_and_survives_restart() {
        let mut initial_state = crate::access::test_helpers::MockDbState::default();
        let seeded_id = "ACTLOG-2000000000000001".to_string();
        let seeded_entry = ActionLogEntry {
            id: seeded_id.clone(),
            action: ResourceActionKind::Login,
            resource_name: Some("UserResource".to_string()),
            resource_id: Some("1".to_string()),
            performed_by: Some("bootstrap@narayani.com".to_string()),
            user_id: Some(1),
            role: Some("Admin".to_string()),
            performed_at: "2026-11-23T05:00:00Z".to_string(),
            status_code: 200,
            success: true,
            duration_ms: 4,
            details: Some("Pre-existing DB audit row".to_string()),
            metadata: None,
        };
        initial_state.seed(
            ResourceName::ActionLogsResource,
            DatabaseResource {
                id: None,
                resource_name: Some(ResourceName::ActionLogsResource),
                inner_content: Some(serde_json::to_value(&seeded_entry).unwrap()),
                external_content: Some(serde_json::json!({ "createdTime": "2026-11-23T05:00:00Z" })),
            },
        );

        let (s_data_1, db_state_1) =
            crate::access::test_helpers::create_mock_server_data_with_state(initial_state).await;

        let logger_1 = AsyncActionLogger::spawn_persistent(
            s_data_1.db.clone(),
            Some(s_data_1.cache.clone()),
            None,
        )
        .await;

        assert_eq!(logger_1.all().len(), 1);
        assert_eq!(logger_1.all()[0].id, seeded_id);

        let post_boot_ids: Vec<String> = (0..4).map(|_| generate_action_log_id()).collect();
        for (i, kind) in [
            ResourceActionKind::Create,
            ResourceActionKind::Update,
            ResourceActionKind::Login,
            ResourceActionKind::Logout,
        ]
        .into_iter()
        .enumerate()
        {
            logger_1.log(ActionLogEntry {
                id: post_boot_ids[i].clone(),
                action: kind,
                resource_name: Some("BillResource".to_string()),
                resource_id: Some((100 + i).to_string()),
                performed_by: Some("admin@narayani.com".to_string()),
                user_id: Some(1),
                role: Some("Admin".to_string()),
                performed_at: Utc::now().to_rfc3339(),
                status_code: 200,
                success: true,
                duration_ms: 3,
                details: Some(format!("Action {i}")),
                metadata: Some(serde_json::json!({
                    "password": "TopSecretPassword!",
                    "nested": { "token": "secret-jwt" },
                    "safe": format!("item-{i}")
                })),
            });
        }
        logger_1.flush().await;

        {
            let cache = s_data_1.cache.read().await;
            let cached = cache
                .read_all(ResourceName::ActionLogsResource)
                .expect("ActionLogsResource cached");
            assert_eq!(cached.len(), 5);
        }

        let restart_snapshot = {
            let guard = db_state_1.lock().await;
            let table_name = ResourceName::ActionLogsResource.to_string().to_lowercase();
            let rows = guard
                .tables
                .get(&table_name)
                .expect("actionlogsresource table populated");
            assert_eq!(rows.len(), 5);
            let latest_inner = rows.last().unwrap().inner_content.as_ref().unwrap();
            assert_eq!(latest_inner["metadata"]["password"], "[REDACTED]");
            assert_eq!(latest_inner["metadata"]["nested"]["token"], "[REDACTED]");
            assert_eq!(latest_inner["metadata"]["safe"], "item-3");

            crate::access::test_helpers::MockDbState {
                tables: guard.tables.clone(),
                next_ids: guard.next_ids.clone(),
            }
        };
        drop(logger_1);

        // Simulate full server restart with fresh Cache + new logger backed by persisted DB rows
        let (s_data_2, db_state_2) =
            crate::access::test_helpers::create_mock_server_data_with_state(restart_snapshot)
                .await;
        {
            let mut cache = s_data_2.cache.write().await;
            cache.load_from_db(&s_data_2.db).await;
        }
        {
            let cache = s_data_2.cache.read().await;
            assert_eq!(
                cache
                    .read_all(ResourceName::ActionLogsResource)
                    .unwrap()
                    .len(),
                5
            );
        }

        let logger_2 = AsyncActionLogger::spawn_persistent(
            s_data_2.db.clone(),
            Some(s_data_2.cache.clone()),
            None,
        )
        .await;

        let hydrated = logger_2.all();
        assert_eq!(hydrated.len(), 5);
        assert_eq!(hydrated[0].id, seeded_id);
        for (idx, expected_id) in post_boot_ids.iter().enumerate() {
            assert_eq!(hydrated[idx + 1].id, *expected_id);
            assert_eq!(
                hydrated[idx + 1].metadata.as_ref().unwrap()["password"],
                "[REDACTED]"
            );
        }

        // Verify post-restart generated ID stays strictly above previously persisted IDs
        let after_restart_id = generate_action_log_id();
        let seeded_num: u64 = seeded_id
            .strip_prefix("ACTLOG-")
            .unwrap()
            .parse()
            .unwrap();
        let after_restart_num: u64 = after_restart_id
            .strip_prefix("ACTLOG-")
            .unwrap()
            .parse()
            .unwrap();
        assert!(after_restart_num > seeded_num);

        logger_2.log(ActionLogEntry {
            id: after_restart_id.clone(),
            action: ResourceActionKind::Delete,
            resource_name: Some("VehicleResource".to_string()),
            resource_id: Some("99".to_string()),
            performed_by: Some("admin@narayani.com".to_string()),
            user_id: Some(1),
            role: Some("Admin".to_string()),
            performed_at: Utc::now().to_rfc3339(),
            status_code: 200,
            success: true,
            duration_ms: 2,
            details: Some("Post-restart action".to_string()),
            metadata: None,
        });
        logger_2.flush().await;

        assert_eq!(logger_2.all().len(), 6);
        assert_eq!(logger_2.all().last().unwrap().id, after_restart_id);
        {
            let guard = db_state_2.lock().await;
            let table_name = ResourceName::ActionLogsResource.to_string().to_lowercase();
            assert_eq!(guard.tables.get(&table_name).unwrap().len(), 6);
        }
        {
            let cache = s_data_2.cache.read().await;
            assert_eq!(
                cache
                    .read_all(ResourceName::ActionLogsResource)
                    .unwrap()
                    .len(),
                6
            );
        }
    }

    #[actix_web::test]
    async fn test_persistent_action_logger_burst_and_malformed_row_resilience() {
        let mut state = crate::access::test_helpers::MockDbState::default();
        // Malformed row without ActionLogEntry fields
        state.seed(
            ResourceName::ActionLogsResource,
            DatabaseResource {
                id: None,
                resource_name: Some(ResourceName::ActionLogsResource),
                inner_content: Some(serde_json::json!({ "unexpected": 123 })),
                external_content: None,
            },
        );
        // Row with missing inner_content
        state.seed(
            ResourceName::ActionLogsResource,
            DatabaseResource {
                id: None,
                resource_name: Some(ResourceName::ActionLogsResource),
                inner_content: None,
                external_content: None,
            },
        );

        let (s_data_1, db_state_1) =
            crate::access::test_helpers::create_mock_server_data_with_state(state).await;
        let logger_1 = AsyncActionLogger::spawn_persistent(
            s_data_1.db.clone(),
            Some(s_data_1.cache.clone()),
            None,
        )
        .await;
        assert!(logger_1.all().is_empty());

        for i in 0..100 {
            logger_1.log(ActionLogEntry {
                id: format!("PERSIST-BURST-{i}"),
                action: ResourceActionKind::Read,
                resource_name: Some("VehicleResource".to_string()),
                resource_id: Some(i.to_string()),
                performed_by: Some("operator@narayani.com".to_string()),
                user_id: Some(2),
                role: Some("Operator".to_string()),
                performed_at: Utc::now().to_rfc3339(),
                status_code: 200,
                success: true,
                duration_ms: 1,
                details: None,
                metadata: None,
            });
        }
        logger_1.flush().await;
        drop(logger_1);

        let snapshot = {
            let guard = db_state_1.lock().await;
            let table_name = ResourceName::ActionLogsResource.to_string().to_lowercase();
            assert_eq!(guard.tables.get(&table_name).unwrap().len(), 102);
            crate::access::test_helpers::MockDbState {
                tables: guard.tables.clone(),
                next_ids: guard.next_ids.clone(),
            }
        };

        let (s_data_2, _) =
            crate::access::test_helpers::create_mock_server_data_with_state(snapshot).await;
        let logger_2 = AsyncActionLogger::spawn_persistent(
            s_data_2.db.clone(),
            Some(s_data_2.cache.clone()),
            None,
        )
        .await;

        let hydrated = logger_2.all();
        assert_eq!(hydrated.len(), 100);
        assert_eq!(hydrated.first().unwrap().id, "PERSIST-BURST-0");
        assert_eq!(hydrated.last().unwrap().id, "PERSIST-BURST-99");
    }
}

