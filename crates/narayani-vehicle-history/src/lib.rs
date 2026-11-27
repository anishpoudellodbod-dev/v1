use chrono::Utc;
use narayani_core::{extract_string_or_int, normalize_vehicle_number, DatabaseResource};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap, HashSet};
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
        .and_then(narayani_plate::parse_vehicle_number);
    let new_parsed = new_number
        .as_deref()
        .and_then(narayani_plate::parse_vehicle_number);

    let previous_canonical = prev_parsed.as_ref().map(|v| v.to_string());
    let new_canonical = new_parsed.as_ref().map(|v| v.to_string());

    let kind = match (prev_parsed.as_ref(), new_parsed.as_ref()) {
        (Some(p), Some(n)) if p == n => VehicleNumberChangeKind::CanonicalFormatAlias,
        (Some(p), Some(n))
            if narayani_plate::is_zone_to_province_transition_parsed(p, n) =>
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
        if let Some(canon) = raw_number.and_then(narayani_plate::canonical_vehicle_number) {
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
            .and_then(narayani_plate::canonical_vehicle_number);
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

        let Some(query_canon) = narayani_plate::canonical_vehicle_number(trimmed) else {
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
            .and_then(narayani_plate::canonical_vehicle_number)
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
                    .and_then(narayani_plate::canonical_vehicle_number)
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
pub fn extract_all_identifiers_from_history(
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
            if let Some(canon_norm) = narayani_plate::canonical_normalized_key(num) {
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
            if let Some(canon_norm) = narayani_plate::canonical_normalized_key(canon) {
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
                if let Some(canon_norm) = narayani_plate::canonical_normalized_key(num) {
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
                if let Some(canon_norm) = narayani_plate::canonical_normalized_key(raw) {
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
                if let Some(canon_norm) = narayani_plate::canonical_normalized_key(raw) {
                    push_unique_str(&mut norm_numbers, &canon_norm);
                }
            }
        }
        cursor = node.get("next");
    }

    (db_ids, vehicle_ids, norm_numbers)
}

/// Read-only expansion index built from `vehicle_number` canonical parsing and cached
/// `VehicleHistoryResource` chains to broaden vehicle search without altering stored vehicle data.
#[derive(Debug, Default)]
pub struct VehicleSearchExpansion {
    pub query_raw: String,
    pub query_norm: String,
    pub query_parsed: Option<narayani_plate::VehicleNumber>,
    pub query_canonical_norm: Option<String>,
    pub matched_db_ids: HashSet<i32>,
    pub matched_vehicle_ids: HashSet<String>,
    pub matched_norm_numbers: HashSet<String>,
    pub matched_canonical_norms: HashSet<String>,
}

impl VehicleSearchExpansion {
    pub fn from_histories<'a, I>(histories: I, query: &str) -> Self
    where
        I: IntoIterator<Item = &'a DatabaseResource>,
    {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Self::default();
        }

        let query_norm = normalize_vehicle_number(trimmed).unwrap_or_default();
        let query_parsed = narayani_plate::parse_vehicle_number(trimmed);
        let query_canonical_norm = narayani_plate::canonical_normalized_key(trimmed);
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
                if let Some(c) = narayani_plate::canonical_normalized_key(canon) {
                    matched_canonical_norms.insert(c);
                }
            }
            for node in chain.traverse_nodes() {
                if let Some(ref num) = node.vehicle_number {
                    if let Some(c) = narayani_plate::canonical_normalized_key(num) {
                        matched_canonical_norms.insert(c);
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
            if narayani_plate::parse_vehicle_number(raw).as_ref() == Some(q_vn) {
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

        if let Some(canon_norm) = raw_num.and_then(narayani_plate::canonical_normalized_key)
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

#[derive(Debug, Clone)]
pub struct VehicleAliasTarget {
    pub db_id: Option<i32>,
    pub vehicle_id: String,
    pub vehicle_number: String,
    pub norm_vehicle_number: String,
    pub vehicle_type: String,
    pub owner_name: String,
    pub sangh_branch: String,
    pub is_displaced: bool,
    pub previous_vehicle_ids: Vec<String>,
    pub previous_vehicle_numbers: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AliasMatchSource {
    PreviousVehicleId,
    PreviousVehicleNumber,
    CurrentVehicleId,
    CurrentVehicleNumber,
    CanonicalVehicleNumber,
}

#[derive(Debug, Clone, Copy)]
pub struct ResolvedVehicleAlias<'a> {
    pub target: &'a Arc<VehicleAliasTarget>,
    pub source: AliasMatchSource,
    pub is_previous: bool,
}

fn extract_str_from_val(v: Option<&Value>) -> String {
    v.and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("")
        .to_string()
}

fn extract_vehicle_owner_name(inner: &Value) -> String {
    if let Some(owners) = inner.get("Owners").or_else(|| inner.get("owners")).and_then(Value::as_array) {
        let mut names = Vec::new();
        for o in owners {
            let n = extract_str_from_val(o.get("Name").or_else(|| o.get("name")));
            if !n.is_empty() && !names.iter().any(|x: &String| x.eq_ignore_ascii_case(&n)) {
                names.push(n);
            }
        }
        if !names.is_empty() {
            return names.join(", ");
        }
    }
    extract_str_from_val(
        inner
            .get("OwnerName")
            .or_else(|| inner.get("ownerName"))
            .or_else(|| inner.get("Name"))
            .or_else(|| inner.get("name")),
    )
}

fn extract_vehicle_sangh_branch(inner: &Value) -> String {
    for key in ["SanghBranch", "sanghBranch", "sthaniyaBranch"] {
        let s = extract_str_from_val(inner.get(key));
        if !s.is_empty() {
            return s;
        }
    }
    if let Some(owners) = inner.get("Owners").or_else(|| inner.get("owners")).and_then(Value::as_array) {
        for o in owners {
            for key in ["SanghBranch", "sanghBranch", "sthaniyaBranch"] {
                let s = extract_str_from_val(o.get(key));
                if !s.is_empty() {
                    return s;
                }
            }
        }
    }
    String::new()
}

fn extract_vehicle_array_aliases(inner: &Value) -> (Vec<String>, Vec<String>) {
    let mut prev_ids = Vec::new();
    let mut prev_nums = Vec::new();
    if let Some(arr) = inner.get("previousVehicleIds").and_then(Value::as_array) {
        for v in arr {
            if let Some(s) = extract_string_or_int(Some(v)) {
                push_unique_str(&mut prev_ids, &s);
            }
        }
    }
    if let Some(arr) = inner.get("previousVehicleNumbers").and_then(Value::as_array) {
        for v in arr {
            if let Some(s) = v.as_str() {
                push_unique_str(&mut prev_nums, s);
            }
        }
    }
    if let Some(arr) = inner.get("reconciliationAudit").and_then(Value::as_array) {
        for entry in arr {
            for k in ["previousVehicleId", "oldVehicleId", "vehicleId"] {
                if let Some(s) = extract_string_or_int(entry.get(k)) {
                    push_unique_str(&mut prev_ids, &s);
                }
            }
            for k in ["previousVehicleNumber", "oldVehicleNumber", "vehicleNumber"] {
                if let Some(s) = entry.get(k).and_then(Value::as_str) {
                    push_unique_str(&mut prev_nums, s);
                }
            }
        }
    }
    (prev_ids, prev_nums)
}

/// O(1) priority-ordered alias resolver used by reports and vehicle reconciliation:
/// 1. `previousVehicleIds` (`reconciliationAudit` / ownership history / `VehicleHistoryResource`)
/// 2. `previousVehicleNumbers` (normalized)
/// 3. Current `VehicleId`
/// 4. Current `VehicleNumber` (normalized)
/// 5. Current canonical plate (`narayani_plate::canonical_normalized_key`)
#[derive(Debug, Clone, Default)]
pub struct VehicleAliasMap {
    by_prev_vehicle_id: HashMap<Box<str>, Arc<VehicleAliasTarget>>,
    by_prev_plate_norm: HashMap<Box<str>, Arc<VehicleAliasTarget>>,
    by_curr_vehicle_id: HashMap<Box<str>, Arc<VehicleAliasTarget>>,
    by_curr_plate_norm: HashMap<Box<str>, Arc<VehicleAliasTarget>>,
    by_curr_canonical_norm: HashMap<Box<str>, Arc<VehicleAliasTarget>>,
    by_db_id: HashMap<i32, Arc<VehicleAliasTarget>>,
}

impl VehicleAliasMap {
    pub fn build<'a, IV, IH>(vehicles: IV, histories: IH) -> Self
    where
        IV: IntoIterator<Item = &'a DatabaseResource>,
        IH: IntoIterator<Item = &'a DatabaseResource>,
    {
        let mut map = Self::default();
        let mut targets: Vec<Arc<VehicleAliasTarget>> = Vec::new();

        for res in vehicles {
            let Some(inner) = res.inner_content.as_ref() else {
                continue;
            };
            let (vid, vnum, vnorm) = extract_vehicle_identifiers(inner);
            let vehicle_id = vid.unwrap_or_default();
            let vehicle_number = vnum.unwrap_or_default();
            let norm_vehicle_number = vnorm.unwrap_or_default();
            let vehicle_type = extract_str_from_val(
                inner.get("VehicleType").or_else(|| inner.get("vehicleType")),
            );
            let owner_name = extract_vehicle_owner_name(inner);
            let sangh_branch = extract_vehicle_sangh_branch(inner);
            let is_displaced = inner
                .get("isDisplaced")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || inner
                    .get("status")
                    .and_then(Value::as_str)
                    .is_some_and(|s| s.trim().eq_ignore_ascii_case("DISPLACED"));
            let (prev_ids, prev_nums) = extract_vehicle_array_aliases(inner);

            let target = Arc::new(VehicleAliasTarget {
                db_id: res.id,
                vehicle_id: vehicle_id.clone(),
                vehicle_number: vehicle_number.clone(),
                norm_vehicle_number: norm_vehicle_number.clone(),
                vehicle_type,
                owner_name,
                sangh_branch,
                is_displaced,
                previous_vehicle_ids: prev_ids.clone(),
                previous_vehicle_numbers: prev_nums.clone(),
            });

            if let Some(id) = res.id {
                map.by_db_id.insert(id, Arc::clone(&target));
            }
            if !vehicle_id.is_empty() {
                map.by_curr_vehicle_id
                    .entry(vehicle_id.to_ascii_lowercase().into_boxed_str())
                    .or_insert_with(|| Arc::clone(&target));
            }
            if !norm_vehicle_number.is_empty() {
                map.by_curr_plate_norm
                    .entry(norm_vehicle_number.clone().into_boxed_str())
                    .or_insert_with(|| Arc::clone(&target));
            }
            if let Some(canon) = narayani_plate::canonical_normalized_key(&vehicle_number) {
                map.by_curr_canonical_norm
                    .entry(canon.into_boxed_str())
                    .or_insert_with(|| Arc::clone(&target));
            }
            for pid in &prev_ids {
                if !pid.eq_ignore_ascii_case(&vehicle_id) {
                    map.by_prev_vehicle_id
                        .entry(pid.trim().to_ascii_lowercase().into_boxed_str())
                        .or_insert_with(|| Arc::clone(&target));
                }
            }
            for pnum in &prev_nums {
                if let Some(pnorm) = normalize_vehicle_number(pnum) {
                    if pnorm != norm_vehicle_number {
                        map.by_prev_plate_norm
                            .entry(pnorm.into_boxed_str())
                            .or_insert_with(|| Arc::clone(&target));
                    }
                }
            }
            targets.push(target);
        }

        // Link VehicleHistoryResource historical nodes onto resolved targets
        for hres in histories {
            let Some(inner) = hres.inner_content.as_ref() else {
                continue;
            };
            let Ok(chain) = VehicleHistoryChain::from_json_value(inner) else {
                continue;
            };
            let target_opt = chain
                .current_db_id
                .and_then(|id| map.by_db_id.get(&id).cloned())
                .or_else(|| {
                    chain.current_vehicle_id.as_deref().and_then(|vid| {
                        map.by_curr_vehicle_id
                            .get(vid.trim().to_ascii_lowercase().as_str())
                            .cloned()
                    })
                })
                .or_else(|| {
                    chain
                        .current_vehicle_number
                        .as_deref()
                        .and_then(normalize_vehicle_number)
                        .and_then(|norm| map.by_curr_plate_norm.get(norm.as_str()).cloned())
                });
            let Some(target) = target_opt else {
                continue;
            };
            let (_, vids, norms) = extract_all_identifiers_from_history(inner);
            for vid in vids {
                if !vid.eq_ignore_ascii_case(&target.vehicle_id) {
                    map.by_prev_vehicle_id
                        .entry(vid.trim().to_ascii_lowercase().into_boxed_str())
                        .or_insert_with(|| Arc::clone(&target));
                }
            }
            for norm in norms {
                if norm != target.norm_vehicle_number {
                    map.by_prev_plate_norm
                        .entry(norm.into_boxed_str())
                        .or_insert_with(|| Arc::clone(&target));
                }
            }
        }

        map
    }

    pub fn get_by_db_id(&self, db_id: i32) -> Option<&Arc<VehicleAliasTarget>> {
        self.by_db_id.get(&db_id)
    }

    /// Resolves `(vehicle_id, vehicle_number)` in strict priority order:
    /// 1. `previousVehicleIds`, 2. `previousVehicleNumbers`, 3. `VehicleId`, 4. `VehicleNumber`, 5. canonical plate.
    pub fn resolve(
        &self,
        vehicle_id: Option<&str>,
        vehicle_number: Option<&str>,
    ) -> Option<ResolvedVehicleAlias<'_>> {
        if let Some(vid) = vehicle_id.map(str::trim).filter(|s| !s.is_empty()) {
            let key = vid.to_ascii_lowercase();
            if let Some(target) = self.by_prev_vehicle_id.get(key.as_str()) {
                return Some(ResolvedVehicleAlias {
                    target,
                    source: AliasMatchSource::PreviousVehicleId,
                    is_previous: true,
                });
            }
        }
        let plate_norm = vehicle_number.and_then(normalize_vehicle_number);
        if let Some(ref norm) = plate_norm {
            if let Some(target) = self.by_prev_plate_norm.get(norm.as_str()) {
                return Some(ResolvedVehicleAlias {
                    target,
                    source: AliasMatchSource::PreviousVehicleNumber,
                    is_previous: true,
                });
            }
        }
        if let Some(vid) = vehicle_id.map(str::trim).filter(|s| !s.is_empty()) {
            let key = vid.to_ascii_lowercase();
            if let Some(target) = self.by_curr_vehicle_id.get(key.as_str()) {
                return Some(ResolvedVehicleAlias {
                    target,
                    source: AliasMatchSource::CurrentVehicleId,
                    is_previous: false,
                });
            }
        }
        if let Some(ref norm) = plate_norm {
            if let Some(target) = self.by_curr_plate_norm.get(norm.as_str()) {
                return Some(ResolvedVehicleAlias {
                    target,
                    source: AliasMatchSource::CurrentVehicleNumber,
                    is_previous: false,
                });
            }
        }
        if let Some(canon) = vehicle_number.and_then(narayani_plate::canonical_normalized_key) {
            if let Some(target) = self.by_curr_canonical_norm.get(canon.as_str()) {
                return Some(ResolvedVehicleAlias {
                    target,
                    source: AliasMatchSource::CanonicalVehicleNumber,
                    is_previous: false,
                });
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use narayani_core::ResourceName;

    #[test]
    fn alias_map_resolves_previous_before_current() {
        let v1 = DatabaseResource {
            id: Some(10),
            resource_name: Some(ResourceName::VehicleResource),
            inner_content: Some(json!({
                "VehicleId": "V-NEW",
                "VehicleNumber": "Province-3-01-001-KA-2000",
                "VehicleType": "10 pangree",
                "previousVehicleIds": ["V-OLD"],
                "previousVehicleNumbers": ["Na-1-KA-1000"]
            })),
            external_content: None,
        };
        let map = VehicleAliasMap::build([&v1], std::iter::empty());

        let by_prev_id = map.resolve(Some("v-old"), None).expect("match prev id");
        assert_eq!(by_prev_id.target.vehicle_id, "V-NEW");
        assert_eq!(by_prev_id.source, AliasMatchSource::PreviousVehicleId);
        assert!(by_prev_id.is_previous);

        let by_prev_num = map.resolve(None, Some("Na 1 KA 1000")).expect("match prev num");
        assert_eq!(by_prev_num.target.vehicle_id, "V-NEW");
        assert_eq!(by_prev_num.source, AliasMatchSource::PreviousVehicleNumber);

        let by_curr = map.resolve(Some("V-NEW"), None).expect("match curr id");
        assert_eq!(by_curr.source, AliasMatchSource::CurrentVehicleId);
        assert!(!by_curr.is_previous);
    }
}
