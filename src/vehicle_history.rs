use crate::dashboard_statistics::{extract_string_or_int, normalize_vehicle_number};
use crate::resource::{DatabaseResource, ResourceName};
use crate::ServerData;
use actix_web::{web, HttpMessage, HttpRequest, HttpResponse};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

pub const VEHICLE_HISTORY_SCHEMA_VERSION: u32 = 1;

static VEHICLE_UID_COUNTER: OnceLock<AtomicU64> = OnceLock::new();

fn vehicle_uid_counter() -> &'static AtomicU64 {
    VEHICLE_UID_COUNTER.get_or_init(|| {
        let initial = u64::try_from(Utc::now().timestamp_micros())
            .unwrap_or(1000)
            .max(1000);
        AtomicU64::new(initial)
    })
}

pub fn generate_vehicle_uid() -> String {
    let seq = vehicle_uid_counter().fetch_add(1, Ordering::Relaxed);
    format!("VHIST-{}", seq)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VehicleChangeType {
    Create,
    Update,
    Delete,
    IdentityChange,
    VehicleNumberChange,
    Sync,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VehicleNumberChangeKind {
    CanonicalFormatAlias,
    ZoneToProvinceTransition,
    PlateNumberChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VehicleNumberOnlyChange {
    pub kind: VehicleNumberChangeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_number: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_number: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_canonical: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_canonical: Option<String>,
}

pub fn is_vehicle_number_field(field: &str) -> bool {
    matches!(
        field,
        "VehicleNumber" | "vehicleNumber" | "NormalizedVehicleNumber" | "normalizedVehicleNumber"
    )
}

pub fn is_vehicle_number_search_field(field_name: &str) -> bool {
    let trimmed = field_name.trim();
    trimmed.eq_ignore_ascii_case("VehicleNumber")
        || trimmed.eq_ignore_ascii_case("NormalizedVehicleNumber")
}

fn extract_raw_vehicle_number(inner: &Value) -> Option<String> {
    inner
        .get("VehicleNumber")
        .or_else(|| inner.get("vehicleNumber"))
        .or_else(|| inner.get("NormalizedVehicleNumber"))
        .or_else(|| inner.get("normalizedVehicleNumber"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

/// Detects when a change between `before` and `after` modified ONLY vehicle number field(s)
/// while leaving all other fields untouched. Does not mutate any vehicle data.
pub fn detect_vehicle_number_only_change(
    before: Option<&Value>,
    after: Option<&Value>,
) -> Option<VehicleNumberOnlyChange> {
    let diffs = compute_field_diffs(before, after);
    if diffs.is_empty() || diffs.iter().any(|d| !is_vehicle_number_field(&d.field)) {
        return None;
    }

    let previous_number = before.and_then(extract_raw_vehicle_number);
    let new_number = after.and_then(extract_raw_vehicle_number);

    let prev_parsed = previous_number
        .as_deref()
        .and_then(crate::vehicle_number::parse_vehicle_number);
    let new_parsed = new_number
        .as_deref()
        .and_then(crate::vehicle_number::parse_vehicle_number);

    let previous_canonical = prev_parsed.as_ref().map(|v| v.to_string());
    let new_canonical = new_parsed.as_ref().map(|v| v.to_string());

    let kind = match (prev_parsed.as_ref(), new_parsed.as_ref()) {
        (Some(p), Some(n)) if p == n => VehicleNumberChangeKind::CanonicalFormatAlias,
        (Some(p), Some(n))
            if crate::vehicle_number::is_zone_to_province_transition_parsed(p, n) =>
        {
            VehicleNumberChangeKind::ZoneToProvinceTransition
        }
        _ => VehicleNumberChangeKind::PlateNumberChanged,
    };

    Some(VehicleNumberOnlyChange {
        kind,
        previous_number,
        new_number,
        previous_canonical,
        new_canonical,
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VehicleFieldDiff {
    pub field: String,
    pub before: Value,
    pub after: Value,
}

/// Single node in the per-vehicle JSON linked list (`head -> next -> ... -> null`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VehicleHistoryNode {
    pub node_id: String,
    pub sequence: u64,
    pub change_type: VehicleChangeType,
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_id: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vehicle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vehicle_number: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normalized_vehicle_number: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_vehicle_number: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number_change_kind: Option<VehicleNumberChangeKind>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed_fields: Vec<VehicleFieldDiff>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Linked-list pointer to the immediately preceding state (`null` at the initial/tail node).
    pub next: Option<Box<VehicleHistoryNode>>,
}

/// Linked-list container stored inside `VehicleHistoryResource.inner_content` (`serde_json::Value`).
/// Unique to a single physical vehicle (`vehicleUid`) even if both `VehicleId` and `VehicleNumber` change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VehicleHistoryChain {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub vehicle_uid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_db_id: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_vehicle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_vehicle_number: Option<String>,
    #[serde(default)]
    pub known_db_ids: Vec<i32>,
    #[serde(default)]
    pub known_vehicle_ids: Vec<String>,
    #[serde(default)]
    pub known_vehicle_numbers: Vec<String>,
    #[serde(default)]
    pub known_canonical_vehicle_numbers: Vec<String>,
    #[serde(default)]
    pub node_count: usize,
    pub head: Option<Box<VehicleHistoryNode>>,
}

fn default_schema_version() -> u32 {
    VEHICLE_HISTORY_SCHEMA_VERSION
}

fn push_unique_i32(list: &mut Vec<i32>, value: i32) {
    if !list.contains(&value) {
        list.push(value);
    }
}

fn push_unique_str(list: &mut Vec<String>, value: &str) {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return;
    }
    if !list.iter().any(|existing| existing.eq_ignore_ascii_case(trimmed)) {
        list.push(trimmed.to_string());
    }
}

pub fn extract_vehicle_identifiers(
    inner: &Value,
) -> (Option<String>, Option<String>, Option<String>) {
    let vehicle_id = extract_string_or_int(inner.get("VehicleId"))
        .or_else(|| extract_string_or_int(inner.get("vehicleId")))
        .or_else(|| extract_string_or_int(inner.get("id")));

    let vehicle_number = inner
        .get("VehicleNumber")
        .or_else(|| inner.get("vehicleNumber"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);

    let normalized_vehicle_number = vehicle_number
        .as_deref()
        .and_then(normalize_vehicle_number)
        .or_else(|| {
            inner
                .get("NormalizedVehicleNumber")
                .or_else(|| inner.get("normalizedVehicleNumber"))
                .and_then(Value::as_str)
                .and_then(normalize_vehicle_number)
        });

    (vehicle_id, vehicle_number, normalized_vehicle_number)
}

pub fn compute_field_diffs(before: Option<&Value>, after: Option<&Value>) -> Vec<VehicleFieldDiff> {
    let (Some(Value::Object(b_map)), Some(Value::Object(a_map))) = (before, after) else {
        return Vec::new();
    };

    let mut keys: BTreeSet<&str> = BTreeSet::new();
    for k in b_map.keys() {
        keys.insert(k.as_str());
    }
    for k in a_map.keys() {
        keys.insert(k.as_str());
    }

    let mut diffs = Vec::new();
    for key in keys {
        let b_val = b_map.get(key);
        let a_val = a_map.get(key);
        if b_val != a_val {
            diffs.push(VehicleFieldDiff {
                field: key.to_string(),
                before: b_val.cloned().unwrap_or(Value::Null),
                after: a_val.cloned().unwrap_or(Value::Null),
            });
        }
    }
    diffs
}

impl VehicleHistoryChain {
    pub fn new(vehicle_uid: String) -> Self {
        Self {
            schema_version: VEHICLE_HISTORY_SCHEMA_VERSION,
            vehicle_uid,
            current_db_id: None,
            current_vehicle_id: None,
            current_vehicle_number: None,
            known_db_ids: Vec::new(),
            known_vehicle_ids: Vec::new(),
            known_vehicle_numbers: Vec::new(),
            known_canonical_vehicle_numbers: Vec::new(),
            node_count: 0,
            head: None,
        }
    }

    pub fn to_json_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({}))
    }

    pub fn from_json_value(value: &Value) -> Result<Self, String> {
        serde_json::from_value(value.clone())
            .map_err(|e| format!("Invalid VehicleHistoryResource inner_content: {}", e))
    }

    pub fn traverse_nodes(&self) -> Vec<&VehicleHistoryNode> {
        let mut nodes = Vec::with_capacity(self.node_count);
        let mut cursor = self.head.as_deref();
        while let Some(node) = cursor {
            nodes.push(node);
            cursor = node.next.as_deref();
        }
        nodes
    }

    pub fn register_identifiers(
        &mut self,
        db_id: Option<i32>,
        vehicle_id: Option<&str>,
        norm_vehicle_number: Option<&str>,
    ) {
        if let Some(id) = db_id {
            push_unique_i32(&mut self.known_db_ids, id);
        }
        if let Some(vid) = vehicle_id {
            push_unique_str(&mut self.known_vehicle_ids, vid);
        }
        if let Some(num) = norm_vehicle_number {
            push_unique_str(&mut self.known_vehicle_numbers, num);
        }
    }

    pub fn register_canonical_vehicle_number(&mut self, raw_number: Option<&str>) {
        if let Some(canon) = raw_number.and_then(crate::vehicle_number::canonical_vehicle_number) {
            push_unique_str(&mut self.known_canonical_vehicle_numbers, &canon);
        }
    }

    pub fn prepend_node(
        &mut self,
        change_type: VehicleChangeType,
        db_id: Option<i32>,
        before_snapshot: Option<&Value>,
        after_snapshot: Option<&Value>,
        changed_by: Option<String>,
        reason: Option<String>,
    ) {
        if let Some(before) = before_snapshot {
            let (b_vid, b_num, b_norm) = extract_vehicle_identifiers(before);
            self.register_identifiers(db_id, b_vid.as_deref(), b_norm.as_deref());
            self.register_canonical_vehicle_number(b_num.as_deref());
        }

        if let Some(after) = after_snapshot {
            let (_, a_num, _) = extract_vehicle_identifiers(after);
            self.register_canonical_vehicle_number(a_num.as_deref());
        }

        let effective_snapshot = after_snapshot.or(before_snapshot);
        let (v_id, v_num, norm_num) = effective_snapshot
            .map(extract_vehicle_identifiers)
            .unwrap_or((None, None, None));
        self.register_identifiers(db_id, v_id.as_deref(), norm_num.as_deref());
        let canonical_num = v_num
            .as_deref()
            .and_then(crate::vehicle_number::canonical_vehicle_number);
        if let Some(ref c) = canonical_num {
            push_unique_str(&mut self.known_canonical_vehicle_numbers, c);
        }

        let number_change_kind = if change_type == VehicleChangeType::VehicleNumberChange {
            detect_vehicle_number_only_change(before_snapshot, after_snapshot).map(|d| d.kind)
        } else {
            None
        };

        let changed_fields = compute_field_diffs(before_snapshot, after_snapshot);
        let sequence = (self.node_count as u64) + 1;
        let node = VehicleHistoryNode {
            node_id: format!("VNODE-{}", sequence),
            sequence,
            change_type: change_type.clone(),
            timestamp: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            db_id,
            vehicle_id: v_id.clone(),
            vehicle_number: v_num.clone(),
            normalized_vehicle_number: norm_num,
            canonical_vehicle_number: canonical_num,
            number_change_kind,
            changed_fields,
            snapshot: effective_snapshot.cloned(),
            changed_by,
            reason,
            next: self.head.take(),
        };

        self.head = Some(Box::new(node));
        self.node_count += 1;

        if change_type != VehicleChangeType::Delete {
            if let Some(id) = db_id {
                self.current_db_id = Some(id);
            }
            if let Some(vid) = v_id {
                self.current_vehicle_id = Some(vid);
            }
            if let Some(num) = v_num {
                self.current_vehicle_number = Some(num);
            }
        }
    }

    pub fn has_vehicle_number_only_change(&self) -> bool {
        self.traverse_nodes().into_iter().any(|n| {
            n.change_type == VehicleChangeType::VehicleNumberChange
                || n.number_change_kind.is_some()
        })
    }

    pub fn matches_vehicle_number_raw(&self, raw_number: &str) -> bool {
        let trimmed = raw_number.trim();
        if trimmed.is_empty() {
            return false;
        }
        let norm = normalize_vehicle_number(trimmed);
        if self.matches_identifier(None, None, norm.as_deref(), None) {
            return true;
        }

        let Some(query_canon) = crate::vehicle_number::canonical_vehicle_number(trimmed) else {
            return false;
        };

        if self
            .known_canonical_vehicle_numbers
            .iter()
            .any(|k| k.eq_ignore_ascii_case(&query_canon))
        {
            return true;
        }

        if self
            .current_vehicle_number
            .as_deref()
            .and_then(crate::vehicle_number::canonical_vehicle_number)
            .is_some_and(|c| c.eq_ignore_ascii_case(&query_canon))
        {
            return true;
        }

        let mut cursor = self.head.as_deref();
        while let Some(node) = cursor {
            if node
                .canonical_vehicle_number
                .as_deref()
                .is_some_and(|c| c.eq_ignore_ascii_case(&query_canon))
                || node
                    .vehicle_number
                    .as_deref()
                    .and_then(crate::vehicle_number::canonical_vehicle_number)
                    .is_some_and(|c| c.eq_ignore_ascii_case(&query_canon))
            {
                return true;
            }
            cursor = node.next.as_deref();
        }

        false
    }

    pub fn matches_identifier(
        &self,
        db_id: Option<i32>,
        vehicle_id: Option<&str>,
        norm_vehicle_number: Option<&str>,
        vehicle_uid: Option<&str>,
    ) -> bool {
        if let Some(uid) = vehicle_uid.map(str::trim).filter(|s| !s.is_empty()) {
            if self.vehicle_uid.eq_ignore_ascii_case(uid) {
                return true;
            }
        }

        if let Some(id) = db_id {
            if self.current_db_id == Some(id) || self.known_db_ids.contains(&id) {
                return true;
            }
        }

        if let Some(vid) = vehicle_id.map(str::trim).filter(|s| !s.is_empty()) {
            if self
                .current_vehicle_id
                .as_deref()
                .is_some_and(|c| c.eq_ignore_ascii_case(vid))
                || self
                    .known_vehicle_ids
                    .iter()
                    .any(|k| k.eq_ignore_ascii_case(vid))
            {
                return true;
            }
        }

        if let Some(norm) = norm_vehicle_number.map(str::trim).filter(|s| !s.is_empty()) {
            if self
                .current_vehicle_number
                .as_deref()
                .and_then(normalize_vehicle_number)
                .as_deref()
                .is_some_and(|c| c == norm)
                || self
                    .known_vehicle_numbers
                    .iter()
                    .any(|k| k.eq_ignore_ascii_case(norm))
            {
                return true;
            }
        }

        let mut cursor = self.head.as_deref();
        while let Some(node) = cursor {
            if let Some(id) = db_id {
                if node.db_id == Some(id) {
                    return true;
                }
            }
            if let Some(vid) = vehicle_id.map(str::trim).filter(|s| !s.is_empty()) {
                if node
                    .vehicle_id
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(vid))
                {
                    return true;
                }
            }
            if let Some(norm) = norm_vehicle_number.map(str::trim).filter(|s| !s.is_empty()) {
                if node
                    .normalized_vehicle_number
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(norm))
                    || node
                        .vehicle_number
                        .as_deref()
                        .and_then(normalize_vehicle_number)
                        .as_deref()
                        .is_some_and(|n| n == norm)
                {
                    return true;
                }
            }
            cursor = node.next.as_deref();
        }

        false
    }
}

/// Extracts all current and historical `(db_ids, vehicle_ids, normalized_vehicle_numbers)` from a
/// `VehicleHistoryResource` JSON `Value` by checking summary indexes and walking the linked list
/// (`head -> next -> ... -> null`). Current identifiers are placed first.
pub(crate) fn extract_all_identifiers_from_history(
    inner: &Value,
) -> (Vec<i32>, Vec<String>, Vec<String>) {
    let mut db_ids = Vec::new();
    let mut vehicle_ids = Vec::new();
    let mut norm_numbers = Vec::new();

    if let Ok(chain) = VehicleHistoryChain::from_json_value(inner) {
        if let Some(id) = chain.current_db_id {
            push_unique_i32(&mut db_ids, id);
        }
        if let Some(ref vid) = chain.current_vehicle_id {
            push_unique_str(&mut vehicle_ids, vid);
        }
        if let Some(ref num) = chain.current_vehicle_number {
            if let Some(norm) = normalize_vehicle_number(num) {
                push_unique_str(&mut norm_numbers, &norm);
            }
            if let Some(canon_norm) = crate::vehicle_number::canonical_normalized_key(num) {
                push_unique_str(&mut norm_numbers, &canon_norm);
            }
        }
        for id in &chain.known_db_ids {
            push_unique_i32(&mut db_ids, *id);
        }
        for vid in &chain.known_vehicle_ids {
            push_unique_str(&mut vehicle_ids, vid);
        }
        for num in &chain.known_vehicle_numbers {
            if let Some(norm) = normalize_vehicle_number(num) {
                push_unique_str(&mut norm_numbers, &norm);
            }
        }
        for canon in &chain.known_canonical_vehicle_numbers {
            if let Some(norm) = normalize_vehicle_number(canon) {
                push_unique_str(&mut norm_numbers, &norm);
            }
            if let Some(canon_norm) = crate::vehicle_number::canonical_normalized_key(canon) {
                push_unique_str(&mut norm_numbers, &canon_norm);
            }
        }
        for node in chain.traverse_nodes() {
            if let Some(id) = node.db_id {
                push_unique_i32(&mut db_ids, id);
            }
            if let Some(ref vid) = node.vehicle_id {
                push_unique_str(&mut vehicle_ids, vid);
            }
            if let Some(ref norm) = node.normalized_vehicle_number {
                if let Some(n) = normalize_vehicle_number(norm) {
                    push_unique_str(&mut norm_numbers, &n);
                }
            }
            if let Some(ref num) = node.vehicle_number {
                if let Some(n) = normalize_vehicle_number(num) {
                    push_unique_str(&mut norm_numbers, &n);
                }
                if let Some(canon_norm) = crate::vehicle_number::canonical_normalized_key(num) {
                    push_unique_str(&mut norm_numbers, &canon_norm);
                }
            }
            if let Some(ref canon) = node.canonical_vehicle_number {
                if let Some(n) = normalize_vehicle_number(canon) {
                    push_unique_str(&mut norm_numbers, &n);
                }
            }
        }
        return (db_ids, vehicle_ids, norm_numbers);
    }

    // Fallback raw JSON linked-list traversal for hand-crafted VehicleHistoryResource documents
    if let Some(arr) = inner
        .get("knownCanonicalVehicleNumbers")
        .and_then(Value::as_array)
    {
        for v in arr {
            if let Some(raw) = v.as_str() {
                if let Some(n) = normalize_vehicle_number(raw) {
                    push_unique_str(&mut norm_numbers, &n);
                }
                if let Some(canon_norm) = crate::vehicle_number::canonical_normalized_key(raw) {
                    push_unique_str(&mut norm_numbers, &canon_norm);
                }
            }
        }
    }
    let mut cursor = inner.get("head");
    while let Some(Value::Object(node)) = cursor {
        if let Some(id) = node.get("dbId").and_then(Value::as_i64).and_then(|v| i32::try_from(v).ok()) {
            push_unique_i32(&mut db_ids, id);
        }
        if let Some(vid) = extract_string_or_int(node.get("vehicleId"))
            .or_else(|| extract_string_or_int(node.get("VehicleId")))
        {
            push_unique_str(&mut vehicle_ids, &vid);
        }
        for k in [
            "normalizedVehicleNumber",
            "canonicalVehicleNumber",
            "vehicleNumber",
            "VehicleNumber",
        ] {
            if let Some(raw) = node.get(k).and_then(Value::as_str) {
                if let Some(norm) = normalize_vehicle_number(raw) {
                    push_unique_str(&mut norm_numbers, &norm);
                }
                if let Some(canon_norm) = crate::vehicle_number::canonical_normalized_key(raw) {
                    push_unique_str(&mut norm_numbers, &canon_norm);
                }
            }
        }
        cursor = node.get("next");
    }

    (db_ids, vehicle_ids, norm_numbers)
}

async fn load_vehicle_histories(s_data: &ServerData) -> Vec<DatabaseResource> {
    {
        let cache = s_data.cache.read().await;
        if let Ok(items) = cache.read_all(ResourceName::VehicleHistoryResource) {
            if !items.is_empty() || cache.is_fully_loaded(ResourceName::VehicleHistoryResource) {
                return items.into_iter().cloned().collect();
            }
        }
    }

    match s_data
        .db
        .read_all_resource(ResourceName::VehicleHistoryResource)
        .await
    {
        Ok(rows) => {
            let mut cache = s_data.cache.write().await;
            for row in &rows {
                if let Some(id) = row.id {
                    cache.write(ResourceName::VehicleHistoryResource, id, row.clone());
                }
            }
            cache.mark_fully_loaded(ResourceName::VehicleHistoryResource);
            rows
        }
        Err(_) => Vec::new(),
    }
}

/// Records a vehicle lifecycle or identity change onto the vehicle's `VehicleHistoryResource`
/// linked list in PostgreSQL and `Cache`.
#[allow(clippy::too_many_arguments)]
pub async fn record_vehicle_change(
    s_data: &ServerData,
    change_type: VehicleChangeType,
    db_id: Option<i32>,
    before_state: Option<&Value>,
    after_state: Option<&Value>,
    changed_by: Option<String>,
    reason: Option<String>,
    explicit_vehicle_uid: Option<&str>,
) -> Result<DatabaseResource, String> {
    let (b_vid, b_num, b_norm) = before_state
        .map(extract_vehicle_identifiers)
        .unwrap_or((None, None, None));
    let (a_vid, a_num, a_norm) = after_state
        .map(extract_vehicle_identifiers)
        .unwrap_or((None, None, None));

    let (effective_change_type, effective_reason) = if change_type == VehicleChangeType::Update {
        if let Some(detected) = detect_vehicle_number_only_change(before_state, after_state) {
            let auto_reason = reason.or_else(|| {
                Some(format!(
                    "Vehicle number changed only ({:?}): '{}' -> '{}'",
                    detected.kind,
                    detected.previous_number.as_deref().unwrap_or(""),
                    detected.new_number.as_deref().unwrap_or("")
                ))
            });
            (VehicleChangeType::VehicleNumberChange, auto_reason)
        } else {
            (change_type, reason)
        }
    } else {
        (change_type, reason)
    };

    let existing_histories = load_vehicle_histories(s_data).await;

    let matched = existing_histories.into_iter().find_map(|res| {
        let inner = res.inner_content.as_ref()?;
        let chain = VehicleHistoryChain::from_json_value(inner).ok()?;
        let hit = chain.matches_identifier(
            db_id,
            b_vid.as_deref(),
            b_norm.as_deref(),
            explicit_vehicle_uid,
        ) || chain.matches_identifier(
            db_id,
            a_vid.as_deref(),
            a_norm.as_deref(),
            explicit_vehicle_uid,
        ) || b_num
            .as_deref()
            .is_some_and(|n| chain.matches_vehicle_number_raw(n))
            || a_num
                .as_deref()
                .is_some_and(|n| chain.matches_vehicle_number_raw(n));
        if hit {
            Some((res, chain))
        } else {
            None
        }
    });

    if let Some((existing_res, mut chain)) = matched {
        if matches!(
            effective_change_type,
            VehicleChangeType::Update | VehicleChangeType::VehicleNumberChange
        ) && before_state.is_some()
            && before_state == after_state
        {
            return Ok(existing_res);
        }

        let Some(hist_id) = existing_res.id else {
            return Err("Matched VehicleHistoryResource is missing id".to_string());
        };

        chain.prepend_node(
            effective_change_type,
            db_id,
            before_state,
            after_state,
            changed_by,
            effective_reason,
        );

        let updated_res = DatabaseResource {
            id: Some(hist_id),
            resource_name: Some(ResourceName::VehicleHistoryResource),
            inner_content: Some(chain.to_json_value()),
            external_content: existing_res.external_content,
        };

        s_data
            .db
            .update_resource(
                ResourceName::VehicleHistoryResource,
                hist_id,
                updated_res.clone(),
            )
            .await
            .map_err(|e| format!("Failed to update VehicleHistoryResource {}: {:?}", hist_id, e))?;

        s_data
            .cache
            .write()
            .await
            .write(ResourceName::VehicleHistoryResource, hist_id, updated_res.clone());

        return Ok(updated_res);
    }

    let uid = explicit_vehicle_uid
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_else(generate_vehicle_uid);
    let mut chain = VehicleHistoryChain::new(uid);

    if matches!(
        effective_change_type,
        VehicleChangeType::Update
            | VehicleChangeType::Delete
            | VehicleChangeType::IdentityChange
            | VehicleChangeType::VehicleNumberChange
    ) && before_state.is_some()
    {
        chain.prepend_node(
            VehicleChangeType::Create,
            db_id,
            None,
            before_state,
            changed_by.clone(),
            Some("Initial state captured prior to change".to_string()),
        );
    }

    chain.prepend_node(
        effective_change_type,
        db_id,
        before_state,
        after_state,
        changed_by,
        effective_reason,
    );

    let mut new_res = DatabaseResource {
        id: None,
        resource_name: Some(ResourceName::VehicleHistoryResource),
        inner_content: Some(chain.to_json_value()),
        external_content: None,
    };

    let created_id = s_data
        .db
        .create_resource(ResourceName::VehicleHistoryResource, new_res.clone())
        .await
        .map_err(|e| format!("Failed to create VehicleHistoryResource: {:?}", e))?;

    new_res.id = Some(created_id);
    s_data
        .cache
        .write()
        .await
        .write(ResourceName::VehicleHistoryResource, created_id, new_res.clone());

    Ok(new_res)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordVehicleHistoryRequest {
    pub vehicle_uid: Option<String>,
    pub previous_db_id: Option<i32>,
    pub previous_vehicle_id: Option<String>,
    pub previous_vehicle_number: Option<String>,
    pub new_db_id: Option<i32>,
    pub new_vehicle_id: Option<String>,
    pub new_vehicle_number: Option<String>,
    pub snapshot: Option<Value>,
    pub change_type: Option<VehicleChangeType>,
    pub reason: Option<String>,
}

pub async fn list_vehicle_histories(
    req: HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    if let crate::permissions::PermissionCheckOutcome::Denied { message, .. } =
        crate::permissions::enforce_request_permission(
            &req,
            &s_data,
            "VehicleHistoryResource",
            crate::permissions::action::READ,
            crate::permissions::extract_branch_id_from_req(&req),
        )
        .await
    {
        return HttpResponse::Forbidden().body(message);
    }

    let histories = load_vehicle_histories(&s_data).await;
    HttpResponse::Ok().json(histories)
}

pub async fn get_vehicle_history_by_identifier(
    path: web::Path<String>,
    req: HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    if let crate::permissions::PermissionCheckOutcome::Denied { message, .. } =
        crate::permissions::enforce_request_permission(
            &req,
            &s_data,
            "VehicleHistoryResource",
            crate::permissions::action::READ,
            crate::permissions::extract_branch_id_from_req(&req),
        )
        .await
    {
        return HttpResponse::Forbidden().body(message);
    }

    let raw_identifier = path.into_inner();
    let trimmed = raw_identifier.trim();
    if trimmed.is_empty() {
        return HttpResponse::BadRequest().json(json!({
            "success": false,
            "message": "Vehicle identifier is required"
        }));
    }

    let parsed_db_id = trimmed.parse::<i32>().ok();
    let norm_num = normalize_vehicle_number(trimmed);
    let histories = load_vehicle_histories(&s_data).await;

    let found = histories.into_iter().find(|res| {
        res.inner_content
            .as_ref()
            .and_then(|inner| VehicleHistoryChain::from_json_value(inner).ok())
            .is_some_and(|chain| {
                chain.matches_identifier(
                    parsed_db_id,
                    Some(trimmed),
                    norm_num.as_deref(),
                    Some(trimmed),
                ) || chain.matches_vehicle_number_raw(trimmed)
            })
    });

    match found {
        Some(res) => HttpResponse::Ok().json(res),
        None => HttpResponse::NotFound().json(json!({
            "success": false,
            "message": format!("No VehicleHistoryResource found for identifier '{}'", trimmed)
        })),
    }
}

pub async fn record_vehicle_history_endpoint(
    body: web::Json<RecordVehicleHistoryRequest>,
    req: HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let payload = body.into_inner();

    if let crate::permissions::PermissionCheckOutcome::Denied { message, .. } =
        crate::permissions::enforce_request_permission(
            &req,
            &s_data,
            "VehicleHistoryResource",
            crate::permissions::action::UPDATE,
            crate::permissions::extract_branch_id_from_req(&req),
        )
        .await
    {
        return HttpResponse::Forbidden().body(message);
    }

    let actor = req
        .extensions()
        .get::<crate::auth::Claims>()
        .map(|c| c.sub.clone());

    let mut before_obj = serde_json::Map::new();
    if let Some(ref vid) = payload.previous_vehicle_id {
        before_obj.insert("VehicleId".to_string(), Value::String(vid.clone()));
    }
    if let Some(ref vnum) = payload.previous_vehicle_number {
        before_obj.insert("VehicleNumber".to_string(), Value::String(vnum.clone()));
    }
    let before_val = if before_obj.is_empty() {
        None
    } else {
        Some(Value::Object(before_obj))
    };

    let after_val = match payload.snapshot {
        Some(Value::Object(mut map)) => {
            if let Some(ref vid) = payload.new_vehicle_id {
                map.entry("VehicleId".to_string())
                    .or_insert_with(|| Value::String(vid.clone()));
            }
            if let Some(ref vnum) = payload.new_vehicle_number {
                map.entry("VehicleNumber".to_string())
                    .or_insert_with(|| Value::String(vnum.clone()));
            }
            Some(Value::Object(map))
        }
        Some(other) => Some(other),
        None => {
            let mut after_obj = serde_json::Map::new();
            if let Some(ref vid) = payload.new_vehicle_id {
                after_obj.insert("VehicleId".to_string(), Value::String(vid.clone()));
            }
            if let Some(ref vnum) = payload.new_vehicle_number {
                after_obj.insert("VehicleNumber".to_string(), Value::String(vnum.clone()));
            }
            if after_obj.is_empty() {
                None
            } else {
                Some(Value::Object(after_obj))
            }
        }
    };

    if before_val.is_none()
        && after_val.is_none()
        && payload.vehicle_uid.as_deref().map(str::trim).unwrap_or("").is_empty()
    {
        return HttpResponse::BadRequest().json(json!({
            "success": false,
            "message": "Provide at least one vehicle identifier or snapshot"
        }));
    }

    // If previous_db_id was provided alongside a different new_db_id, match using previous_db_id first
    // so both DB IDs get registered on the same chain.
    if let (Some(prev_db), Some(new_db)) = (payload.previous_db_id, payload.new_db_id) {
        if prev_db != new_db {
            if let Ok(mut existing) = record_vehicle_change(
                &s_data,
                VehicleChangeType::Update,
                Some(prev_db),
                before_val.as_ref(),
                before_val.as_ref(),
                actor.clone(),
                None,
                payload.vehicle_uid.as_deref(),
            )
            .await
            {
                let uid = existing
                    .inner_content
                    .as_ref()
                    .and_then(|v| v.get("vehicleUid"))
                    .and_then(Value::as_str)
                    .map(String::from);
                let _ = &mut existing;
                return match record_vehicle_change(
                    &s_data,
                    payload
                        .change_type
                        .unwrap_or(VehicleChangeType::IdentityChange),
                    Some(new_db),
                    before_val.as_ref(),
                    after_val.as_ref(),
                    actor,
                    payload.reason,
                    uid.as_deref().or(payload.vehicle_uid.as_deref()),
                )
                .await
                {
                    Ok(res) => HttpResponse::Ok().json(res),
                    Err(err) => HttpResponse::InternalServerError().body(err),
                };
            }
        }
    }

    let effective_db_id = payload.new_db_id.or(payload.previous_db_id);
    let change_type = payload.change_type.unwrap_or_else(|| {
        if detect_vehicle_number_only_change(before_val.as_ref(), after_val.as_ref()).is_some() {
            VehicleChangeType::VehicleNumberChange
        } else if before_val.is_some() {
            VehicleChangeType::IdentityChange
        } else {
            VehicleChangeType::Update
        }
    });

    match record_vehicle_change(
        &s_data,
        change_type,
        effective_db_id,
        before_val.as_ref(),
        after_val.as_ref(),
        actor,
        payload.reason,
        payload.vehicle_uid.as_deref(),
    )
    .await
    {
        Ok(res) => HttpResponse::Ok().json(res),
        Err(err) => HttpResponse::InternalServerError().body(err),
    }
}

/// Read-only expansion index built from `vehicle_number` canonical parsing and cached
/// `VehicleHistoryResource` chains to broaden vehicle search without altering stored vehicle data.
#[derive(Debug, Default)]
pub struct VehicleSearchExpansion {
    pub query_raw: String,
    pub query_norm: String,
    pub query_parsed: Option<crate::vehicle_number::VehicleNumber>,
    pub query_canonical_norm: Option<String>,
    pub matched_db_ids: HashSet<i32>,
    pub matched_vehicle_ids: HashSet<String>,
    pub matched_norm_numbers: HashSet<String>,
    pub matched_canonical_norms: HashSet<String>,
}

impl VehicleSearchExpansion {
    pub fn from_cache(cache: &crate::cache::Cache, query: &str) -> Self {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Self::default();
        }

        let query_norm = normalize_vehicle_number(trimmed).unwrap_or_default();
        let query_parsed = crate::vehicle_number::parse_vehicle_number(trimmed);
        let query_canonical_norm = crate::vehicle_number::canonical_normalized_key(trimmed);
        let parsed_db_id = trimmed.parse::<i32>().ok();

        let mut matched_db_ids = HashSet::new();
        let mut matched_vehicle_ids = HashSet::new();
        let mut matched_norm_numbers = HashSet::new();
        let mut matched_canonical_norms = HashSet::new();

        if !query_norm.is_empty() {
            matched_norm_numbers.insert(query_norm.clone());
        }
        if let Some(ref c) = query_canonical_norm {
            matched_canonical_norms.insert(c.clone());
        }

        if let Ok(histories) = cache.read_all(ResourceName::VehicleHistoryResource) {
            for res in histories {
                let Some(inner) = res.inner_content.as_ref() else {
                    continue;
                };
                let Ok(chain) = VehicleHistoryChain::from_json_value(inner) else {
                    continue;
                };
                let hit = chain.matches_identifier(
                    parsed_db_id,
                    Some(trimmed),
                    if query_norm.is_empty() {
                        None
                    } else {
                        Some(query_norm.as_str())
                    },
                    Some(trimmed),
                ) || chain.matches_vehicle_number_raw(trimmed);

                if !hit {
                    continue;
                }

                let (db_ids, vids, norms) = extract_all_identifiers_from_history(inner);
                matched_db_ids.extend(db_ids);
                for vid in vids {
                    matched_vehicle_ids.insert(vid);
                }
                for norm in norms {
                    matched_norm_numbers.insert(norm);
                }
                for canon in &chain.known_canonical_vehicle_numbers {
                    if let Some(n) = normalize_vehicle_number(canon) {
                        matched_canonical_norms.insert(n);
                    }
                    if let Some(c) = crate::vehicle_number::canonical_normalized_key(canon) {
                        matched_canonical_norms.insert(c);
                    }
                }
                for node in chain.traverse_nodes() {
                    if let Some(ref num) = node.vehicle_number {
                        if let Some(c) = crate::vehicle_number::canonical_normalized_key(num) {
                            matched_canonical_norms.insert(c);
                        }
                    }
                }
            }
        }

        Self {
            query_raw: trimmed.to_string(),
            query_norm,
            query_parsed,
            query_canonical_norm,
            matched_db_ids,
            matched_vehicle_ids,
            matched_norm_numbers,
            matched_canonical_norms,
        }
    }

    pub fn matches_vehicle_inner(&self, db_id: Option<i32>, inner: &Value) -> bool {
        if self.query_raw.is_empty() {
            return false;
        }

        if let Some(db_id) = db_id {
            if self.matched_db_ids.contains(&db_id) {
                return true;
            }
        }

        let (vid, vnum, vnorm) = extract_vehicle_identifiers(inner);

        if let Some(vid_str) = vid.as_deref() {
            if self
                .matched_vehicle_ids
                .iter()
                .any(|m| m.eq_ignore_ascii_case(vid_str))
            {
                return true;
            }
        }

        let raw_num = vnum
            .as_deref()
            .or_else(|| {
                inner
                    .get("NormalizedVehicleNumber")
                    .or_else(|| inner.get("normalizedVehicleNumber"))
                    .and_then(Value::as_str)
            });

        if let (Some(q_vn), Some(raw)) = (self.query_parsed.as_ref(), raw_num) {
            if crate::vehicle_number::parse_vehicle_number(raw).as_ref() == Some(q_vn) {
                return true;
            }
        }

        if let Some(norm) = vnorm.as_deref() {
            if self.matched_norm_numbers.contains(norm)
                || self.matched_canonical_norms.contains(norm)
            {
                return true;
            }
        }

        if let Some(canon_norm) = raw_num.and_then(crate::vehicle_number::canonical_normalized_key)
        {
            if self.matched_canonical_norms.contains(&canon_norm)
                || self.matched_norm_numbers.contains(&canon_norm)
            {
                return true;
            }
        }

        false
    }

    pub fn matches_vehicle_resource(&self, res: &DatabaseResource) -> bool {
        res.inner_content
            .as_ref()
            .is_some_and(|inner| self.matches_vehicle_inner(res.id, inner))
    }
}

pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.route("", web::get().to(list_vehicle_histories))
        .route("/record", web::post().to(record_vehicle_history_endpoint))
        .route(
            "/{identifier}",
            web::get().to(get_vehicle_history_by_identifier),
        );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::test_helpers::{create_mock_server_data_with_state, MockDbState};
    use crate::auth::Claims;
    use crate::roles::Roles;
    use actix_web::{http::StatusCode, test as actix_test, App};

    #[test]
    fn test_vehicle_history_linked_list_json_value_across_id_and_number_changes() {
        let mut chain = VehicleHistoryChain::new("VHIST-TEST-1".to_string());

        let v1 = json!({
            "VehicleId": "1001",
            "VehicleNumber": "BA-01-PA-1001",
            "VehicleType": "Truck"
        });
        chain.prepend_node(
            VehicleChangeType::Create,
            Some(10),
            None,
            Some(&v1),
            Some("admin@narayani.com".to_string()),
            Some("Initial registration".to_string()),
        );

        // Both VehicleId AND VehicleNumber change simultaneously
        let v2 = json!({
            "VehicleId": "9999",
            "VehicleNumber": "GA-02-KHA-9999",
            "VehicleType": "Container Truck"
        });
        chain.prepend_node(
            VehicleChangeType::IdentityChange,
            Some(10),
            Some(&v1),
            Some(&v2),
            Some("admin@narayani.com".to_string()),
            Some("Re-registered with new plate and ID".to_string()),
        );

        let as_json = chain.to_json_value();

        // Verify explicit linked list structure in JSON Value: head -> next -> null
        assert_eq!(as_json["vehicleUid"], "VHIST-TEST-1");
        assert_eq!(as_json["nodeCount"], 2);
        assert_eq!(as_json["head"]["sequence"], 2);
        assert_eq!(as_json["head"]["vehicleId"], "9999");
        assert_eq!(as_json["head"]["vehicleNumber"], "GA-02-KHA-9999");
        assert_eq!(as_json["head"]["next"]["sequence"], 1);
        assert_eq!(as_json["head"]["next"]["vehicleId"], "1001");
        assert_eq!(as_json["head"]["next"]["vehicleNumber"], "BA-01-PA-1001");
        assert!(as_json["head"]["next"]["next"].is_null());

        let diffs = as_json["head"]["changedFields"].as_array().unwrap();
        assert_eq!(diffs.len(), 3);

        // Roundtrip and lookup by both old and new ID/number/uid
        let parsed = VehicleHistoryChain::from_json_value(&as_json).unwrap();
        assert_eq!(parsed.traverse_nodes().len(), 2);
        assert!(parsed.matches_identifier(None, Some("1001"), None, None));
        assert!(parsed.matches_identifier(None, Some("9999"), None, None));
        assert!(parsed.matches_identifier(None, None, Some("BA01PA1001"), None));
        assert!(parsed.matches_identifier(None, None, Some("GA02KHA9999"), None));
        assert!(parsed.matches_identifier(Some(10), None, None, None));
        assert!(parsed.matches_identifier(None, None, None, Some("VHIST-TEST-1")));

        let (db_ids, vids, nums) = extract_all_identifiers_from_history(&as_json);
        assert_eq!(db_ids, vec![10]);
        assert!(vids.contains(&"1001".to_string()));
        assert!(vids.contains(&"9999".to_string()));
        assert!(nums.contains(&"BA01PA1001".to_string()));
        assert!(nums.contains(&"GA02KHA9999".to_string()));
    }

    #[actix_web::test]
    async fn test_vehicle_actions_and_history_endpoints_track_full_lifecycle() {
        let state = MockDbState::default();
        let (s_data, db_state) = create_mock_server_data_with_state(state).await;

        let make_admin_req = || {
            let req = actix_test::TestRequest::default().to_http_request();
            req.extensions_mut().insert(Claims {
                sub: "admin@narayani.com".to_string(),
                exp: 9999999999,
                iat: 1000000000,
                role: Roles::Admin,
                user_id: Some(1),
                jti: None,
                role_name: Some("Admin".to_string()),
                tenant_id: Some("loadboard-sthaniya".to_string()),
            });
            req
        };

        // 1. Create VehicleResource via actions::write -> creates VehicleHistoryResource node 1
        let create_resp = crate::actions::write(
            web::Path::from(ResourceName::VehicleResource),
            web::Json(DatabaseResource {
                id: None,
                resource_name: Some(ResourceName::VehicleResource),
                inner_content: Some(json!({
                    "VehicleId": "35930",
                    "VehicleNumber": "State-3-G-KK-8211",
                    "VehicleType": "Truck"
                })),
                external_content: None,
            }),
            make_admin_req(),
            s_data.clone(),
        )
        .await;
        assert_eq!(create_resp.status(), StatusCode::OK);
        let created_vehicle: DatabaseResource = serde_json::from_slice(
            &actix_web::body::to_bytes(create_resp.into_body())
                .await
                .unwrap(),
        )
        .unwrap();
        let v_db_id = created_vehicle.id.unwrap();

        // 2. Update VehicleResource changing BOTH VehicleId AND VehicleNumber -> prepends node 2
        let update_resp = crate::actions::update(
            web::Path::from((ResourceName::VehicleResource, v_db_id)),
            web::Json(DatabaseResource {
                id: Some(v_db_id),
                resource_name: Some(ResourceName::VehicleResource),
                inner_content: Some(json!({
                    "VehicleId": "88888",
                    "VehicleNumber": "Province-3-01-KA-8888",
                    "VehicleType": "Heavy Truck"
                })),
                external_content: None,
            }),
            make_admin_req(),
            s_data.clone(),
        )
        .await;
        assert_eq!(update_resp.status(), StatusCode::OK);

        // 3. Delete VehicleResource -> prepends node 3 (DELETE) to the SAME linked list
        let delete_resp = crate::actions::delete(
            web::Path::from((ResourceName::VehicleResource, v_db_id)),
            s_data.clone(),
            make_admin_req(),
        )
        .await;
        assert_eq!(delete_resp.status(), StatusCode::OK);

        // Verify exactly ONE VehicleHistoryResource exists in DB for this single vehicle
        {
            let guard = db_state.lock().await;
            let hist_rows = guard.tables.get("vehiclehistoryresource").unwrap();
            assert_eq!(hist_rows.len(), 1);
            let inner = hist_rows[0].inner_content.as_ref().unwrap();
            assert_eq!(inner["nodeCount"], 3);
            assert_eq!(inner["head"]["changeType"], "DELETE");
            assert_eq!(inner["head"]["next"]["changeType"], "UPDATE");
            assert_eq!(inner["head"]["next"]["vehicleId"], "88888");
            assert_eq!(inner["head"]["next"]["next"]["changeType"], "CREATE");
            assert_eq!(inner["head"]["next"]["next"]["vehicleId"], "35930");
            assert!(inner["head"]["next"]["next"]["next"].is_null());
        }

        // 4. Query HTTP endpoints using old ID, new ID, old number, and new number -> all return the same vehicleUid
        let app = actix_test::init_service(
            App::new()
                .app_data(s_data.clone())
                .service(web::scope("/api/v1/vehicles/history").configure(configure_routes)),
        )
        .await;

        let mut uids = Vec::new();
        for identifier in ["35930", "88888", "State-3-G-KK-8211", "Province-3-01-KA-8888"] {
            let resp = actix_test::call_service(
                &app,
                actix_test::TestRequest::get()
                    .uri(&format!("/api/v1/vehicles/history/{}", identifier))
                    .to_request(),
            )
            .await;
            assert_eq!(resp.status(), StatusCode::OK, "lookup failed for {identifier}");
            let hist_res: DatabaseResource = actix_test::read_body_json(resp).await;
            let uid = hist_res.inner_content.unwrap()["vehicleUid"]
                .as_str()
                .unwrap()
                .to_string();
            uids.push(uid);
        }
        assert!(uids.windows(2).all(|w| w[0] == w[1]));

        // 5. Explicitly link another identity change via POST /api/v1/vehicles/history/record
        let record_resp = actix_test::call_service(
            &app,
            actix_test::TestRequest::post()
                .uri("/api/v1/vehicles/history/record")
                .set_json(&json!({
                    "previousVehicleId": "88888",
                    "newVehicleId": "99999",
                    "newVehicleNumber": "Bagmati-01-GA-9999",
                    "reason": "Manual number plate transfer"
                }))
                .to_request(),
        )
        .await;
        assert_eq!(record_resp.status(), StatusCode::OK);
        let updated_hist: DatabaseResource = actix_test::read_body_json(record_resp).await;
        let updated_inner = updated_hist.inner_content.unwrap();
        assert_eq!(updated_inner["vehicleUid"], uids[0]);
        assert_eq!(updated_inner["nodeCount"], 4);
        assert_eq!(updated_inner["head"]["changeType"], "IDENTITY_CHANGE");
        assert_eq!(updated_inner["head"]["vehicleId"], "99999");
        assert_eq!(updated_inner["head"]["next"]["changeType"], "DELETE");
    }

    #[test]
    fn test_detect_vehicle_number_only_change_classifications() {
        let base = json!({
            "VehicleId": "24631",
            "VehicleNumber": "Ga Pra 15-23-KHA-5555",
            "VehicleType": "Truck"
        });
        let alias = json!({
            "VehicleId": "24631",
            "VehicleNumber": "Province-4-15-23-KHA-5555",
            "VehicleType": "Truck"
        });
        let detected_alias =
            detect_vehicle_number_only_change(Some(&base), Some(&alias)).expect("alias change");
        assert_eq!(
            detected_alias.kind,
            VehicleNumberChangeKind::CanonicalFormatAlias
        );
        assert_eq!(
            detected_alias.previous_canonical.as_deref(),
            Some("Province-4-15-23-KHA-5555")
        );
        assert_eq!(
            detected_alias.new_canonical.as_deref(),
            Some("Province-4-15-23-KHA-5555")
        );

        let zonal_old = json!({
            "VehicleId": "1001",
            "VehicleNumber": "BA-23-KHA-5555",
            "VehicleType": "Truck"
        });
        let provincial_new = json!({
            "VehicleId": "1001",
            "VehicleNumber": "Province-3-01-23-KHA-5555",
            "VehicleType": "Truck"
        });
        let detected_z2p = detect_vehicle_number_only_change(Some(&zonal_old), Some(&provincial_new))
            .expect("zone-to-province transition");
        assert_eq!(
            detected_z2p.kind,
            VehicleNumberChangeKind::ZoneToProvinceTransition
        );

        let plate_changed = json!({
            "VehicleId": "1001",
            "VehicleNumber": "NA-04-KHA-9999",
            "VehicleType": "Truck"
        });
        let detected_plate =
            detect_vehicle_number_only_change(Some(&zonal_old), Some(&plate_changed))
                .expect("plate number changed");
        assert_eq!(
            detected_plate.kind,
            VehicleNumberChangeKind::PlateNumberChanged
        );

        // If another field also changed, it must NOT be classified as vehicle-number-only
        let multi_field_change = json!({
            "VehicleId": "1001",
            "VehicleNumber": "Province-3-01-23-KHA-5555",
            "VehicleType": "Heavy Truck"
        });
        assert!(detect_vehicle_number_only_change(Some(&zonal_old), Some(&multi_field_change)).is_none());

        let id_also_changed = json!({
            "VehicleId": "9999",
            "VehicleNumber": "Province-3-01-23-KHA-5555",
            "VehicleType": "Truck"
        });
        assert!(detect_vehicle_number_only_change(Some(&zonal_old), Some(&id_also_changed)).is_none());
    }

    #[actix_web::test]
    async fn test_actions_update_vehicle_number_only_detects_history_without_mutating_stored_format() {
        let state = MockDbState::default();
        let (s_data, db_state) = create_mock_server_data_with_state(state).await;

        let make_admin_req = || {
            let req = actix_test::TestRequest::default().to_http_request();
            req.extensions_mut().insert(Claims {
                sub: "admin@narayani.com".to_string(),
                exp: 9999999999,
                iat: 1000000000,
                role: Roles::Admin,
                user_id: Some(1),
                jti: None,
                role_name: Some("Admin".to_string()),
                tenant_id: Some("loadboard-sthaniya".to_string()),
            });
            req
        };

        // 1. Create vehicle with zonal plate
        let create_resp = crate::actions::write(
            web::Path::from(ResourceName::VehicleResource),
            web::Json(DatabaseResource {
                id: None,
                resource_name: Some(ResourceName::VehicleResource),
                inner_content: Some(json!({
                    "VehicleId": "77001",
                    "VehicleNumber": "BA-23-KHA-5555",
                    "VehicleType": "Truck"
                })),
                external_content: None,
            }),
            make_admin_req(),
            s_data.clone(),
        )
        .await;
        assert_eq!(create_resp.status(), StatusCode::OK);
        let created_vehicle: DatabaseResource = serde_json::from_slice(
            &actix_web::body::to_bytes(create_resp.into_body())
                .await
                .unwrap(),
        )
        .unwrap();
        let v_db_id = created_vehicle.id.unwrap();

        // 2. Update ONLY VehicleNumber -> should record VEHICLE_NUMBER_CHANGE without rewriting stored VehicleNumber format
        let update_resp = crate::actions::update(
            web::Path::from((ResourceName::VehicleResource, v_db_id)),
            web::Json(DatabaseResource {
                id: Some(v_db_id),
                resource_name: Some(ResourceName::VehicleResource),
                inner_content: Some(json!({
                    "VehicleId": "77001",
                    "VehicleNumber": "Ba Pra 01-23-KHA-5555",
                    "VehicleType": "Truck"
                })),
                external_content: None,
            }),
            make_admin_req(),
            s_data.clone(),
        )
        .await;
        assert_eq!(update_resp.status(), StatusCode::OK);

        // Verify stored VehicleResource preserves exact submitted format ("Ba Pra 01-23-KHA-5555")
        {
            let cache = s_data.cache.read().await;
            let cached_vehicle = cache.read(ResourceName::VehicleResource, v_db_id).unwrap();
            assert_eq!(
                cached_vehicle.inner_content.as_ref().unwrap()["VehicleNumber"],
                "Ba Pra 01-23-KHA-5555"
            );
        }

        // Verify VehicleHistoryResource captured VEHICLE_NUMBER_CHANGE & ZONE_TO_PROVINCE_TRANSITION
        {
            let guard = db_state.lock().await;
            let hist_rows = guard.tables.get("vehiclehistoryresource").unwrap();
            assert_eq!(hist_rows.len(), 1);
            let inner = hist_rows[0].inner_content.as_ref().unwrap();
            assert_eq!(inner["nodeCount"], 2);
            assert_eq!(inner["head"]["changeType"], "VEHICLE_NUMBER_CHANGE");
            assert_eq!(inner["head"]["numberChangeKind"], "ZONE_TO_PROVINCE_TRANSITION");
            assert_eq!(inner["head"]["canonicalVehicleNumber"], "Province-3-1-23-KHA-5555");
            let chain = VehicleHistoryChain::from_json_value(inner).unwrap();
            assert!(chain.has_vehicle_number_only_change());
        }

        // Verify HTTP history lookup works by canonical alias of the new plate ("Province-3-01-23-KHA-5555")
        // as well as old zonal plate ("Bagmati 23 KHA 5555")
        let app = actix_test::init_service(
            App::new()
                .app_data(s_data.clone())
                .service(web::scope("/api/v1/vehicles/history").configure(configure_routes)),
        )
        .await;

        for alias in ["Province-3-01-23-KHA-5555", "Bagmati 23 KHA 5555"] {
            let encoded = alias.replace(' ', "%20");
            let resp = actix_test::call_service(
                &app,
                actix_test::TestRequest::get()
                    .uri(&format!("/api/v1/vehicles/history/{}", encoded))
                    .to_request(),
            )
            .await;
            assert_eq!(resp.status(), StatusCode::OK, "alias lookup failed for {alias}");
        }
    }
}
