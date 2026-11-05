use crate::resource::{DatabaseResource, ResourceName};
use crate::ServerData;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{OnceLock, RwLock};

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

static LOG_STORE: OnceLock<RwLock<Vec<BillLogEntry>>> = OnceLock::new();
static LOG_COUNTER: OnceLock<std::sync::atomic::AtomicU64> = OnceLock::new();

pub fn get_log_store() -> &'static RwLock<Vec<BillLogEntry>> {
    LOG_STORE.get_or_init(|| RwLock::new(Vec::new()))
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
    store.clone()
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
}

