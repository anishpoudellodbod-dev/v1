use crate::resource::{DatabaseResource, ResourceName::VehicleResource};
use crate::ServerData;
use actix_web::{error::ErrorInternalServerError, http::header, web, Error, HttpRequest, HttpResponse};
use anyhow::{bail, Context, Result};
use bytes::Bytes;
use futures_util::{stream, StreamExt};
pub use narayani_integrations::tqms::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{atomic::{AtomicUsize, Ordering}, Arc};
use std::time::{Duration, Instant};

fn normalize_vehicle_num(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().filter(|c| c.is_alphanumeric()) {
        for upper in c.to_uppercase() {
            out.push(upper);
        }
    }
    out
}

fn contains_case_insensitive(haystack: &str, needle: &str) -> bool {
    let h = haystack.trim();
    let n = needle.trim();
    if n.is_empty() {
        return true;
    }
    if h.is_ascii() && n.is_ascii() {
        h.as_bytes()
            .windows(n.len())
            .any(|w| w.eq_ignore_ascii_case(n.as_bytes()))
    } else {
        h.to_lowercase().contains(&n.to_lowercase())
    }
}

fn extract_vehicle_id_str(v: &Value) -> Option<String> {
    for key in ["VehicleId", "vehicleId", "id"] {
        if let Some(val) = v.get(key) {
            if let Some(s) = val.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                return Some(s.to_string());
            }
            if let Some(n) = val.as_i64() {
                return Some(n.to_string());
            }
            if let Some(n) = val.as_u64() {
                return Some(n.to_string());
            }
        }
    }
    None
}

fn extract_raw_vehicle_num(inner: &Value) -> Option<&str> {
    inner
        .get("VehicleNumber")
        .or_else(|| inner.get("vehicleNumber"))
        .or_else(|| inner.get("NormalizedVehicleNumber"))
        .or_else(|| inner.get("normalizedVehicleNumber"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn extract_vehicle_norm_num(inner: &Value) -> String {
    extract_raw_vehicle_num(inner)
        .map(normalize_vehicle_num)
        .unwrap_or_default()
}

fn extract_canonical_norm_num(inner: &Value) -> Option<String> {
    extract_raw_vehicle_num(inner).and_then(crate::vehicle_number::canonical_normalized_key)
}

fn matches_vehicle_number_field(
    val: Option<&Value>,
    q_raw: &str,
    q_norm: &str,
    q_parsed: Option<&crate::vehicle_number::VehicleNumber>,
) -> bool {
    let Some(s) = val.and_then(Value::as_str) else {
        return false;
    };
    if contains_case_insensitive(s, q_raw) {
        return true;
    }
    if !q_norm.is_empty() && normalize_vehicle_num(s).contains(q_norm) {
        return true;
    }
    if let Some(q_vn) = q_parsed {
        if crate::vehicle_number::parse_vehicle_number(s).as_ref() == Some(q_vn) {
            return true;
        }
    }
    false
}

fn vehicle_matches_non_number_filters(inner: &Value, params: &VehicleSearchParameters) -> bool {
    if let Some(q_owner) = params
        .owner_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let owner_match = inner
            .get("Owners")
            .or_else(|| inner.get("owners"))
            .and_then(Value::as_array)
            .is_some_and(|owners| {
                owners.iter().any(|o| {
                    o.get("Name")
                        .or_else(|| o.get("name"))
                        .and_then(Value::as_str)
                        .is_some_and(|name| contains_case_insensitive(name, q_owner))
                })
            });
        if !owner_match {
            return false;
        }
    }

    if let Some(q_branch) = params
        .sangh_branch
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let top_branch = inner
            .get("SanghBranch")
            .or_else(|| inner.get("sanghBranch"))
            .and_then(Value::as_str)
            .is_some_and(|b| contains_case_insensitive(b, q_branch));
        let owner_branch = inner
            .get("Owners")
            .or_else(|| inner.get("owners"))
            .and_then(Value::as_array)
            .is_some_and(|owners| {
                owners.iter().any(|o| {
                    o.get("SanghBranch")
                        .or_else(|| o.get("sanghBranch"))
                        .and_then(Value::as_str)
                        .is_some_and(|b| contains_case_insensitive(b, q_branch))
                })
            });
        if !top_branch && !owner_branch {
            return false;
        }
    }

    if let Some(ref q_status) = params.status {
        let status_match = inner
            .pointer("/Registration/Status")
            .or_else(|| inner.get("status"))
            .and_then(Value::as_str)
            .is_some_and(|s| s.trim().eq_ignore_ascii_case(q_status.as_str()));
        if !status_match {
            return false;
        }
    }

    true
}

fn vehicle_matches_search_params(
    inner: &Value,
    params: &VehicleSearchParameters,
    query_norm_num: &str,
    query_parsed: Option<&crate::vehicle_number::VehicleNumber>,
) -> bool {
    if let Some(q_raw) = params
        .vehicle_number
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let num_match = matches_vehicle_number_field(
            inner.get("VehicleNumber"),
            q_raw,
            query_norm_num,
            query_parsed,
        ) || matches_vehicle_number_field(
            inner.get("NormalizedVehicleNumber"),
            q_raw,
            query_norm_num,
            query_parsed,
        );
        let id_match = extract_vehicle_id_str(inner).is_some_and(|id| id == q_raw);
        if !num_match && !id_match {
            return false;
        }
    }

    vehicle_matches_non_number_filters(inner, params)
}

fn search_cache_vehicles_with_id_lookup(
    cache: &crate::cache::Cache,
    params: &VehicleSearchParameters,
    tqms_items: &[Value],
) -> Vec<(Option<i32>, Value)> {
    let Ok(cached_vehicles) = cache.read_all(VehicleResource) else {
        return Vec::new();
    };

    let query_norm_num = params
        .vehicle_number
        .as_deref()
        .map(normalize_vehicle_num)
        .unwrap_or_default();

    let expansion = params
        .vehicle_number
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|q| crate::vehicle_history::expand_vehicle_search_from_cache(cache, q));
    let query_parsed = expansion.as_ref().and_then(|e| e.query_parsed.as_ref());

    let mut matched_resources: Vec<DatabaseResource> = Vec::new();
    let mut seen_cache_ids: HashSet<i32> = HashSet::new();
    let mut target_vehicle_ids: HashSet<String> = HashSet::new();

    if let Some(q_raw) = params
        .vehicle_number
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        target_vehicle_ids.insert(q_raw.to_string());
    }
    if let Some(ref exp) = expansion {
        target_vehicle_ids.extend(exp.matched_vehicle_ids.iter().cloned());
    }
    for item in tqms_items {
        if let Some(vid) = extract_vehicle_id_str(item) {
            target_vehicle_ids.insert(vid);
        }
    }

    // Pass 1: search cache directly with provided params, canonical VehicleNumber equivalence, and VehicleHistoryResource expansion
    for res in &cached_vehicles {
        let Some(inner) = &res.inner_content else {
            continue;
        };
        let direct_match =
            vehicle_matches_search_params(inner, params, &query_norm_num, query_parsed);
        let expanded_match = expansion
            .as_ref()
            .is_some_and(|exp| exp.matches_vehicle_inner(res.id, inner))
            && vehicle_matches_non_number_filters(inner, params);

        if direct_match || expanded_match {
            if let Some(id_str) = extract_vehicle_id_str(inner) {
                target_vehicle_ids.insert(id_str);
            }
            if let Some(cid) = res.id {
                if !seen_cache_ids.insert(cid) {
                    continue;
                }
            }
            matched_resources.push((*res).clone());
        }
    }

    // Pass 2: when searched by vehicle number, also search cache by matched VehicleId(s)
    if params
        .vehicle_number
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty())
        && !target_vehicle_ids.is_empty()
    {
        for res in &cached_vehicles {
            if res.id.is_some_and(|cid| seen_cache_ids.contains(&cid)) {
                continue;
            }
            let Some(inner) = &res.inner_content else {
                continue;
            };
            let Some(vid) = extract_vehicle_id_str(inner) else {
                continue;
            };
            if !target_vehicle_ids.contains(&vid)
                || !vehicle_matches_non_number_filters(inner, params)
            {
                continue;
            }
            if let Some(cid) = res.id {
                seen_cache_ids.insert(cid);
            }
            matched_resources.push((*res).clone());
        }
    }

    // Deduplicate by VehicleId / normalized VehicleNumber / resource id, keeping only the latest record
    let deduped = crate::cache::Cache::deduplicate_vehicle_resources(matched_resources);
    deduped
        .into_iter()
        .filter_map(|res| {
            let id = res.id;
            res.inner_content.map(|inner| (id, inner))
        })
        .collect()
}

fn insert_vehicle_flags(
    item: &mut Value,
    source: &str,
    is_in_billing: bool,
    is_in_tqms: bool,
    cache_id: Option<i32>,
) {
    if let Value::Object(map) = item {
        map.insert("source".to_string(), Value::String(source.to_string()));
        map.insert("is_in_billing".to_string(), Value::Bool(is_in_billing));
        map.insert("is_in_tqms".to_string(), Value::Bool(is_in_tqms));
        if let Some(cid) = cache_id {
            map.insert("cache_id".to_string(), json!(cid));
        }
    }
}

fn find_matching_vehicle_resource<'a, I>(
    resources: I,
    vid: Option<&str>,
    norm_num: &str,
    canonical_key: Option<&str>,
) -> Option<&'a DatabaseResource>
where
    I: IntoIterator<Item = &'a DatabaseResource>,
{
    resources.into_iter().find(|res| {
        let Some(inner) = res.inner_content.as_ref() else {
            return false;
        };
        let id_match = vid.is_some_and(|v| extract_vehicle_id_str(inner).as_deref() == Some(v));
        let num_match = !norm_num.is_empty() && extract_vehicle_norm_num(inner) == norm_num;
        let canon_match =
            canonical_key.is_some_and(|ck| extract_canonical_norm_num(inner).as_deref() == Some(ck));
        id_match || num_match || canon_match
    })
}

fn clean_tqms_vehicle_value(item: &Value) -> Value {
    let mut cleaned = item.clone();
    if let Some(map) = cleaned.as_object_mut() {
        for key in ["source", "is_in_billing", "is_in_tqms", "cache_id"] {
            map.remove(key);
        }
    }
    cleaned
}

fn is_vehicle_object(val: &Value) -> bool {
    val.is_object()
        && (extract_vehicle_id_str(val).is_some() || !extract_vehicle_norm_num(val).is_empty())
}

fn extract_tqms_vehicle_items(payload: &Value) -> Vec<Value> {
    if let Some(arr) = payload.pointer("/Data/Data").and_then(Value::as_array) {
        return arr.iter().filter(|v| is_vehicle_object(v)).cloned().collect();
    }
    if let Some(data) = payload.get("Data") {
        if let Some(arr) = data.as_array() {
            return arr.iter().filter(|v| is_vehicle_object(v)).cloned().collect();
        }
        if is_vehicle_object(data) {
            return vec![data.clone()];
        }
    }
    if is_vehicle_object(payload) {
        return vec![payload.clone()];
    }
    Vec::new()
}

/// Ensures every TQMS vehicle exists in either `s_data.cache` or `s_data.db`
/// (matched by `VehicleId`, normalized `VehicleNumber`, or canonical `VehicleNumber` equivalence).
/// Vehicles present in DB but missing from cache are synced into cache; vehicles absent from both
/// DB and cache are created as new `VehicleResource` records in DB and written into cache.
/// Existing `VehicleResource` data is never overwritten; if only `VehicleNumber` changed compared
/// to a matched record, that number change is recorded onto `VehicleHistoryResource`.
pub async fn ensure_tqms_vehicles_in_cache_and_db(
    s_data: &ServerData,
    tqms_items: &[Value],
) -> Vec<DatabaseResource> {
    if tqms_items.is_empty() {
        return Vec::new();
    }

    let mut number_only_changes: Vec<(Option<i32>, Value, Value)> = Vec::new();
    let uncached_items: Vec<Value> = {
        let cache = s_data.cache.read().await;
        let cached = cache.read_all(VehicleResource).unwrap_or_default();

        tqms_items
            .iter()
            .filter_map(|item| {
                let vid = extract_vehicle_id_str(item);
                let norm_num = extract_vehicle_norm_num(item);
                if vid.is_none() && norm_num.is_empty() {
                    return None;
                }
                let canonical_key = extract_canonical_norm_num(item);
                if let Some(existing) = find_matching_vehicle_resource(
                    cached.iter().copied(),
                    vid.as_deref(),
                    &norm_num,
                    canonical_key.as_deref(),
                ) {
                    let clean_item = clean_tqms_vehicle_value(item);
                    if let Some(existing_inner) = existing.inner_content.as_ref() {
                        if crate::vehicle_history::detect_vehicle_number_only_change(
                            Some(existing_inner),
                            Some(&clean_item),
                        )
                        .is_some()
                        {
                            number_only_changes.push((
                                existing.id,
                                existing_inner.clone(),
                                clean_item,
                            ));
                        }
                    }
                    None
                } else {
                    Some(clean_tqms_vehicle_value(item))
                }
            })
            .collect()
    };

    for (existing_id, before_inner, after_inner) in number_only_changes {
        if let Err(err) = crate::vehicle_history::record_vehicle_change(
            s_data,
            crate::vehicle_history::VehicleChangeType::VehicleNumberChange,
            existing_id,
            Some(&before_inner),
            Some(&after_inner),
            Some("tqms_sync".to_string()),
            None,
            None,
        )
        .await
        {
            eprintln!(
                "Failed to record VehicleNumberChange on TQMS match for VehicleResource {:?}: {}",
                existing_id, err
            );
        }
    }

    if uncached_items.is_empty() {
        return Vec::new();
    }

    let mut known_db_vehicles = s_data
        .db
        .read_all_resource(VehicleResource)
        .await
        .unwrap_or_default();

    let mut created_resources = Vec::new();

    for clean_item in uncached_items {
        let vid = extract_vehicle_id_str(&clean_item);
        let norm_num = extract_vehicle_norm_num(&clean_item);
        let canonical_key = extract_canonical_norm_num(&clean_item);

        if let Some(existing_db_res) = find_matching_vehicle_resource(
            known_db_vehicles.iter(),
            vid.as_deref(),
            &norm_num,
            canonical_key.as_deref(),
        )
        .cloned()
        {
            if let Some(db_id) = existing_db_res.id {
                s_data
                    .cache
                    .write()
                    .await
                    .write(VehicleResource, db_id, existing_db_res.clone());
            }
            if let Some(existing_inner) = existing_db_res.inner_content.as_ref() {
                if crate::vehicle_history::detect_vehicle_number_only_change(
                    Some(existing_inner),
                    Some(&clean_item),
                )
                .is_some()
                {
                    let _ = crate::vehicle_history::record_vehicle_change(
                        s_data,
                        crate::vehicle_history::VehicleChangeType::VehicleNumberChange,
                        existing_db_res.id,
                        Some(existing_inner),
                        Some(&clean_item),
                        Some("tqms_sync".to_string()),
                        None,
                        None,
                    )
                    .await;
                }
            }
            continue;
        }

        let now_str = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let mut new_res = DatabaseResource {
            id: None,
            resource_name: Some(VehicleResource),
            inner_content: Some(clean_item),
            external_content: Some(json!({ "createdTime": now_str })),
        };

        match s_data
            .db
            .create_resource(VehicleResource, new_res.clone())
            .await
        {
            Ok(created_id) => {
                new_res.id = Some(created_id);
                s_data
                    .cache
                    .write()
                    .await
                    .write(VehicleResource, created_id, new_res.clone());
                if let Err(err) = crate::vehicle_history::record_vehicle_change(
                    s_data,
                    crate::vehicle_history::VehicleChangeType::Sync,
                    Some(created_id),
                    None,
                    new_res.inner_content.as_ref(),
                    Some("tqms_sync".to_string()),
                    Some("Created from TQMS sync".to_string()),
                    None,
                )
                .await
                {
                    eprintln!(
                        "Failed to record VehicleHistoryResource on TQMS sync for VehicleResource {}: {}",
                        created_id, err
                    );
                }
                known_db_vehicles.push(new_res.clone());
                created_resources.push(new_res);
            }
            Err(err) => {
                eprintln!("Failed to create TQMS vehicle resource in DB: {err:?}");
            }
        }
    }

    created_resources
}

fn merge_and_annotate_vehicle_results(
    cache: &crate::cache::Cache,
    params: &VehicleSearchParameters,
    tqms_result: Result<Value>,
) -> Result<Value> {
    let mut tqms_payload = None;
    let mut tqms_items: Vec<Value> = Vec::new();
    let mut tqms_err = None;

    match tqms_result {
        Ok(mut val) => {
            if let Some(Value::Array(arr)) = val.pointer_mut("/Data/Data").map(Value::take) {
                tqms_items = arr;
            }
            tqms_payload = Some(val);
        }
        Err(err) => {
            tqms_err = Some(err);
        }
    }

    let raw_cache_hits = search_cache_vehicles_with_id_lookup(cache, params, &tqms_items);

    let mut tqms_ids: HashSet<String> = HashSet::with_capacity(tqms_items.len());
    let mut tqms_norms: HashSet<String> = HashSet::with_capacity(tqms_items.len() * 2);
    for item in &tqms_items {
        if let Some(vid) = extract_vehicle_id_str(item) {
            tqms_ids.insert(vid);
        }
        let norm = extract_vehicle_norm_num(item);
        if !norm.is_empty() {
            tqms_norms.insert(norm);
        }
        if let Some(canon_norm) = extract_canonical_norm_num(item) {
            tqms_norms.insert(canon_norm);
        }
    }

    let mut cache_ids: HashSet<String> = HashSet::with_capacity(raw_cache_hits.len());
    let mut cache_norms: HashSet<String> = HashSet::with_capacity(raw_cache_hits.len() * 2);
    for (_, item) in &raw_cache_hits {
        if let Some(vid) = extract_vehicle_id_str(item) {
            cache_ids.insert(vid);
        }
        let norm = extract_vehicle_norm_num(item);
        if !norm.is_empty() {
            cache_norms.insert(norm);
        }
        if let Some(canon_norm) = extract_canonical_norm_num(item) {
            cache_norms.insert(canon_norm);
        }
    }

    // Broaden billing <-> TQMS cross-matching across cached VehicleHistoryResource links
    if let Ok(histories) = cache.read_all(crate::resource::ResourceName::VehicleHistoryResource) {
        for hist in histories {
            let Some(inner) = hist.inner_content.as_ref() else {
                continue;
            };
            let (_, h_vids, h_norms) =
                crate::vehicle_history::extract_all_identifiers_from_history(inner);
            if h_vids.iter().any(|id| tqms_ids.contains(id))
                || h_norms.iter().any(|n| tqms_norms.contains(n))
            {
                tqms_ids.extend(h_vids.iter().cloned());
                tqms_norms.extend(h_norms.iter().cloned());
            }
            if h_vids.iter().any(|id| cache_ids.contains(id))
                || h_norms.iter().any(|n| cache_norms.contains(n))
            {
                cache_ids.extend(h_vids);
                cache_norms.extend(h_norms);
            }
        }
    }

    let mut cache_results: Vec<Value> = Vec::with_capacity(raw_cache_hits.len());
    for (cache_id, mut item) in raw_cache_hits {
        let in_tqms = extract_vehicle_id_str(&item).is_some_and(|vid| tqms_ids.contains(&vid))
            || {
                let norm = extract_vehicle_norm_num(&item);
                !norm.is_empty() && tqms_norms.contains(&norm)
            }
            || extract_canonical_norm_num(&item).is_some_and(|c| tqms_norms.contains(&c));
        insert_vehicle_flags(&mut item, "cache", true, in_tqms, cache_id);
        cache_results.push(item);
    }

    let mut tqms_results: Vec<Value> = Vec::with_capacity(tqms_items.len());
    for mut item in tqms_items {
        let in_billing = extract_vehicle_id_str(&item).is_some_and(|vid| cache_ids.contains(&vid))
            || {
                let norm = extract_vehicle_norm_num(&item);
                !norm.is_empty() && cache_norms.contains(&norm)
            }
            || extract_canonical_norm_num(&item).is_some_and(|c| cache_norms.contains(&c));
        insert_vehicle_flags(&mut item, "tqms", in_billing, true, None);
        tqms_results.push(item);
    }

    if let Some(err) = tqms_err {
        if cache_results.is_empty() {
            return Err(err);
        }
        return Ok(json!({
            "Code": 200,
            "Message": "Cache search results (TQMS unavailable)",
            "Data": {
                "cache_results": cache_results,
                "tqms_results": []
            }
        }));
    }

    let mut payload = tqms_payload.unwrap_or_else(|| json!({ "Code": 200, "Data": {} }));
    if let Some(data_obj) = payload.get_mut("Data").and_then(Value::as_object_mut) {
        data_obj.remove("Data");
        data_obj.insert("cache_results".to_string(), Value::Array(cache_results));
        data_obj.insert("tqms_results".to_string(), Value::Array(tqms_results));
    } else if let Some(root_obj) = payload.as_object_mut() {
        root_obj.insert(
            "Data".to_string(),
            json!({
                "cache_results": cache_results,
                "tqms_results": tqms_results
            }),
        );
    }

    Ok(payload)
}

pub async fn search_vehicles(
    vehicle_search_parameters: web::Json<VehicleSearchParameters>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let params = vehicle_search_parameters.into_inner();
    if params.is_empty() {
        return HttpResponse::BadRequest().body("At least one search parameter must be provided");
    }

    let tqms_result = s_data.tqms_client.search_vehicles(params.clone()).await;
    if let Ok(ref val) = tqms_result {
        let tqms_items = extract_tqms_vehicle_items(val);
        ensure_tqms_vehicles_in_cache_and_db(&s_data, &tqms_items).await;
    }

    if !s_data
        .cache
        .read()
        .await
        .is_fully_loaded(crate::resource::ResourceName::VehicleHistoryResource)
    {
        if let Ok(hist_resources) = s_data
            .db
            .read_all_resource(crate::resource::ResourceName::VehicleHistoryResource)
            .await
        {
            let mut cache_w = s_data.cache.write().await;
            for hist in hist_resources {
                if let Some(hid) = hist.id {
                    cache_w.write(
                        crate::resource::ResourceName::VehicleHistoryResource,
                        hid,
                        hist,
                    );
                }
            }
            cache_w.mark_fully_loaded(crate::resource::ResourceName::VehicleHistoryResource);
        }
    }

    let cache = s_data.cache.read().await;

    match merge_and_annotate_vehicle_results(&cache, &params, tqms_result) {
        Ok(result) => HttpResponse::Ok().json(result),
        Err(e) => HttpResponse::InternalServerError()
            .body(format!("Failed to search vehicles: {e:?}")),
    }
}

pub async fn get_all_vehicles_tqms() -> Result<Vec<Value>> {
    const PAGE_SIZE: u32 = 50;
    const CONCURRENCY_LIMIT: usize = 100;
    const MAX_ATTEMPTS: u32 = 5;

    let tqms_client = TQMSClient::new().await;
    tqms_client
        .ensure_token()
        .await
        .context("TQMS token is missing after authentication")?;

    // Fetch page 1 first to obtain TotalItems and TotalPages.
    let mut first_page = fetch_tqms_vehicle_page(&tqms_client, 1, PAGE_SIZE, MAX_ATTEMPTS)
        .await
        .context("Failed to fetch the first TQMS vehicle page")?;

    validate_tqms_response(&first_page, 1)?;

    let paging = first_page
        .pointer("/Data/Paging")
        .context("Missing Data.Paging in the TQMS response")?;

    let total_items = paging
        .get("TotalItems")
        .and_then(Value::as_u64)
        .context("Missing or invalid Data.Paging.TotalItems")?;

    let total_pages = paging
        .get("TotalPages")
        .and_then(Value::as_u64)
        .context("Missing or invalid Data.Paging.TotalPages")?;

    let returned_page_size = paging
        .get("PageSize")
        .and_then(Value::as_u64)
        .context("Missing or invalid Data.Paging.PageSize")?;

    if returned_page_size != u64::from(PAGE_SIZE) {
        eprintln!(
            "Warning: requested page size {PAGE_SIZE}, but TQMS returned page size {returned_page_size}"
        );
    }

    println!("TQMS total items: {total_items}, total pages: {total_pages}, page size: {returned_page_size}");

    if total_items == 0 || total_pages == 0 {
        return Ok(Vec::new());
    }

    let calculated_pages = total_items.div_ceil(u64::from(PAGE_SIZE));
    if calculated_pages != total_pages {
        eprintln!(
            "Warning: TQMS reports {total_pages} pages, but {total_items} items with page size {PAGE_SIZE} calculates to {calculated_pages} pages"
        );
    }

    let mut completed_pages: Vec<(u64, Vec<Value>)> = Vec::with_capacity(total_pages as usize);
    let mut failed_pages: Vec<(u64, String)> = Vec::new();

    match take_vehicles(&mut first_page) {
        Ok(vehicles) => completed_pages.push((1, vehicles)),
        Err(error) => failed_pages.push((1, error.to_string())),
    }

    if total_pages > 1 {
        let mut page_stream = stream::iter(2..=total_pages)
            .map(|page| {
                let client = tqms_client.clone();
                async move {
                    let result =
                        fetch_tqms_vehicle_page(&client, page as u32, PAGE_SIZE, MAX_ATTEMPTS)
                            .await
                            .map_err(|e| e.to_string());
                    (page, result)
                }
            })
            .buffer_unordered(CONCURRENCY_LIMIT);

        while let Some((requested_page, result)) = page_stream.next().await {
            match result {
                Ok(mut response) => {
                    if let Err(error) = validate_tqms_response(&response, requested_page) {
                        failed_pages.push((requested_page, error.to_string()));
                        continue;
                    }

                    let response_page = response
                        .pointer("/Data/Paging/Page")
                        .and_then(Value::as_u64);

                    if response_page != Some(requested_page) {
                        failed_pages.push((
                            requested_page,
                            format!(
                                "Requested page {requested_page}, but TQMS returned page {response_page:?}"
                            ),
                        ));
                        continue;
                    }

                    match take_vehicles(&mut response) {
                        Ok(vehicles) => {
                            let item_count = vehicles.len();

                            if requested_page < total_pages && item_count != PAGE_SIZE as usize {
                                eprintln!(
                                    "Warning: TQMS page {requested_page} returned {item_count} vehicles; expected {PAGE_SIZE}"
                                );
                            } else if requested_page == total_pages {
                                let remainder = total_items % u64::from(PAGE_SIZE);
                                let expected_last = if remainder == 0 {
                                    PAGE_SIZE as usize
                                } else {
                                    remainder as usize
                                };
                                if item_count != expected_last {
                                    eprintln!(
                                        "Warning: final page {requested_page} returned {item_count} vehicles; expected {expected_last} based on TotalItems"
                                    );
                                }
                            }

                            completed_pages.push((requested_page, vehicles));
                        }
                        Err(error) => failed_pages.push((requested_page, error.to_string())),
                    }
                }
                Err(error) => failed_pages.push((requested_page, error)),
            }
        }
    }

    completed_pages.sort_unstable_by_key(|(page, _)| *page);
    failed_pages.sort_unstable_by_key(|(page, _)| *page);

    println!(
        "Successfully fetched TQMS pages: {}/{}",
        completed_pages.len(),
        total_pages
    );

    if !failed_pages.is_empty() {
        for (page, error) in &failed_pages {
            eprintln!("TQMS page {page} permanently failed: {error}");
        }
        bail!(
            "{} of {} TQMS pages failed after {} attempts",
            failed_pages.len(),
            total_pages,
            MAX_ATTEMPTS
        );
    }

    let mut seen_vehicle_ids = HashSet::with_capacity(total_items as usize);
    let mut vehicles = Vec::with_capacity(total_items as usize);
    let mut duplicate_count = 0usize;
    let mut missing_id_count = 0usize;

    for (_, page_vehicles) in completed_pages {
        for vehicle in page_vehicles {
            match vehicle.get("VehicleId").and_then(Value::as_u64) {
                Some(vehicle_id) => {
                    if seen_vehicle_ids.insert(vehicle_id) {
                        vehicles.push(vehicle);
                    } else {
                        duplicate_count += 1;
                        eprintln!("Warning: duplicate VehicleId {vehicle_id} returned by TQMS");
                    }
                }
                None => {
                    missing_id_count += 1;
                    vehicles.push(vehicle);
                }
            }
        }
    }

    if duplicate_count > 0 {
        eprintln!("Warning: removed {duplicate_count} duplicate TQMS vehicle records");
    }
    if missing_id_count > 0 {
        eprintln!("Warning: {missing_id_count} TQMS vehicle records did not contain VehicleId");
    }

    println!("Total unique vehicles fetched from TQMS: {}", vehicles.len());

    if vehicles.len() as u64 != total_items {
        bail!(
            "Incomplete TQMS result: Data.Paging.TotalItems is {}, \
             but {} unique vehicles were fetched. \
             Raw records: {}, duplicates: {}",
            total_items,
            vehicles.len(),
            vehicles.len() + duplicate_count,
            duplicate_count
        );
    }

    Ok(vehicles)
}

async fn fetch_tqms_vehicle_page(
    client: &TQMSClient,
    page: u32,
    page_size: u32,
    max_attempts: u32,
) -> Result<Value> {
    fetch_tqms_vehicle_page_with_backoff(client, page, page_size, max_attempts, 500).await
}

async fn fetch_tqms_vehicle_page_with_backoff(
    client: &TQMSClient,
    page: u32,
    page_size: u32,
    max_attempts: u32,
    retry_base_delay_ms: u64,
) -> Result<Value> {
    let mut last_error = None;
    let attempts = max_attempts.max(1);

    for attempt in 1..=attempts {
        let result = client
            .search_vehicles(VehicleSearchParameters {
                page: Some(page),
                page_size: Some(page_size),
                vehicle_number: None,
                owner_name: None,
                sangh_branch: None,
                status: None,
            })
            .await;

        match result {
            Ok(response) => {
                let response_code = response.get("Code").and_then(Value::as_u64);
                if response_code == Some(200) {
                    return Ok(response);
                }

                let message = response
                    .get("Message")
                    .and_then(Value::as_str)
                    .unwrap_or("Unknown TQMS error");

                last_error = Some(format!(
                    "TQMS page {page} returned Code {response_code:?}: {message}"
                ));
            }
            Err(error) => {
                last_error = Some(format!("TQMS page {page} request failed: {error}"));
            }
        }

        if attempt < attempts && retry_base_delay_ms > 0 {
            let delay_ms = retry_base_delay_ms * 2_u64.pow(attempt - 1);
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }
    }

    bail!(
        "Failed to fetch TQMS page {} after {} attempts: {}",
        page,
        attempts,
        last_error.unwrap_or_else(|| "unknown error".to_string())
    )
}

fn validate_tqms_response(response: &Value, requested_page: u64) -> Result<()> {
    let response_code = response
        .get("Code")
        .and_then(Value::as_u64)
        .context("Missing or invalid TQMS response Code")?;

    if response_code != 200 {
        let message = response
            .get("Message")
            .and_then(Value::as_str)
            .unwrap_or("Unknown TQMS error");

        bail!("TQMS page {requested_page} returned Code {response_code}: {message}");
    }

    if response.pointer("/Data/Data").is_none() {
        bail!("TQMS page {requested_page} is missing Data.Data");
    }

    if response.pointer("/Data/Paging").is_none() {
        bail!("TQMS page {requested_page} is missing Data.Paging");
    }

    Ok(())
}

/// Takes ownership of the `Data.Data` array in-place instead of deep-cloning it.
fn take_vehicles(response: &mut Value) -> Result<Vec<Value>> {
    match response.pointer_mut("/Data/Data").map(Value::take) {
        Some(Value::Array(arr)) => Ok(arr),
        _ => bail!("Missing or invalid Data.Data vehicle array"),
    }
}

/// Shared helper to fetch from TQMS, diff against existing DB records in O(N + M) time,
/// and return only new `DatabaseResource` items ready for insertion.
async fn prepare_new_vehicles_for_sync(
    db: &crate::db::Database,
) -> Result<Vec<DatabaseResource>, HttpResponse> {
    let vehicles = get_all_vehicles_tqms().await.map_err(|e| {
        HttpResponse::InternalServerError()
            .body(format!("Failed to get vehicles from TQMS: {e:?}"))
    })?;

    println!("Fetched {} vehicles from TQMS", vehicles.len());

    let previous_vehicles = db.read_all_resource(VehicleResource).await.map_err(|e| {
        HttpResponse::InternalServerError()
            .body(format!("Failed to read vehicles from database: {e:?}"))
    })?;

    println!("Fetched {} vehicles from database", previous_vehicles.len());

    // Index existing DB vehicles by VehicleId for O(1) lookup instead of O(N * M) full-JSON scans.
    let mut existing_by_id: HashMap<u64, Vec<&Value>> =
        HashMap::with_capacity(previous_vehicles.len());
    let mut existing_without_id: Vec<&Value> = Vec::new();

    for db_v in &previous_vehicles {
        if let Some(inner) = &db_v.inner_content {
            if let Some(id) = inner.get("VehicleId").and_then(Value::as_u64) {
                existing_by_id.entry(id).or_default().push(inner);
            } else {
                existing_without_id.push(inner);
            }
        }
    }

    let db_vehicles: Vec<DatabaseResource> = vehicles
        .into_iter()
        .filter(|vehicle| {
            if let Some(id) = vehicle.get("VehicleId").and_then(Value::as_u64) {
                !existing_by_id
                    .get(&id)
                    .is_some_and(|list| list.iter().any(|&db_inner| db_inner == vehicle))
            } else {
                !existing_without_id.iter().any(|&db_inner| db_inner == vehicle)
            }
        })
        .map(|vehicle| DatabaseResource {
            id: None,
            inner_content: Some(vehicle),
            external_content: None,
            resource_name: Some(VehicleResource),
        })
        .collect();

    println!("Syncing {} new vehicles to database", db_vehicles.len());
    Ok(db_vehicles)
}

/// Inserts only newly created vehicle resources directly into the in-memory cache
/// instead of reloading the entire database from scratch.
async fn update_vehicle_cache(
    s_data: &ServerData,
    ids: Vec<i32>,
    resources: Vec<DatabaseResource>,
) {
    let mut cache = s_data.cache.write().await;
    for (id, mut resource) in ids.into_iter().zip(resources) {
        resource.id = Some(id);
        cache.write(VehicleResource, id, resource);
    }
}

pub async fn get_vehicle_from_tqms() -> HttpResponse {
    match get_all_vehicles_tqms().await {
        Ok(_) => HttpResponse::Ok()
            .json(json!({"success": "true", "message": "Vehicles fetched successfully"})),
        Err(e) => HttpResponse::InternalServerError()
            .body(format!("Failed to get vehicle details from TQMS: {e:?}")),
    }
}

pub async fn sync_vehicles_tqms(s_data: web::Data<Arc<ServerData>>) -> HttpResponse {
    let db = &s_data.db;
    let db_vehicles = match prepare_new_vehicles_for_sync(db).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    if db_vehicles.is_empty() {
        return HttpResponse::Ok()
            .json(json!({"success": "true", "message": "No new vehicles to sync"}));
    }

    // Clone only the new delta so we can populate the cache directly with the returned IDs
    let cache_vehicles = db_vehicles.clone();

    match db.batch_create_resources(VehicleResource, db_vehicles).await {
        Ok(ids) => {
            update_vehicle_cache(&s_data, ids, cache_vehicles).await;
            HttpResponse::Ok()
                .json(json!({"success": "true", "message": "Vehicles synced successfully"}))
        }
        Err(e) => HttpResponse::InternalServerError()
            .body(format!("Failed to sync vehicles: {e:?}")),
    }
}

#[derive(Serialize)]
struct VehicleTypeCount {
    vehicle_type: String,
    vehicle_count: u64,
}

pub async fn sync_vehicle_tqms_concurrency_limited(
    s_data: web::Data<Arc<ServerData>>,
    concurrency: web::Path<usize>,
) -> HttpResponse {
    let db = &s_data.db;
    let concurrency = concurrency.into_inner().max(1);

    let db_vehicles = match prepare_new_vehicles_for_sync(db).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    if db_vehicles.is_empty() {
        return HttpResponse::Ok()
            .json(json!({"success": "true", "message": "No new vehicles to sync"}));
    }

    let successful = Arc::new(AtomicUsize::new(0));
    let failed = Arc::new(AtomicUsize::new(0));

    // Process in chunks of `concurrency` using `batch_create_resources`, falling back
    // to individual creation only if a chunk fails, and updating the cache along the way.
    let synced_entries: Vec<(i32, DatabaseResource)> = stream::iter(db_vehicles)
        .map(|vehicle| {
            let db = s_data.db.clone();
            let successful = successful.clone();
            let failed = failed.clone();

            async move {
                match db.create_resource(VehicleResource, vehicle.clone()).await {
                    Ok(id) => {
                        successful.fetch_add(1, Ordering::Relaxed);
                        Some((id, vehicle))
                    }
                    Err(err) => {
                        eprintln!("Failed to sync vehicle: {err:?}");
                        failed.fetch_add(1, Ordering::Relaxed);
                        None
                    }
                }
            }
        })
        .buffer_unordered(concurrency)
        .filter_map(|x| async move { x })
        .collect()
        .await;

    if !synced_entries.is_empty() {
        let mut cache = s_data.cache.write().await;
        for (id, mut resource) in synced_entries {
            resource.id = Some(id);
            cache.write(VehicleResource, id, resource);
        }
    }

    HttpResponse::Ok().json(json!({
        "success": true,
        "synced": successful.load(Ordering::Relaxed),
        "failed": failed.load(Ordering::Relaxed),
        "concurrency": concurrency
    }))
}

pub async fn read_all_vehicle_types(s_data: web::Data<Arc<ServerData>>) -> HttpResponse {
    let cache = s_data.cache.read().await;

    // Count using borrowed `&str` keys first so we only allocate Strings for unique vehicle types
    let mut counts: HashMap<&str, u64> = HashMap::new();

    if let Ok(vehicle_resources) = cache.read_all(VehicleResource) {
        for resource in vehicle_resources {
            if let Some(vehicle_type) = resource
                .inner_content
                .as_ref()
                .and_then(|v| v.get("VehicleType"))
                .and_then(Value::as_str)
            {
                *counts.entry(vehicle_type).or_default() += 1;
            }
        }
    }

    let result: Vec<VehicleTypeCount> = counts
        .into_iter()
        .map(|(vehicle_type, vehicle_count)| VehicleTypeCount {
            vehicle_type: vehicle_type.to_owned(),
            vehicle_count,
        })
        .collect();

    HttpResponse::Ok().json(result)
}

pub async fn validate_vehicle(
    vehicle_number: web::Path<String>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    match s_data.tqms_client.validate_vehicles(&vehicle_number).await {
        Ok(res) => HttpResponse::Ok().json(res),
        Err(e) => HttpResponse::InternalServerError()
            .body(format!("Failed to validate vehicle: {e:?}")),
    }
}

pub async fn get_vehicle_details(
    vehicle_number: web::Path<String>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    match s_data.tqms_client.get_vehicle_details(&vehicle_number).await {
        Ok(res) => {
            let tqms_items = extract_tqms_vehicle_items(&res);
            ensure_tqms_vehicles_in_cache_and_db(&s_data, &tqms_items).await;
            HttpResponse::Ok().json(res)
        }
        Err(e) => HttpResponse::InternalServerError()
            .body(format!("Failed to get vehicle details: {e:?}")),
    }
}

pub async fn get_vehicle_documents(
    vehicle_number: web::Path<String>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    match s_data.tqms_client.get_vehicle_documents(&vehicle_number).await {
        Ok(res) => HttpResponse::Ok().json(res),
        Err(e) => HttpResponse::InternalServerError()
            .body(format!("Failed to get vehicle documents: {e:?}")),
    }
}

const MAX_STREAM_SYNC_BATCH_SIZE: usize = 500;
const MAX_STREAM_SYNC_CONCURRENCY: usize = 64;
const STREAM_SYNC_MAX_ATTEMPTS: u32 = 5;
const STREAM_SYNC_RETRY_BASE_MS: u64 = 500;
const STREAM_SYNC_SSE_HEARTBEAT_SECS: u64 = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamSyncFormat {
    Sse,
    Ndjson,
}

impl StreamSyncFormat {
    fn from_request(req: &HttpRequest) -> Self {
        for pair in req.query_string().split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                if k.eq_ignore_ascii_case("format") {
                    let val = v.trim();
                    if val.eq_ignore_ascii_case("ndjson")
                        || val.eq_ignore_ascii_case("x-ndjson")
                        || val.eq_ignore_ascii_case("application/x-ndjson")
                        || val.eq_ignore_ascii_case("application%2Fx-ndjson")
                        || val.eq_ignore_ascii_case("application/ndjson")
                        || val.eq_ignore_ascii_case("application%2Fndjson")
                    {
                        return Self::Ndjson;
                    }
                    if val.eq_ignore_ascii_case("sse")
                        || val.eq_ignore_ascii_case("event-stream")
                        || val.eq_ignore_ascii_case("text/event-stream")
                        || val.eq_ignore_ascii_case("text%2Fevent-stream")
                    {
                        return Self::Sse;
                    }
                }
            }
        }

        if let Some(accept) = req.headers().get(header::ACCEPT).and_then(|v| v.to_str().ok()) {
            let accept_lower = accept.to_ascii_lowercase();
            if (accept_lower.contains("application/x-ndjson")
                || accept_lower.contains("application/ndjson"))
                && !accept_lower.contains("text/event-stream")
            {
                return Self::Ndjson;
            }
        }

        Self::Sse
    }

    const fn content_type(self) -> &'static str {
        match self {
            Self::Sse => "text/event-stream; charset=utf-8",
            Self::Ndjson => "application/x-ndjson; charset=utf-8",
        }
    }

    const fn header_label(self) -> &'static str {
        match self {
            Self::Sse => "sse",
            Self::Ndjson => "ndjson",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamedVehicleItem {
    pub id: Option<i32>,
    pub sync_status: String,
    pub vehicle: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn ndjson_chunk(value: &Value) -> Result<Bytes, String> {
    serde_json::to_vec(value)
        .map(|mut json_bytes| {
            json_bytes.push(b'\n');
            Bytes::from(json_bytes)
        })
        .map_err(|e| e.to_string())
}

fn sse_chunk(event_name: &str, event_id: u64, value: &Value) -> Result<Bytes, String> {
    let json = serde_json::to_string(value).map_err(|e| e.to_string())?;
    Ok(Bytes::from(format!(
        "id: {event_id}\nevent: {event_name}\ndata: {json}\n\n"
    )))
}

fn stream_event_chunk(
    format: StreamSyncFormat,
    event_name: &str,
    event_id: u64,
    value: &Value,
) -> Result<Bytes, String> {
    match format {
        StreamSyncFormat::Sse => sse_chunk(event_name, event_id, value),
        StreamSyncFormat::Ndjson => ndjson_chunk(value),
    }
}

struct ExistingVehicleIndex {
    by_vehicle_id: HashMap<String, DatabaseResource>,
    by_norm_number: HashMap<String, DatabaseResource>,
}

impl ExistingVehicleIndex {
    fn from_resources(resources: Vec<DatabaseResource>) -> Self {
        let mut index = Self {
            by_vehicle_id: HashMap::with_capacity(resources.len()),
            by_norm_number: HashMap::with_capacity(resources.len()),
        };
        for res in resources {
            index.upsert(res);
        }
        index
    }

    fn find(&self, vid: Option<&str>, norm_num: &str) -> Option<&DatabaseResource> {
        if let Some(id) = vid {
            if let Some(found) = self.by_vehicle_id.get(id) {
                return Some(found);
            }
        }
        if !norm_num.is_empty() {
            if let Some(found) = self.by_norm_number.get(norm_num) {
                return Some(found);
            }
        }
        None
    }

    fn upsert(&mut self, res: DatabaseResource) {
        if let Some(ref inner) = res.inner_content {
            if let Some(vid) = extract_vehicle_id_str(inner) {
                self.by_vehicle_id.insert(vid, res.clone());
            }
            let norm = extract_vehicle_norm_num(inner);
            if !norm.is_empty() {
                self.by_norm_number.insert(norm, res);
            }
        }
    }
}

fn merge_local_metadata(fresh: &mut Value, existing_inner: &Value) {
    if let Some(existing_meta) = existing_inner.get("metadata").filter(|m| !m.is_null()) {
        if let Some(map) = fresh.as_object_mut() {
            let has_fresh_meta = map.get("metadata").is_some_and(|m| !m.is_null());
            if !has_fresh_meta {
                map.insert("metadata".to_string(), existing_meta.clone());
            }
        }
    }
}

enum PendingVehicleDbOp {
    Create {
        idx: usize,
        resource: DatabaseResource,
    },
    Update {
        idx: usize,
        id: i32,
        resource: DatabaseResource,
    },
}

struct BatchSyncOutcome {
    created: usize,
    updated: usize,
    skipped: usize,
    failed: usize,
    duplicates_filtered: usize,
    streamed_vehicles: Vec<StreamedVehicleItem>,
}

async fn sync_stream_batch(
    s_data: &Arc<ServerData>,
    batch: Vec<Value>,
    existing_index: &mut ExistingVehicleIndex,
    seen_ids: &mut HashSet<String>,
    seen_nums: &mut HashSet<String>,
    concurrency: usize,
) -> BatchSyncOutcome {
    let mut created = 0usize;
    let mut updated = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    let mut duplicates_filtered = 0usize;

    let mut ops: Vec<PendingVehicleDbOp> = Vec::with_capacity(batch.len());
    let mut ordered_slots: Vec<Option<StreamedVehicleItem>> = Vec::with_capacity(batch.len());
    let mut unchanged_for_cache: Vec<(i32, DatabaseResource)> = Vec::new();
    let now_str = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    for raw_vehicle in batch {
        let mut clean_vehicle = clean_tqms_vehicle_value(&raw_vehicle);
        let vid = extract_vehicle_id_str(&clean_vehicle);
        let norm_num = extract_vehicle_norm_num(&clean_vehicle);

        let dup_by_id = vid.as_ref().is_some_and(|id| seen_ids.contains(id));
        let dup_by_num = !norm_num.is_empty() && seen_nums.contains(&norm_num);
        if dup_by_id || dup_by_num {
            duplicates_filtered += 1;
            continue;
        }
        if let Some(ref id) = vid {
            seen_ids.insert(id.clone());
        }
        if !norm_num.is_empty() {
            seen_nums.insert(norm_num.clone());
        }

        let slot_idx = ordered_slots.len();
        ordered_slots.push(None);

        if let Some(existing_res) = existing_index.find(vid.as_deref(), &norm_num).cloned() {
            if let (Some(existing_id), Some(existing_inner)) =
                (existing_res.id, existing_res.inner_content.as_ref())
            {
                merge_local_metadata(&mut clean_vehicle, existing_inner);
                if &clean_vehicle == existing_inner {
                    skipped += 1;
                    unchanged_for_cache.push((existing_id, existing_res.clone()));
                    ordered_slots[slot_idx] = Some(StreamedVehicleItem {
                        id: Some(existing_id),
                        sync_status: "skipped".to_string(),
                        vehicle: clean_vehicle,
                        error: None,
                    });
                } else {
                    ops.push(PendingVehicleDbOp::Update {
                        idx: slot_idx,
                        id: existing_id,
                        resource: DatabaseResource {
                            id: Some(existing_id),
                            resource_name: Some(VehicleResource),
                            inner_content: Some(clean_vehicle),
                            external_content: existing_res.external_content.clone(),
                        },
                    });
                }
                continue;
            }
        }

        ops.push(PendingVehicleDbOp::Create {
            idx: slot_idx,
            resource: DatabaseResource {
                id: None,
                resource_name: Some(VehicleResource),
                inner_content: Some(clean_vehicle),
                external_content: Some(json!({ "createdTime": now_str })),
            },
        });
    }

    let op_results: Vec<(
        usize,
        Result<(bool, DatabaseResource, StreamedVehicleItem), StreamedVehicleItem>,
    )> = stream::iter(ops)
        .map(|op| {
            let db = s_data.db.clone();
            async move {
                match op {
                    PendingVehicleDbOp::Create { idx, mut resource } => {
                        match db.create_resource(VehicleResource, resource.clone()).await {
                            Ok(new_id) => {
                                resource.id = Some(new_id);
                                let vehicle_val =
                                    resource.inner_content.clone().unwrap_or(Value::Null);
                                let item = StreamedVehicleItem {
                                    id: Some(new_id),
                                    sync_status: "created".to_string(),
                                    vehicle: vehicle_val,
                                    error: None,
                                };
                                (idx, Ok((true, resource, item)))
                            }
                            Err(err) => {
                                let vehicle_val =
                                    resource.inner_content.unwrap_or(Value::Null);
                                let item = StreamedVehicleItem {
                                    id: None,
                                    sync_status: "failed".to_string(),
                                    vehicle: vehicle_val,
                                    error: Some(format!("{err:?}")),
                                };
                                (idx, Err(item))
                            }
                        }
                    }
                    PendingVehicleDbOp::Update { idx, id, resource } => {
                        match db.update_resource(VehicleResource, id, resource.clone()).await {
                            Ok(()) => {
                                let vehicle_val =
                                    resource.inner_content.clone().unwrap_or(Value::Null);
                                let item = StreamedVehicleItem {
                                    id: Some(id),
                                    sync_status: "updated".to_string(),
                                    vehicle: vehicle_val,
                                    error: None,
                                };
                                (idx, Ok((false, resource, item)))
                            }
                            Err(err) => {
                                let vehicle_val =
                                    resource.inner_content.unwrap_or(Value::Null);
                                let item = StreamedVehicleItem {
                                    id: Some(id),
                                    sync_status: "failed".to_string(),
                                    vehicle: vehicle_val,
                                    error: Some(format!("{err:?}")),
                                };
                                (idx, Err(item))
                            }
                        }
                    }
                }
            }
        })
        .buffer_unordered(concurrency.max(1))
        .collect()
        .await;

    let mut cache_updates: Vec<(i32, DatabaseResource)> = Vec::new();
    let mut index_upserts: Vec<DatabaseResource> = Vec::new();

    for (idx, res) in op_results {
        match res {
            Ok((is_create, stored_res, item)) => {
                if is_create {
                    created += 1;
                } else {
                    updated += 1;
                }
                if let Some(db_id) = stored_res.id {
                    cache_updates.push((db_id, stored_res.clone()));
                }
                index_upserts.push(stored_res);
                ordered_slots[idx] = Some(item);
            }
            Err(failed_item) => {
                failed += 1;
                ordered_slots[idx] = Some(failed_item);
            }
        }
    }

    if !cache_updates.is_empty() || !unchanged_for_cache.is_empty() {
        let mut cache = s_data.cache.write().await;
        for (db_id, res) in cache_updates {
            cache.write(VehicleResource, db_id, res);
        }
        for (db_id, res) in unchanged_for_cache {
            if cache.read(VehicleResource, db_id).is_none() {
                cache.write(VehicleResource, db_id, res);
            }
        }
    }

    for res in index_upserts {
        existing_index.upsert(res);
    }

    let streamed_vehicles: Vec<StreamedVehicleItem> =
        ordered_slots.into_iter().flatten().collect();

    BatchSyncOutcome {
        created,
        updated,
        skipped,
        failed,
        duplicates_filtered,
        streamed_vehicles,
    }
}

/// Streams TQMS vehicles page-by-page as SSE (`text/event-stream`, default) or NDJSON
/// (`Accept: application/x-ndjson` / `?format=ndjson`) while concurrently fetching TQMS pages
/// and persisting deltas to PostgreSQL and the in-memory cache.
/// `GET /api/v1/tqms/stream_sync/{batch_size}/{concurrency}`
pub async fn stream_sync_vehicles_tqms(
    req: HttpRequest,
    path: web::Path<(usize, usize)>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let (batch_size, concurrency) = path.into_inner();
    let format = StreamSyncFormat::from_request(&req);
    stream_sync_vehicles_tqms_with_retry(
        batch_size,
        concurrency,
        format,
        s_data.get_ref().clone(),
        STREAM_SYNC_MAX_ATTEMPTS,
        STREAM_SYNC_RETRY_BASE_MS,
    )
    .await
}

async fn stream_sync_vehicles_tqms_with_retry(
    batch_size: usize,
    concurrency: usize,
    format: StreamSyncFormat,
    s_data: Arc<ServerData>,
    max_attempts: u32,
    retry_base_delay_ms: u64,
) -> HttpResponse {
    if batch_size == 0 || concurrency == 0 {
        return HttpResponse::BadRequest().json(json!({
            "success": false,
            "message": "batch_size and concurrency must both be greater than zero"
        }));
    }

    let effective_batch_size = batch_size.min(MAX_STREAM_SYNC_BATCH_SIZE);
    let effective_concurrency = concurrency.min(MAX_STREAM_SYNC_CONCURRENCY);
    let page_size = effective_batch_size as u32;

    if let Err(err) = s_data.tqms_client.ensure_token().await {
        return HttpResponse::BadGateway().json(json!({
            "success": false,
            "message": format!("TQMS authentication failed: {err}")
        }));
    }

    let mut first_page = match fetch_tqms_vehicle_page_with_backoff(
        &s_data.tqms_client,
        1,
        page_size,
        max_attempts,
        retry_base_delay_ms,
    )
    .await
    {
        Ok(page) => page,
        Err(err) => {
            return HttpResponse::BadGateway().json(json!({
                "success": false,
                "message": format!("Failed to fetch initial TQMS vehicle page: {err}")
            }));
        }
    };

    if let Err(err) = validate_tqms_response(&first_page, 1) {
        return HttpResponse::BadGateway().json(json!({
            "success": false,
            "message": format!("Invalid initial TQMS response: {err}")
        }));
    }

    let total_items = first_page
        .pointer("/Data/Paging/TotalItems")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total_pages = first_page
        .pointer("/Data/Paging/TotalPages")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    let first_page_vehicles = if total_items > 0 && total_pages > 0 {
        match take_vehicles(&mut first_page) {
            Ok(v) => v,
            Err(err) => {
                return HttpResponse::BadGateway().json(json!({
                    "success": false,
                    "message": format!("Failed to parse initial TQMS vehicles: {err}")
                }));
            }
        }
    } else {
        Vec::new()
    };

    let existing_db_vehicles = match s_data.db.read_all_resource(VehicleResource).await {
        Ok(v) => v,
        Err(err) => {
            return HttpResponse::InternalServerError().json(json!({
                "success": false,
                "message": format!("Failed to read existing vehicles from database: {err:?}")
            }));
        }
    };
    let mut existing_index = ExistingVehicleIndex::from_resources(existing_db_vehicles);

    let channel_cap = (effective_concurrency * 2).clamp(4, 64);
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, String>>(channel_cap);

    tokio::spawn(async move {
        let started_at = Instant::now();
        let mut batch_number = 0usize;
        let mut pages_completed = 0u64;
        let mut failed_pages: Vec<u64> = Vec::new();
        let mut total_fetched = 0usize;
        let mut total_created = 0usize;
        let mut total_updated = 0usize;
        let mut total_skipped = 0usize;
        let mut total_failed = 0usize;
        let mut total_duplicates = 0usize;
        let mut seen_ids: HashSet<String> = HashSet::with_capacity(total_items as usize);
        let mut seen_nums: HashSet<String> = HashSet::with_capacity(total_items as usize);
        let mut pending: Vec<Value> = Vec::with_capacity(effective_batch_size * 2);
        let mut event_seq: u64 = 0;

        let started_event = json!({
            "event": "started",
            "totalItems": total_items,
            "totalPages": total_pages,
            "batchSize": effective_batch_size,
            "concurrency": effective_concurrency,
        });
        event_seq += 1;
        if tx
            .send(stream_event_chunk(format, "started", event_seq, &started_event))
            .await
            .is_err()
        {
            return;
        }

        struct StreamState<'a> {
            s_data: &'a Arc<ServerData>,
            existing_index: &'a mut ExistingVehicleIndex,
            seen_ids: &'a mut HashSet<String>,
            seen_nums: &'a mut HashSet<String>,
            batch_number: &'a mut usize,
            total_fetched: &'a mut usize,
            total_created: &'a mut usize,
            total_updated: &'a mut usize,
            total_skipped: &'a mut usize,
            total_failed: &'a mut usize,
            total_duplicates: &'a mut usize,
            tx: &'a tokio::sync::mpsc::Sender<Result<Bytes, String>>,
            format: StreamSyncFormat,
            event_seq: &'a mut u64,
        }

        async fn emit_page_error(st: &mut StreamState<'_>, page: u64, err_msg: String) -> bool {
            *st.event_seq += 1;
            let err_event = json!({
                "event": "page_error",
                "page": page,
                "error": err_msg,
            });
            st.tx
                .send(stream_event_chunk(
                    st.format,
                    "page_error",
                    *st.event_seq,
                    &err_event,
                ))
                .await
                .is_ok()
        }

        async fn drain_and_emit(
            pending: &mut Vec<Value>,
            flush_remainder: bool,
            effective_batch_size: usize,
            effective_concurrency: usize,
            pages_completed: u64,
            total_pages: u64,
            st: &mut StreamState<'_>,
        ) -> bool {
            while pending.len() >= effective_batch_size
                || (flush_remainder && !pending.is_empty())
            {
                let take_len = pending.len().min(effective_batch_size);
                let batch: Vec<Value> = pending.drain(..take_len).collect();
                *st.total_fetched += batch.len();

                let outcome = sync_stream_batch(
                    st.s_data,
                    batch,
                    st.existing_index,
                    st.seen_ids,
                    st.seen_nums,
                    effective_concurrency,
                )
                .await;

                *st.batch_number += 1;
                *st.total_created += outcome.created;
                *st.total_updated += outcome.updated;
                *st.total_skipped += outcome.skipped;
                *st.total_failed += outcome.failed;
                *st.total_duplicates += outcome.duplicates_filtered;

                let event = json!({
                    "event": "batch",
                    "batchNumber": *st.batch_number,
                    "pagesCompleted": pages_completed,
                    "totalPages": total_pages,
                    "batchCount": outcome.streamed_vehicles.len(),
                    "created": outcome.created,
                    "updated": outcome.updated,
                    "skipped": outcome.skipped,
                    "failed": outcome.failed,
                    "duplicatesFiltered": outcome.duplicates_filtered,
                    "totals": {
                        "fetched": *st.total_fetched,
                        "created": *st.total_created,
                        "updated": *st.total_updated,
                        "skipped": *st.total_skipped,
                        "failed": *st.total_failed,
                        "duplicatesFiltered": *st.total_duplicates,
                    },
                    "vehicles": outcome.streamed_vehicles,
                });

                *st.event_seq += 1;
                if st
                    .tx
                    .send(stream_event_chunk(st.format, "batch", *st.event_seq, &event))
                    .await
                    .is_err()
                {
                    return false;
                }
            }
            true
        }

        let mut st = StreamState {
            s_data: &s_data,
            existing_index: &mut existing_index,
            seen_ids: &mut seen_ids,
            seen_nums: &mut seen_nums,
            batch_number: &mut batch_number,
            total_fetched: &mut total_fetched,
            total_created: &mut total_created,
            total_updated: &mut total_updated,
            total_skipped: &mut total_skipped,
            total_failed: &mut total_failed,
            total_duplicates: &mut total_duplicates,
            tx: &tx,
            format,
            event_seq: &mut event_seq,
        };

        if total_items > 0 && total_pages > 0 {
            pages_completed = 1;
            pending.extend(first_page_vehicles);
            if !drain_and_emit(
                &mut pending,
                false,
                effective_batch_size,
                effective_concurrency,
                pages_completed,
                total_pages,
                &mut st,
            )
            .await
            {
                return;
            }
        }

        if total_pages > 1 {
            let mut page_stream = stream::iter(2..=total_pages)
                .map(|page| {
                    let client = s_data.tqms_client.clone();
                    let tx_check = tx.clone();
                    async move {
                        if tx_check.is_closed() {
                            return (page, Err("Client disconnected".to_string()));
                        }
                        let res = fetch_tqms_vehicle_page_with_backoff(
                            &client,
                            page as u32,
                            page_size,
                            max_attempts,
                            retry_base_delay_ms,
                        )
                        .await
                        .map_err(|e| e.to_string());
                        (page, res)
                    }
                })
                .buffer_unordered(effective_concurrency);

            while let Some((requested_page, result)) = page_stream.next().await {
                if tx.is_closed() {
                    return;
                }

                match result {
                    Ok(mut response) => {
                        if let Err(err) = validate_tqms_response(&response, requested_page) {
                            failed_pages.push(requested_page);
                            if !emit_page_error(&mut st, requested_page, err.to_string()).await {
                                return;
                            }
                            continue;
                        }

                        match take_vehicles(&mut response) {
                            Ok(vehicles) => {
                                pages_completed += 1;
                                pending.extend(vehicles);
                                if !drain_and_emit(
                                    &mut pending,
                                    false,
                                    effective_batch_size,
                                    effective_concurrency,
                                    pages_completed,
                                    total_pages,
                                    &mut st,
                                )
                                .await
                                {
                                    return;
                                }
                            }
                            Err(err) => {
                                failed_pages.push(requested_page);
                                if !emit_page_error(&mut st, requested_page, err.to_string()).await
                                {
                                    return;
                                }
                            }
                        }
                    }
                    Err(err_msg) => {
                        failed_pages.push(requested_page);
                        if !emit_page_error(&mut st, requested_page, err_msg).await {
                            return;
                        }
                    }
                }
            }
        }

        if !drain_and_emit(
            &mut pending,
            true,
            effective_batch_size,
            effective_concurrency,
            pages_completed,
            total_pages,
            &mut st,
        )
        .await
        {
            return;
        }

        failed_pages.sort_unstable();
        let completed_event = json!({
            "event": "completed",
            "success": failed_pages.is_empty() && total_failed == 0,
            "pagesCompleted": pages_completed,
            "totalPages": total_pages,
            "totalItems": total_items,
            "totalFetched": total_fetched,
            "totalCreated": total_created,
            "totalUpdated": total_updated,
            "totalSkipped": total_skipped,
            "totalFailed": total_failed,
            "duplicatesFiltered": total_duplicates,
            "failedPages": failed_pages,
            "durationMs": started_at.elapsed().as_millis() as u64,
        });
        event_seq += 1;
        let _ = tx
            .send(stream_event_chunk(
                format,
                "completed",
                event_seq,
                &completed_event,
            ))
            .await;
    });

    let body_stream = stream::unfold(rx, move |mut rx| async move {
        match format {
            StreamSyncFormat::Sse => match tokio::time::timeout(
                Duration::from_secs(STREAM_SYNC_SSE_HEARTBEAT_SECS),
                rx.recv(),
            )
            .await
            {
                Ok(Some(item)) => Some((item.map_err(ErrorInternalServerError), rx)),
                Ok(None) => None,
                Err(_) => Some((Ok(Bytes::from_static(b": keep-alive\n\n")), rx)),
            },
            StreamSyncFormat::Ndjson => rx
                .recv()
                .await
                .map(|item| (item.map_err(ErrorInternalServerError), rx)),
        }
    });

    HttpResponse::Ok()
        .insert_header((header::CONTENT_TYPE, format.content_type()))
        .insert_header((
            header::CACHE_CONTROL,
            "no-cache, no-store, no-transform, must-revalidate",
        ))
        .insert_header((header::PRAGMA, "no-cache"))
        .insert_header((header::EXPIRES, "0"))
        .insert_header((header::CONNECTION, "keep-alive"))
        .insert_header((header::CONTENT_ENCODING, "identity"))
        .insert_header((header::VARY, "Accept"))
        .insert_header(("X-Accel-Buffering", "no"))
        .insert_header(("X-Content-Type-Options", "nosniff"))
        .insert_header(("Surrogate-Control", "no-store"))
        .insert_header(("X-Stream-Format", format.header_label()))
        .insert_header(("X-Batch-Size", effective_batch_size.to_string()))
        .insert_header(("X-Concurrency", effective_concurrency.to_string()))
        .insert_header(("X-Total-Items", total_items.to_string()))
        .insert_header(("X-Total-Pages", total_pages.to_string()))
        .streaming::<_, Error>(body_stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::Cache;
    use tokio::sync::RwLock;

    fn make_cached_vehicle(id: i32, vehicle_id: i64, vehicle_number: &str, owner: &str) -> DatabaseResource {
        DatabaseResource {
            id: Some(id),
            resource_name: Some(VehicleResource),
            inner_content: Some(json!({
                "VehicleId": vehicle_id,
                "VehicleNumber": vehicle_number,
                "NormalizedVehicleNumber": vehicle_number,
                "Registration": { "Status": "ACTIVE" },
                "Owners": [{ "Name": owner, "SanghBranch": "Main Branch" }]
            })),
            external_content: None,
        }
    }

    #[test]
    fn test_search_vehicles_by_number_also_finds_by_vehicle_id_in_cache_and_displays_both() {
        let mut cache = Cache::new();
        // Cached vehicle shares VehicleId 36017 with TQMS result even with an older number
        cache.write(
            VehicleResource,
            6390,
            make_cached_vehicle(6390, 36017, "LU-548-GA-0001", "Cached Owner"),
        );
        // Second vehicle matching directly by normalized vehicle number
        cache.write(
            VehicleResource,
            6391,
            make_cached_vehicle(6391, 99999, "BA-01-PA-1234", "Another Owner"),
        );

        let params = VehicleSearchParameters {
            page: Some(1),
            page_size: Some(20),
            vehicle_number: Some("LU-548-GA-6236".to_string()),
            owner_name: None,
            sangh_branch: None,
            status: None,
        };

        let tqms_response = json!({
            "Code": 200,
            "Message": "Success",
            "Data": {
                "Data": [
                    {
                        "VehicleId": 36017,
                        "VehicleNumber": "LU-548-GA-6236",
                        "Registration": { "Status": "ACTIVE" },
                        "Owners": [{ "Name": "TQMS Owner" }]
                    }
                ],
                "Paging": { "Page": 1, "TotalItems": 1 }
            }
        });

        let merged = merge_and_annotate_vehicle_results(&cache, &params, Ok(tqms_response))
            .expect("expected merged search response");

        assert!(
            merged.pointer("/Data/Data").is_none(),
            "redundant Data.Data array must not be present"
        );

        let cache_items = merged
            .pointer("/Data/cache_results")
            .and_then(Value::as_array)
            .unwrap();
        let tqms_items = merged
            .pointer("/Data/tqms_results")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(cache_items.len(), 1);
        assert_eq!(tqms_items.len(), 1);

        assert_eq!(cache_items[0]["VehicleId"], 36017);
        assert_eq!(cache_items[0]["source"], "cache");
        assert_eq!(cache_items[0]["cache_id"], 6390);
        assert_eq!(cache_items[0]["is_in_billing"], true);
        assert_eq!(cache_items[0]["is_in_tqms"], true);

        assert_eq!(tqms_items[0]["VehicleId"], 36017);
        assert_eq!(tqms_items[0]["source"], "tqms");
        assert_eq!(tqms_items[0]["is_in_billing"], true);
        assert_eq!(tqms_items[0]["is_in_tqms"], true);
    }

    #[test]
    fn test_search_vehicles_cache_only_and_direct_vehicle_id_query() {
        let mut cache = Cache::new();
        cache.write(
            VehicleResource,
            10,
            make_cached_vehicle(10, 36048, "KA-02-GHA-0006", "Aman Test"),
        );

        // Searching with vehicle_number containing the VehicleId finds cache even when TQMS fails
        let params_by_id = VehicleSearchParameters {
            page: Some(1),
            page_size: Some(20),
            vehicle_number: Some("36048".to_string()),
            owner_name: None,
            sangh_branch: None,
            status: None,
        };
        let merged = merge_and_annotate_vehicle_results(
            &cache,
            &params_by_id,
            Err(anyhow::anyhow!("TQMS offline")),
        )
        .expect("cache hit should still return 200-equivalent payload");

        assert!(merged.pointer("/Data/Data").is_none());
        let cache_items = merged
            .pointer("/Data/cache_results")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(cache_items.len(), 1);
        assert_eq!(cache_items[0]["VehicleId"], 36048);
        assert_eq!(cache_items[0]["is_in_billing"], true);
        assert_eq!(cache_items[0]["is_in_tqms"], false);
        assert!(merged
            .pointer("/Data/tqms_results")
            .and_then(Value::as_array)
            .unwrap()
            .is_empty());

        // Searching same number as both cache and TQMS returns cache_results + tqms_results without Data.Data
        let params_by_num = VehicleSearchParameters {
            page: Some(1),
            page_size: Some(20),
            vehicle_number: Some("ka 02 gha 0006".to_string()),
            owner_name: None,
            sangh_branch: None,
            status: None,
        };
        let tqms_same = json!({
            "Code": 200,
            "Data": {
                "Data": [{ "VehicleId": 36048, "VehicleNumber": "KA-02-GHA-0006" }]
            }
        });
        let merged_both = merge_and_annotate_vehicle_results(&cache, &params_by_num, Ok(tqms_same))
            .expect("expected both cache and TQMS results");
        assert!(merged_both.pointer("/Data/Data").is_none());
        assert_eq!(
            merged_both
                .pointer("/Data/cache_results")
                .and_then(Value::as_array)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            merged_both
                .pointer("/Data/tqms_results")
                .and_then(Value::as_array)
                .unwrap()
                .len(),
            1
        );

        // Even if multiple duplicate vehicle resources exist in the underlying map with the same VehicleId,
        // only the latest one is returned in cache_results.
        let mut stale_res = make_cached_vehicle(5, 36048, "KA-02-GHA-0006", "Old Owner");
        stale_res.external_content = Some(json!({ "createdTime": "2026-01-01T00:00:00Z" }));
        let mut latest_res = make_cached_vehicle(12, 36048, "KA-02-GHA-0006", "Latest Owner");
        latest_res.external_content = Some(json!({ "createdTime": "2026-11-18T00:00:00Z" }));
        {
            let map = cache.cache.get_mut(&VehicleResource).unwrap();
            map.clear();
            map.insert(5, stale_res);
            map.insert(12, latest_res);
        }
        let merged_latest =
            merge_and_annotate_vehicle_results(&cache, &params_by_id, Err(anyhow::anyhow!("offline")))
                .unwrap();
        let deduped_cache_items = merged_latest
            .pointer("/Data/cache_results")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(deduped_cache_items.len(), 1);
        assert_eq!(deduped_cache_items[0]["cache_id"], 12);
        assert_eq!(deduped_cache_items[0]["Owners"][0]["Name"], "Latest Owner");
    }

    struct MockDbState {
        rows: Vec<DatabaseResource>,
        inserted: Vec<DatabaseResource>,
        select_calls: usize,
        next_id: i32,
    }

    fn append_row_desc_field(buf: &mut Vec<u8>, name: &str, type_oid: i32, type_size: i16) {
        buf.extend_from_slice(name.as_bytes());
        buf.push(0);
        buf.extend_from_slice(&0i32.to_be_bytes()); // table oid
        buf.extend_from_slice(&0i16.to_be_bytes()); // col attr
        buf.extend_from_slice(&type_oid.to_be_bytes());
        buf.extend_from_slice(&type_size.to_be_bytes());
        buf.extend_from_slice(&(-1i32).to_be_bytes()); // type mod
        buf.extend_from_slice(&1i16.to_be_bytes()); // format = binary
    }

    fn push_pg_msg(out: &mut Vec<u8>, tag: u8, payload: &[u8]) {
        out.push(tag);
        out.extend_from_slice(&((payload.len() + 4) as i32).to_be_bytes());
        out.extend_from_slice(payload);
    }

    fn decode_cstr(buf: &[u8], pos: &mut usize) -> String {
        let start = *pos;
        let end = buf[start..]
            .iter()
            .position(|&b| b == 0)
            .map(|rel| start + rel)
            .unwrap_or(buf.len());
        *pos = (end + 1).min(buf.len());
        String::from_utf8_lossy(&buf[start..end]).to_string()
    }

    fn decode_bind_json_params(body: &[u8]) -> (Option<Value>, Option<Value>) {
        let mut pos = 0usize;
        let _portal = decode_cstr(body, &mut pos);
        let _stmt = decode_cstr(body, &mut pos);
        if pos + 2 > body.len() {
            return (None, None);
        }
        let fmt_count = i16::from_be_bytes([body[pos], body[pos + 1]]) as usize;
        pos += 2 + fmt_count * 2;
        if pos + 2 > body.len() {
            return (None, None);
        }
        let param_count = i16::from_be_bytes([body[pos], body[pos + 1]]) as usize;
        pos += 2;
        let mut parsed = Vec::with_capacity(param_count);
        for _ in 0..param_count {
            if pos + 4 > body.len() {
                break;
            }
            let len = i32::from_be_bytes([body[pos], body[pos + 1], body[pos + 2], body[pos + 3]]);
            pos += 4;
            if len < 0 {
                parsed.push(None);
            } else {
                let ulen = len as usize;
                let end = (pos + ulen).min(body.len());
                let slice = &body[pos..end];
                pos = end;
                let json_bytes = if slice.first() == Some(&1) {
                    &slice[1..]
                } else {
                    slice
                };
                parsed.push(serde_json::from_slice::<Value>(json_bytes).ok());
            }
        }
        (
            parsed.first().cloned().flatten(),
            parsed.get(1).cloned().flatten(),
        )
    }

    async fn create_mock_server_data_with_db(
        initial_db_rows: Vec<DatabaseResource>,
        initial_cache: Cache,
    ) -> (Arc<ServerData>, Arc<tokio::sync::Mutex<MockDbState>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let db_state = Arc::new(tokio::sync::Mutex::new(MockDbState {
            rows: initial_db_rows,
            inserted: Vec::new(),
            select_calls: 0,
            next_id: 9001,
        }));
        let state_for_task = Arc::clone(&db_state);

        let (client_io, mut server_io) = tokio::io::duplex(65536);

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

            #[derive(Clone, Copy)]
            enum QueryKind {
                SelectVehicles,
                SelectOther,
                InsertVehicle,
                InsertOther,
            }

            let mut current_query = QueryKind::InsertVehicle;
            let mut pending_insert: (Option<Value>, Option<Value>) = (None, None);
            let mut resp_buf: Vec<u8> = Vec::new();
            let mut other_insert_id: i32 = 50_000;

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
                        let sql = String::from_utf8_lossy(&body).to_ascii_lowercase();
                        let is_vehicle_table = sql.contains("vehicleresource");
                        current_query = if sql.contains("insert into") {
                            if is_vehicle_table {
                                QueryKind::InsertVehicle
                            } else {
                                QueryKind::InsertOther
                            }
                        } else if is_vehicle_table {
                            QueryKind::SelectVehicles
                        } else {
                            QueryKind::SelectOther
                        };
                        resp_buf.extend_from_slice(&[b'1', 0, 0, 0, 4]);
                    }
                    b'D' => {
                        if body.first() == Some(&b'S') {
                            match current_query {
                                QueryKind::SelectVehicles | QueryKind::SelectOther => {
                                    // 0 parameters
                                    push_pg_msg(&mut resp_buf, b't', &0i16.to_be_bytes());
                                    // 3 columns: id, inner_content, external_content
                                    let mut fields = Vec::new();
                                    fields.extend_from_slice(&3i16.to_be_bytes());
                                    append_row_desc_field(&mut fields, "id", 23, 4);
                                    append_row_desc_field(&mut fields, "inner_content", 3802, -1);
                                    append_row_desc_field(
                                        &mut fields,
                                        "external_content",
                                        3802,
                                        -1,
                                    );
                                    push_pg_msg(&mut resp_buf, b'T', &fields);
                                }
                                QueryKind::InsertVehicle | QueryKind::InsertOther => {
                                    // 2 JSONB params (3802)
                                    resp_buf.extend_from_slice(&[
                                        b't', 0, 0, 0, 14, 0, 2, 0, 0, 0x0e, 0xda, 0, 0, 0x0e,
                                        0xda,
                                    ]);
                                    let mut fields = Vec::new();
                                    fields.extend_from_slice(&1i16.to_be_bytes());
                                    append_row_desc_field(&mut fields, "id", 23, 4);
                                    push_pg_msg(&mut resp_buf, b'T', &fields);
                                }
                            }
                        }
                    }
                    b'B' => {
                        if matches!(current_query, QueryKind::InsertVehicle) {
                            pending_insert = decode_bind_json_params(&body);
                        }
                        resp_buf.extend_from_slice(&[b'2', 0, 0, 0, 4]);
                    }
                    b'E' => {
                        let mut guard = state_for_task.lock().await;
                        match current_query {
                            QueryKind::SelectVehicles => {
                                guard.select_calls += 1;
                                for row in &guard.rows {
                                    let mut row_buf = Vec::new();
                                    row_buf.extend_from_slice(&3i16.to_be_bytes());
                                    // col 0: id (i32)
                                    let id_val = row.id.unwrap_or_default();
                                    row_buf.extend_from_slice(&4i32.to_be_bytes());
                                    row_buf.extend_from_slice(&id_val.to_be_bytes());
                                    // col 1: inner_content (jsonb binary: 0x01 + utf8)
                                    if let Some(inner) = &row.inner_content {
                                        let json_bytes = serde_json::to_vec(inner).unwrap();
                                        let len = (1 + json_bytes.len()) as i32;
                                        row_buf.extend_from_slice(&len.to_be_bytes());
                                        row_buf.push(1);
                                        row_buf.extend_from_slice(&json_bytes);
                                    } else {
                                        row_buf.extend_from_slice(&(-1i32).to_be_bytes());
                                    }
                                    // col 2: external_content
                                    if let Some(ext) = &row.external_content {
                                        let json_bytes = serde_json::to_vec(ext).unwrap();
                                        let len = (1 + json_bytes.len()) as i32;
                                        row_buf.extend_from_slice(&len.to_be_bytes());
                                        row_buf.push(1);
                                        row_buf.extend_from_slice(&json_bytes);
                                    } else {
                                        row_buf.extend_from_slice(&(-1i32).to_be_bytes());
                                    }
                                    push_pg_msg(&mut resp_buf, b'D', &row_buf);
                                }
                                let complete = format!("SELECT {}\0", guard.rows.len());
                                push_pg_msg(&mut resp_buf, b'C', complete.as_bytes());
                            }
                            QueryKind::SelectOther => {
                                push_pg_msg(&mut resp_buf, b'C', b"SELECT 0\0");
                            }
                            QueryKind::InsertVehicle => {
                                let assigned_id = guard.next_id;
                                guard.next_id += 1;
                                let (inner_val, ext_val) = pending_insert.clone();
                                let new_row = DatabaseResource {
                                    id: Some(assigned_id),
                                    resource_name: Some(VehicleResource),
                                    inner_content: inner_val,
                                    external_content: ext_val,
                                };
                                guard.rows.push(new_row.clone());
                                guard.inserted.push(new_row);

                                let mut row_buf = Vec::new();
                                row_buf.extend_from_slice(&1i16.to_be_bytes());
                                row_buf.extend_from_slice(&4i32.to_be_bytes());
                                row_buf.extend_from_slice(&assigned_id.to_be_bytes());
                                push_pg_msg(&mut resp_buf, b'D', &row_buf);
                                push_pg_msg(&mut resp_buf, b'C', b"INSERT 0 1\0");
                            }
                            QueryKind::InsertOther => {
                                let assigned_id = other_insert_id;
                                other_insert_id += 1;
                                let mut row_buf = Vec::new();
                                row_buf.extend_from_slice(&1i16.to_be_bytes());
                                row_buf.extend_from_slice(&4i32.to_be_bytes());
                                row_buf.extend_from_slice(&assigned_id.to_be_bytes());
                                push_pg_msg(&mut resp_buf, b'D', &row_buf);
                                push_pg_msg(&mut resp_buf, b'C', b"INSERT 0 1\0");
                            }
                        }
                    }
                    b'S' => {
                        resp_buf.extend_from_slice(&[b'Z', 0, 0, 0, 5, b'I']);
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

        let s_data = Arc::new(ServerData {
            db: Arc::new(crate::db::Database::new(Arc::new(client))),
            tqms_client: Arc::new(TQMSClient::new().await),
            cache: Arc::new(RwLock::new(initial_cache)),
        });

        (s_data, db_state)
    }

    #[actix_web::test]
    async fn test_tqms_vehicle_not_in_cache_or_db_creates_vehicle_resource_and_caches_it() {
        let (s_data, db_state) = create_mock_server_data_with_db(Vec::new(), Cache::new()).await;

        let tqms_payload = json!({
            "Code": 200,
            "Message": "Success",
            "Data": {
                "Data": [
                    {
                        "VehicleId": 777,
                        "VehicleNumber": "ना १ ज ७७७७",
                        "VehicleType": "Jeep",
                        "Owners": [{ "Name": "TQMS New Owner", "SanghBranch": "Pokhara" }],
                        "source": "tqms",
                        "is_in_billing": false,
                        "is_in_tqms": true,
                        "cache_id": null
                    }
                ]
            }
        });

        let tqms_items = extract_tqms_vehicle_items(&tqms_payload);
        assert_eq!(tqms_items.len(), 1);

        let created = ensure_tqms_vehicles_in_cache_and_db(&s_data, &tqms_items).await;
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].id, Some(9001));
        assert_eq!(created[0].resource_name, Some(VehicleResource));

        let created_inner = created[0].inner_content.as_ref().unwrap();
        assert_eq!(created_inner["VehicleId"], 777);
        assert_eq!(created_inner["VehicleNumber"], "ना १ ज ७७७७");
        assert!(created_inner.get("source").is_none());
        assert!(created_inner.get("is_in_billing").is_none());

        // DB received exactly one INSERT with clean vehicle JSON
        {
            let guard = db_state.lock().await;
            assert_eq!(guard.inserted.len(), 1);
            assert_eq!(guard.inserted[0].id, Some(9001));
            assert_eq!(guard.inserted[0].inner_content.as_ref().unwrap()["VehicleId"], 777);
        }

        // Cache now holds the newly created VehicleResource under id 9001
        let cached = s_data
            .cache
            .read()
            .await
            .read(VehicleResource, 9001)
            .expect("newly created TQMS vehicle must be stored in cache");
        assert_eq!(cached.inner_content.as_ref().unwrap()["VehicleId"], 777);

        // Merging search results afterwards reflects it as in billing too
        let params = VehicleSearchParameters {
            page: Some(1),
            page_size: Some(20),
            vehicle_number: Some("ना १ ज ७७७७".to_string()),
            owner_name: None,
            sangh_branch: None,
            status: None,
        };
        let cache = s_data.cache.read().await;
        let merged = merge_and_annotate_vehicle_results(&cache, &params, Ok(tqms_payload)).unwrap();
        assert!(merged.pointer("/Data/Data").is_none());
        assert_eq!(merged.pointer("/Data/cache_results/0/cache_id").unwrap(), 9001);
        assert_eq!(merged.pointer("/Data/tqms_results/0/is_in_billing").unwrap(), true);
    }

    #[actix_web::test]
    async fn test_tqms_vehicle_already_in_cache_is_not_created_in_db() {
        let mut cache = Cache::new();
        cache.write(
            VehicleResource,
            101,
            make_cached_vehicle(101, 36017, "LU-548-GA-0001", "Existing By Id"),
        );
        cache.write(
            VehicleResource,
            102,
            make_cached_vehicle(102, 44444, "BA-01-PA-2222", "Existing By Num"),
        );

        let (s_data, db_state) = create_mock_server_data_with_db(Vec::new(), cache).await;

        let tqms_items = vec![
            // Same VehicleId 36017 even though plate changed
            json!({ "VehicleId": 36017, "VehicleNumber": "LU-548-GA-9999" }),
            // Same normalized VehicleNumber even with different formatting / id
            json!({ "VehicleId": 88888, "VehicleNumber": "ba 01 pa 2222" }),
        ];

        let created = ensure_tqms_vehicles_in_cache_and_db(&s_data, &tqms_items).await;
        assert!(created.is_empty());

        let guard = db_state.lock().await;
        assert_eq!(guard.select_calls, 0);
        assert!(guard.inserted.is_empty());
    }

    #[actix_web::test]
    async fn test_tqms_vehicle_missing_from_cache_but_present_in_db_syncs_cache_without_duplicating_db() {
        let existing_db_vehicle =
            make_cached_vehicle(555, 5050, "NA-04-KHA-5050", "Only In DB Initially");

        let (s_data, db_state) =
            create_mock_server_data_with_db(vec![existing_db_vehicle], Cache::new()).await;

        let tqms_items = vec![
            // Already in DB (by VehicleId / VehicleNumber)
            json!({ "VehicleId": 5050, "VehicleNumber": "NA-04-KHA-5050" }),
            // Brand new TQMS-only vehicle (not in cache or DB)
            json!({ "VehicleId": 6060, "VehicleNumber": "GA-02-CHA-6060" }),
            // Duplicate of the brand new vehicle inside same TQMS batch -> should only insert once
            json!({ "VehicleId": 6060, "VehicleNumber": "GA-02-CHA-6060" }),
        ];

        let created = ensure_tqms_vehicles_in_cache_and_db(&s_data, &tqms_items).await;
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].id, Some(9001));
        assert_eq!(created[0].inner_content.as_ref().unwrap()["VehicleId"], 6060);

        {
            let guard = db_state.lock().await;
            assert_eq!(guard.select_calls, 1);
            assert_eq!(guard.inserted.len(), 1, "must not duplicate DB vehicle or intra-batch vehicle");
        }

        let cache = s_data.cache.read().await;
        assert!(
            cache.read(VehicleResource, 555).is_some(),
            "existing DB vehicle should be warmed into cache"
        );
        assert!(
            cache.read(VehicleResource, 9001).is_some(),
            "newly created TQMS vehicle should be stored in cache"
        );
    }

    #[test]
    fn test_extract_tqms_vehicle_items_handles_search_details_and_non_vehicle_payloads() {
        let search_payload = json!({
            "Code": 200,
            "Data": {
                "Data": [
                    { "VehicleId": 1, "VehicleNumber": "BA-01-PA-0001" },
                    { "NotAVehicle": true }
                ]
            }
        });
        assert_eq!(extract_tqms_vehicle_items(&search_payload).len(), 1);

        let details_payload = json!({
            "Code": 200,
            "Data": {
                "VehicleId": 2,
                "VehicleNumber": "BA-02-PA-0002",
                "Owners": [{ "Name": "Test Owner" }]
            }
        });
        let detail_items = extract_tqms_vehicle_items(&details_payload);
        assert_eq!(detail_items.len(), 1);
        assert_eq!(detail_items[0]["VehicleId"], 2);

        let non_vehicle_payload = json!({
            "Code": 200,
            "Data": { "IsValid": true }
        });
        assert!(extract_tqms_vehicle_items(&non_vehicle_payload).is_empty());
    }

    async fn spawn_mock_tqms_server(
        pages: HashMap<u32, (u16, Value)>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock tqms tcp listener");
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{addr}");
        let pages = Arc::new(pages);

        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let pages = Arc::clone(&pages);
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let Ok(n) = socket.read(&mut buf).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    let req_str = String::from_utf8_lossy(&buf[..n]);
                    let first_line = req_str.lines().next().unwrap_or("");
                    let path = first_line.split_whitespace().nth(1).unwrap_or("");
                    let page_num = path
                        .split('?')
                        .nth(1)
                        .and_then(|qs| {
                            qs.split('&').find_map(|kv| {
                                let mut it = kv.splitn(2, '=');
                                if it.next() == Some("page") {
                                    it.next().and_then(|v| v.parse::<u32>().ok())
                                } else {
                                    None
                                }
                            })
                        })
                        .unwrap_or(1);

                    let (status, body_val) = pages.get(&page_num).cloned().unwrap_or_else(|| {
                        (
                            404,
                            json!({ "Code": 404, "Message": "Page not found" }),
                        )
                    });
                    let body = serde_json::to_string(&body_val).unwrap();
                    let status_text = if status == 200 {
                        "200 OK"
                    } else if status == 500 {
                        "500 Internal Server Error"
                    } else {
                        "404 Not Found"
                    };
                    let raw_resp = format!(
                        "HTTP/1.1 {status_text}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(raw_resp.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });

        (base_url, handle)
    }

    async fn create_stream_sync_server_data(
        initial_state: crate::access::test_helpers::MockDbState,
        tqms_base_url: String,
    ) -> (
        web::Data<Arc<ServerData>>,
        Arc<tokio::sync::Mutex<crate::access::test_helpers::MockDbState>>,
    ) {
        let (base_data, db_state) =
            crate::access::test_helpers::create_mock_server_data_with_state(initial_state).await;
        let combined = web::Data::new(Arc::new(ServerData {
            db: base_data.db.clone(),
            tqms_client: Arc::new(TQMSClient::with_base_url_and_token(
                tqms_base_url,
                "mock-tqms-token".to_string(),
            )),
            cache: base_data.cache.clone(),
        }));
        (combined, db_state)
    }

    fn assert_streaming_headers(
        headers: &actix_web::http::header::HeaderMap,
        expected_content_type: &str,
        expected_format: &str,
        expected_batch_size: &str,
        expected_concurrency: &str,
        expected_total_items: &str,
        expected_total_pages: &str,
    ) {
        let get_str = |name: &str| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        assert_eq!(get_str("content-type").as_deref(), Some(expected_content_type));
        assert_eq!(
            get_str("cache-control").as_deref(),
            Some("no-cache, no-store, no-transform, must-revalidate")
        );
        assert_eq!(get_str("pragma").as_deref(), Some("no-cache"));
        assert_eq!(get_str("expires").as_deref(), Some("0"));
        assert_eq!(get_str("connection").as_deref(), Some("keep-alive"));
        assert_eq!(get_str("content-encoding").as_deref(), Some("identity"));
        assert_eq!(get_str("vary").as_deref(), Some("Accept"));
        assert_eq!(get_str("x-accel-buffering").as_deref(), Some("no"));
        assert_eq!(get_str("x-content-type-options").as_deref(), Some("nosniff"));
        assert_eq!(get_str("surrogate-control").as_deref(), Some("no-store"));
        assert_eq!(get_str("x-stream-format").as_deref(), Some(expected_format));
        assert_eq!(get_str("x-batch-size").as_deref(), Some(expected_batch_size));
        assert_eq!(get_str("x-concurrency").as_deref(), Some(expected_concurrency));
        assert_eq!(get_str("x-total-items").as_deref(), Some(expected_total_items));
        assert_eq!(get_str("x-total-pages").as_deref(), Some(expected_total_pages));
    }

    #[actix_web::test]
    async fn test_stream_sync_rejects_zero_batch_size_or_concurrency() {
        let (s_data, _) = create_stream_sync_server_data(
            crate::access::test_helpers::MockDbState::default(),
            "http://127.0.0.1:1".to_string(),
        )
        .await;
        let req = actix_web::test::TestRequest::default().to_http_request();

        let resp_zero_batch =
            stream_sync_vehicles_tqms(req.clone(), web::Path::from((0, 4)), s_data.clone()).await;
        assert_eq!(resp_zero_batch.status(), actix_web::http::StatusCode::BAD_REQUEST);

        let resp_zero_conc =
            stream_sync_vehicles_tqms(req, web::Path::from((10, 0)), s_data).await;
        assert_eq!(resp_zero_conc.status(), actix_web::http::StatusCode::BAD_REQUEST);
    }

    #[actix_web::test]
    async fn test_stream_sync_streams_ndjson_batches_creates_updates_skips_and_deduplicates() {
        let unchanged_vehicle = json!({
            "VehicleId": 1001,
            "VehicleNumber": "NA-01-KHA-1001",
            "NormalizedVehicleNumber": "NA01KHA1001",
            "VehicleType": "Truck",
            "Owners": [{ "Name": "Unchanged Owner" }]
        });
        let existing_outdated_vehicle = json!({
            "VehicleId": 1002,
            "VehicleNumber": "NA-01-KHA-1002",
            "NormalizedVehicleNumber": "NA01KHA1002",
            "VehicleType": "Old Type",
            "Owners": [{ "Name": "Old Owner" }],
            "metadata": { "blacklisted": true }
        });
        let fresh_updated_vehicle_1002 = json!({
            "VehicleId": 1002,
            "VehicleNumber": "NA-01-KHA-1002",
            "NormalizedVehicleNumber": "NA01KHA1002",
            "VehicleType": "New Type",
            "Owners": [{ "Name": "Updated Owner" }]
        });
        let new_vehicle_1003 = json!({
            "VehicleId": 1003,
            "VehicleNumber": "NA-01-KHA-1003",
            "NormalizedVehicleNumber": "NA01KHA1003",
            "VehicleType": "Tipper",
            "Owners": [{ "Name": "New Owner 3" }]
        });
        let new_vehicle_1004 = json!({
            "VehicleId": 1004,
            "VehicleNumber": "NA-01-KHA-1004",
            "NormalizedVehicleNumber": "NA01KHA1004",
            "VehicleType": "Mini Truck",
            "Owners": [{ "Name": "New Owner 4" }]
        });
        let new_vehicle_1005 = json!({
            "VehicleId": 1005,
            "VehicleNumber": "NA-01-KHA-1005",
            "NormalizedVehicleNumber": "NA01KHA1005",
            "VehicleType": "Trailer",
            "Owners": [{ "Name": "New Owner 5" }]
        });

        let mut state = crate::access::test_helpers::MockDbState::default();
        state.seed(
            VehicleResource,
            DatabaseResource {
                id: Some(1),
                resource_name: Some(VehicleResource),
                inner_content: Some(unchanged_vehicle.clone()),
                external_content: None,
            },
        );
        state.seed(
            VehicleResource,
            DatabaseResource {
                id: Some(2),
                resource_name: Some(VehicleResource),
                inner_content: Some(existing_outdated_vehicle),
                external_content: Some(json!({ "createdTime": "2026-01-01T00:00:00Z" })),
            },
        );

        let mut pages = HashMap::new();
        pages.insert(
            1,
            (
                200,
                json!({
                    "Code": 200,
                    "Message": "Success",
                    "Data": {
                        "Data": [unchanged_vehicle, fresh_updated_vehicle_1002],
                        "Paging": { "Page": 1, "PageSize": 2, "TotalItems": 6, "TotalPages": 3 }
                    }
                }),
            ),
        );
        pages.insert(
            2,
            (
                200,
                json!({
                    "Code": 200,
                    "Message": "Success",
                    "Data": {
                        "Data": [new_vehicle_1003.clone(), new_vehicle_1004],
                        "Paging": { "Page": 2, "PageSize": 2, "TotalItems": 6, "TotalPages": 3 }
                    }
                }),
            ),
        );
        // Page 3 includes duplicate 1003 plus new 1005
        pages.insert(
            3,
            (
                200,
                json!({
                    "Code": 200,
                    "Message": "Success",
                    "Data": {
                        "Data": [new_vehicle_1003, new_vehicle_1005],
                        "Paging": { "Page": 3, "PageSize": 2, "TotalItems": 6, "TotalPages": 3 }
                    }
                }),
            ),
        );

        let (tqms_url, server_handle) = spawn_mock_tqms_server(pages).await;
        let (s_data, db_state) = create_stream_sync_server_data(state, tqms_url).await;

        let app = actix_web::test::init_service(
            actix_web::App::new()
                .app_data(s_data.clone())
                .route(
                    "/api/v1/tqms/stream_sync/{batch_size}/{concurrency}",
                    web::get().to(stream_sync_vehicles_tqms),
                ),
        )
        .await;

        let req = actix_web::test::TestRequest::get()
            .uri("/api/v1/tqms/stream_sync/2/2")
            .insert_header((header::ACCEPT, "application/x-ndjson"))
            .to_request();
        let resp = actix_web::test::call_service(&app, req).await;
        assert_eq!(resp.status(), actix_web::http::StatusCode::OK);
        assert_streaming_headers(
            resp.headers(),
            "application/x-ndjson; charset=utf-8",
            "ndjson",
            "2",
            "2",
            "6",
            "3",
        );

        let body_bytes = actix_web::test::read_body(resp).await;
        server_handle.abort();

        let body_text = String::from_utf8(body_bytes.to_vec()).expect("valid utf8 ndjson");
        let events: Vec<Value> = body_text
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("valid json line"))
            .collect();

        assert_eq!(events.first().unwrap()["event"], "started");
        assert_eq!(events.last().unwrap()["event"], "completed");

        let batch_events: Vec<&Value> = events
            .iter()
            .filter(|e| e["event"] == "batch")
            .collect();
        assert!(!batch_events.is_empty());

        let completed = events.last().unwrap();
        assert_eq!(completed["success"], true);
        assert_eq!(completed["totalPages"], 3);
        assert_eq!(completed["pagesCompleted"], 3);
        assert_eq!(completed["totalFetched"], 6);
        assert_eq!(completed["totalCreated"], 3);
        assert_eq!(completed["totalUpdated"], 1);
        assert_eq!(completed["totalSkipped"], 1);
        assert_eq!(completed["duplicatesFiltered"], 1);
        assert_eq!(completed["totalFailed"], 0);
        assert_eq!(completed["failedPages"], json!([]));

        {
            let guard = db_state.lock().await;
            let db_rows = guard.tables.get("vehicleresource").unwrap();
            assert_eq!(db_rows.len(), 5, "should store exactly 5 unique vehicles");

            let updated_1002 = db_rows.iter().find(|r| r.id == Some(2)).unwrap();
            let u_inner = updated_1002.inner_content.as_ref().unwrap();
            assert_eq!(u_inner["VehicleType"], "New Type");
            assert_eq!(u_inner["metadata"]["blacklisted"], true);
        }

        let cache = s_data.cache.read().await;
        let cached_all = cache.read_all(VehicleResource).unwrap();
        assert_eq!(cached_all.len(), 5);
    }

    #[actix_web::test]
    async fn test_stream_sync_handles_page_failure_gracefully_and_streams_remaining_pages() {
        let mut pages = HashMap::new();
        pages.insert(
            1,
            (
                200,
                json!({
                    "Code": 200,
                    "Message": "Success",
                    "Data": {
                        "Data": [{ "VehicleId": 2001, "VehicleNumber": "BA-01-PA-2001" }],
                        "Paging": { "Page": 1, "PageSize": 1, "TotalItems": 3, "TotalPages": 3 }
                    }
                }),
            ),
        );
        pages.insert(2, (500, json!({ "Code": 500, "Message": "TQMS error" })));
        pages.insert(
            3,
            (
                200,
                json!({
                    "Code": 200,
                    "Message": "Success",
                    "Data": {
                        "Data": [{ "VehicleId": 2003, "VehicleNumber": "BA-01-PA-2003" }],
                        "Paging": { "Page": 3, "PageSize": 1, "TotalItems": 3, "TotalPages": 3 }
                    }
                }),
            ),
        );

        let (tqms_url, server_handle) = spawn_mock_tqms_server(pages).await;
        let (s_data, db_state) = create_stream_sync_server_data(
            crate::access::test_helpers::MockDbState::default(),
            tqms_url,
        )
        .await;

        let resp = stream_sync_vehicles_tqms_with_retry(
            1,
            2,
            StreamSyncFormat::Ndjson,
            s_data.get_ref().clone(),
            2,
            0,
        )
        .await;
        assert_eq!(resp.status(), actix_web::http::StatusCode::OK);

        let body_bytes = actix_web::body::to_bytes(resp.into_body()).await.unwrap();
        server_handle.abort();

        let body_text = String::from_utf8(body_bytes.to_vec()).unwrap();
        let events: Vec<Value> = body_text
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .collect();

        assert!(events
            .iter()
            .any(|e| e["event"] == "page_error" && e["page"] == 2));

        let completed = events.last().unwrap();
        assert_eq!(completed["event"], "completed");
        assert_eq!(completed["success"], false);
        assert_eq!(completed["totalCreated"], 2);
        assert_eq!(completed["pagesCompleted"], 2);
        assert_eq!(completed["failedPages"], json!([2]));

        let guard = db_state.lock().await;
        assert_eq!(guard.tables.get("vehicleresource").unwrap().len(), 2);
    }

    #[actix_web::test]
    async fn test_stream_sync_returns_502_when_initial_tqms_page_fails() {
        let mut pages = HashMap::new();
        pages.insert(1, (500, json!({ "Code": 500, "Message": "Down" })));

        let (tqms_url, server_handle) = spawn_mock_tqms_server(pages).await;
        let (s_data, _) = create_stream_sync_server_data(
            crate::access::test_helpers::MockDbState::default(),
            tqms_url,
        )
        .await;

        let resp = stream_sync_vehicles_tqms_with_retry(
            10,
            2,
            StreamSyncFormat::Sse,
            s_data.get_ref().clone(),
            1,
            0,
        )
        .await;
        server_handle.abort();

        assert_eq!(resp.status(), actix_web::http::StatusCode::BAD_GATEWAY);
    }

    #[actix_web::test]
    async fn test_stream_sync_defaults_to_sse_with_proper_streaming_headers_and_framing() {
        // 1. Format negotiation checks
        let req_default = actix_web::test::TestRequest::get()
            .uri("/api/v1/tqms/stream_sync/2/2")
            .to_http_request();
        assert_eq!(StreamSyncFormat::from_request(&req_default), StreamSyncFormat::Sse);

        let req_accept_sse = actix_web::test::TestRequest::get()
            .uri("/api/v1/tqms/stream_sync/2/2")
            .insert_header((header::ACCEPT, "text/event-stream"))
            .to_http_request();
        assert_eq!(StreamSyncFormat::from_request(&req_accept_sse), StreamSyncFormat::Sse);

        let req_accept_ndjson = actix_web::test::TestRequest::get()
            .uri("/api/v1/tqms/stream_sync/2/2")
            .insert_header((header::ACCEPT, "application/x-ndjson"))
            .to_http_request();
        assert_eq!(
            StreamSyncFormat::from_request(&req_accept_ndjson),
            StreamSyncFormat::Ndjson
        );

        let req_accept_both = actix_web::test::TestRequest::get()
            .uri("/api/v1/tqms/stream_sync/2/2")
            .insert_header((header::ACCEPT, "text/event-stream, application/x-ndjson"))
            .to_http_request();
        assert_eq!(StreamSyncFormat::from_request(&req_accept_both), StreamSyncFormat::Sse);

        let req_query_ndjson = actix_web::test::TestRequest::get()
            .uri("/api/v1/tqms/stream_sync/2/2?format=ndjson")
            .insert_header((header::ACCEPT, "text/event-stream"))
            .to_http_request();
        assert_eq!(
            StreamSyncFormat::from_request(&req_query_ndjson),
            StreamSyncFormat::Ndjson
        );

        let req_query_sse = actix_web::test::TestRequest::get()
            .uri("/api/v1/tqms/stream_sync/2/2?format=sse")
            .insert_header((header::ACCEPT, "application/x-ndjson"))
            .to_http_request();
        assert_eq!(StreamSyncFormat::from_request(&req_query_sse), StreamSyncFormat::Sse);

        // 2. End-to-end SSE response headers & frame parsing with no Accept header
        let v1 = json!({
            "VehicleId": 3001,
            "VehicleNumber": "BA-01-PA-3001",
            "NormalizedVehicleNumber": "BA01PA3001",
            "VehicleType": "Truck",
            "Owners": [{ "Name": "SSE Owner 1" }]
        });
        let v2 = json!({
            "VehicleId": 3002,
            "VehicleNumber": "BA-01-PA-3002",
            "NormalizedVehicleNumber": "BA01PA3002",
            "VehicleType": "Tipper",
            "Owners": [{ "Name": "SSE Owner 2" }]
        });
        let mut pages = HashMap::new();
        pages.insert(
            1,
            (
                200,
                json!({
                    "Code": 200,
                    "Message": "Success",
                    "Data": {
                        "Data": [v1, v2],
                        "Paging": { "Page": 1, "PageSize": 2, "TotalItems": 2, "TotalPages": 1 }
                    }
                }),
            ),
        );

        let (tqms_url, server_handle) = spawn_mock_tqms_server(pages).await;
        let (s_data, db_state) = create_stream_sync_server_data(
            crate::access::test_helpers::MockDbState::default(),
            tqms_url,
        )
        .await;

        let app = actix_web::test::init_service(
            actix_web::App::new()
                .app_data(s_data)
                .route(
                    "/api/v1/tqms/stream_sync/{batch_size}/{concurrency}",
                    web::get().to(stream_sync_vehicles_tqms),
                ),
        )
        .await;

        let resp = actix_web::test::call_service(
            &app,
            actix_web::test::TestRequest::get()
                .uri("/api/v1/tqms/stream_sync/2/2")
                .to_request(),
        )
        .await;
        assert_eq!(resp.status(), actix_web::http::StatusCode::OK);
        assert_streaming_headers(
            resp.headers(),
            "text/event-stream; charset=utf-8",
            "sse",
            "2",
            "2",
            "2",
            "1",
        );

        let body_bytes = actix_web::test::read_body(resp).await;
        server_handle.abort();

        let body_text = String::from_utf8(body_bytes.to_vec()).expect("valid utf8 sse");
        let raw_frames: Vec<&str> = body_text
            .split("\n\n")
            .filter(|f| !f.trim().is_empty())
            .collect();
        assert_eq!(raw_frames.len(), 3, "expected started, batch, completed SSE frames");

        let mut parsed_frames: Vec<(u64, String, Value)> = Vec::new();
        for frame in raw_frames {
            let mut id_val: Option<u64> = None;
            let mut event_val: Option<String> = None;
            let mut data_val: Option<Value> = None;
            for line in frame.lines() {
                if let Some(rest) = line.strip_prefix("id:") {
                    id_val = rest.trim().parse::<u64>().ok();
                } else if let Some(rest) = line.strip_prefix("event:") {
                    event_val = Some(rest.trim().to_string());
                } else if let Some(rest) = line.strip_prefix("data:") {
                    data_val = Some(serde_json::from_str(rest.trim()).expect("valid sse json data"));
                }
            }
            parsed_frames.push((
                id_val.expect("sse frame missing id"),
                event_val.expect("sse frame missing event"),
                data_val.expect("sse frame missing data"),
            ));
        }

        assert_eq!(parsed_frames[0].0, 1);
        assert_eq!(parsed_frames[0].1, "started");
        assert_eq!(parsed_frames[0].2["event"], "started");
        assert_eq!(parsed_frames[0].2["totalItems"], 2);

        assert_eq!(parsed_frames[1].0, 2);
        assert_eq!(parsed_frames[1].1, "batch");
        assert_eq!(parsed_frames[1].2["event"], "batch");
        assert_eq!(parsed_frames[1].2["created"], 2);

        assert_eq!(parsed_frames[2].0, 3);
        assert_eq!(parsed_frames[2].1, "completed");
        assert_eq!(parsed_frames[2].2["event"], "completed");
        assert_eq!(parsed_frames[2].2["totalCreated"], 2);

        let guard = db_state.lock().await;
        assert_eq!(guard.tables.get("vehicleresource").unwrap().len(), 2);
    }

    #[test]
    fn test_tqms_search_broadens_by_canonical_vehicle_number_and_history_without_mutating_data() {
        let mut cache = Cache::new();
        cache.write(
            VehicleResource,
            7001,
            make_cached_vehicle(7001, 41523, "Province-4-15-23-KHA-5555", "Gandaki Owner"),
        );
        cache.write(
            VehicleResource,
            7002,
            make_cached_vehicle(7002, 30123, "Province-3-01-23-KHA-5555", "Bagmati Owner"),
        );

        let before = json!({
            "VehicleId": 30123,
            "VehicleNumber": "BA-23-KHA-5555",
            "Registration": { "Status": "ACTIVE" },
            "Owners": [{ "Name": "Bagmati Owner" }]
        });
        let after = json!({
            "VehicleId": 30123,
            "VehicleNumber": "Province-3-01-23-KHA-5555",
            "Registration": { "Status": "ACTIVE" },
            "Owners": [{ "Name": "Bagmati Owner" }]
        });
        let mut chain =
            crate::vehicle_history::VehicleHistoryChain::new("VHIST-TQMS-7002".to_string());
        chain.prepend_node(
            crate::vehicle_history::VehicleChangeType::Create,
            Some(7002),
            None,
            Some(&before),
            None,
            None,
        );
        chain.prepend_node(
            crate::vehicle_history::VehicleChangeType::VehicleNumberChange,
            Some(7002),
            Some(&before),
            Some(&after),
            None,
            None,
        );
        cache.write(
            crate::resource::ResourceName::VehicleHistoryResource,
            1,
            DatabaseResource {
                id: Some(1),
                resource_name: Some(crate::resource::ResourceName::VehicleHistoryResource),
                inner_content: Some(chain.to_json_value()),
                external_content: None,
            },
        );

        // 1. Canonical alias query finds cached "Province-4-15-23-KHA-5555" and cross-matches TQMS alias
        let params_alias = VehicleSearchParameters {
            page: Some(1),
            page_size: Some(20),
            vehicle_number: Some("Ga Pra 15-23-KHA-5555".to_string()),
            owner_name: None,
            sangh_branch: None,
            status: None,
        };
        let tqms_response = json!({
            "Code": 200,
            "Message": "Success",
            "Data": {
                "Data": [{
                    "VehicleId": 41523,
                    "VehicleNumber": "P4-15-23-KHA-5555",
                    "Registration": { "Status": "ACTIVE" },
                    "Owners": [{ "Name": "Gandaki Owner" }]
                }],
                "Paging": { "Page": 1, "TotalItems": 1 }
            }
        });
        let merged = merge_and_annotate_vehicle_results(&cache, &params_alias, Ok(tqms_response))
            .expect("expected merged alias search");
        let cache_items = merged.pointer("/Data/cache_results").and_then(Value::as_array).unwrap();
        let tqms_items = merged.pointer("/Data/tqms_results").and_then(Value::as_array).unwrap();
        assert_eq!(cache_items.len(), 1);
        assert_eq!(cache_items[0]["cache_id"], 7001);
        assert_eq!(cache_items[0]["VehicleNumber"], "Province-4-15-23-KHA-5555");
        assert_eq!(cache_items[0]["is_in_tqms"], true);
        assert_eq!(tqms_items.len(), 1);
        assert_eq!(tqms_items[0]["is_in_billing"], true);

        // 2. Historical plate query finds vehicle 7002 via VehicleHistoryResource, respecting non-number filters
        let params_hist = VehicleSearchParameters {
            page: Some(1),
            page_size: Some(20),
            vehicle_number: Some("BA-23-KHA-5555".to_string()),
            owner_name: Some("Bagmati".to_string()),
            sangh_branch: None,
            status: Some(Status::ACTIVE),
        };
        let merged_hist = merge_and_annotate_vehicle_results(
            &cache,
            &params_hist,
            Err(anyhow::anyhow!("TQMS offline")),
        )
        .expect("historical plate match should succeed");
        let hist_hits = merged_hist
            .pointer("/Data/cache_results")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(hist_hits.len(), 1);
        assert_eq!(hist_hits[0]["cache_id"], 7002);
        assert_eq!(hist_hits[0]["VehicleNumber"], "Province-3-01-23-KHA-5555");

        // 3. Non-matching non-number filter still filters out historical hit
        let params_wrong_owner = VehicleSearchParameters {
            page: Some(1),
            page_size: Some(20),
            vehicle_number: Some("BA-23-KHA-5555".to_string()),
            owner_name: Some("Someone Else".to_string()),
            sangh_branch: None,
            status: None,
        };
        assert!(merge_and_annotate_vehicle_results(
            &cache,
            &params_wrong_owner,
            Err(anyhow::anyhow!("TQMS offline")),
        )
        .is_err());
    }

    #[actix_web::test]
    async fn test_get_vehicle_documents_route_fetches_from_tqms() {
        use crate::access::test_helpers::{create_mock_server_data_with_state, MockDbState};
        use actix_web::{http::StatusCode, test, App};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock TQMS listener");
        let addr = listener.local_addr().unwrap();

        let expected_body = json!({
            "Code": 200,
            "Message": "Success",
            "Data": [
                { "DocumentType": "BlueBook", "DocumentUrl": "https://tqms.example/doc/1.pdf" }
            ]
        });
        let expected_body_str = expected_body.to_string();

        let server_task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept request");
            let mut buf = vec![0u8; 4096];
            let n = socket.read(&mut buf).await.expect("read request");
            let req_text = String::from_utf8_lossy(&buf[..n]).to_string();

            assert!(
                req_text.starts_with("GET /api/v1/cts/vehicle/NA-5-KHA-1234/documents "),
                "unexpected request line: {req_text}"
            );
            assert!(
                req_text.contains("authorization: Bearer test-tqms-token")
                    || req_text.contains("Authorization: Bearer test-tqms-token"),
                "missing bearer token in request: {req_text}"
            );

            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                expected_body_str.len(),
                expected_body_str
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write response");
        });

        let (base_data, _db_state) = create_mock_server_data_with_state(MockDbState::default()).await;
        let custom_state = web::Data::new(Arc::new(ServerData {
            db: base_data.db.clone(),
            tqms_client: Arc::new(TQMSClient::with_base_url_and_token(
                format!("http://{addr}"),
                "test-tqms-token".to_string(),
            )),
            cache: base_data.cache.clone(),
        }));

        let app = test::init_service(
            App::new()
                .app_data(custom_state)
                .service(
                    web::scope("/api/v1/tqms").route(
                        "/vehicle_documents/{vehicle_number}",
                        web::get().to(get_vehicle_documents),
                    ),
                ),
        )
        .await;

        let req = test::TestRequest::get()
            .uri("/api/v1/tqms/vehicle_documents/NA-5-KHA-1234")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body: Value = test::read_body_json(resp).await;
        assert_eq!(body, expected_body);

        server_task.await.expect("mock TQMS server finished");
    }
}
