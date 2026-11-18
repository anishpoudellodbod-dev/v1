use crate::resource::{DatabaseResource, ResourceName::VehicleResource};
use crate::ServerData;
use actix_web::{web, HttpResponse};
use anyhow::{bail, Context, Result};
use futures::{stream, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub enum Status {
    ACTIVE,
    SUSPENDED,
    DEREGISTERED,
}

impl Status {
    const fn as_str(&self) -> &'static str {
        match self {
            Self::ACTIVE => "ACTIVE",
            Self::SUSPENDED => "SUSPENDED",
            Self::DEREGISTERED => "DEREGISTERED",
        }
    }
}

// Sample of search url /api/v1/cts/vehicle/search?page=1&pageSize=20&vehicleNumber=&ownerName=&sanghBranch=&status=
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VehicleSearchParameters {
    pub page: Option<u32>,
    pub page_size: Option<u32>,
    pub vehicle_number: Option<String>,
    pub owner_name: Option<String>,
    pub sangh_branch: Option<String>,
    pub status: Option<Status>,
}

impl VehicleSearchParameters {
    pub fn is_empty(&self) -> bool {
        self.vehicle_number.is_none()
            && self.owner_name.is_none()
            && self.sangh_branch.is_none()
            && self.status.is_none()
    }
}

#[derive(Clone, Default)]
struct TokenState {
    token: Option<String>,
    refreshed_at: Option<Instant>,
}

#[derive(Clone)]
pub struct TQMSClient {
    http: Client,
    base_url: Option<String>,
    auth: Arc<RwLock<TokenState>>,
}

pub async fn check_validity(tqms_client: TQMSClient) -> Result<TQMSClient> {
    let _ = tqms_client.ensure_token().await?;
    Ok(tqms_client)
}

async fn fetch_token(http: &Client, base_url: &str) -> Result<String> {
    let url = format!("{}/connect/token", base_url.trim_end_matches('/'));
    let res = http
        .post(&url)
        .form(&[
            ("client_id", "cts_client"),
            ("client_secret", "cts_client_secret_tqms"),
            ("grant_type", "client_credentials"),
        ])
        .send()
        .await
        .context("Failed to send token request to TQMS")?;

    let status = res.status();
    if !status.is_success() {
        bail!("Failed to get token from TQMS: HTTP {status}");
    }

    let json: Value = res
        .json()
        .await
        .context("Failed to parse TQMS token JSON")?;

    json["access_token"]
        .as_str()
        .map( str::to_owned)
        .context("Missing `access_token` in TQMS token response")
}

impl TQMSClient {
    pub async fn new() -> Self {
        let base_url = std::env::var("TQMS_BASE_URL")
            .ok()
            .map(|u| u.trim_end_matches('/').to_string());

        let http = Client::builder()
            .pool_max_idle_per_host(100)
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();

        let token = match base_url.as_deref() {
            Some(url) => fetch_token(&http, url).await.ok(),
            None => None,
        };

        let refreshed_at = token.as_ref().map(|_| Instant::now());

        Self {
            http,
            base_url,
            auth: Arc::new(RwLock::new(TokenState {
                token,
                refreshed_at,
            })),
        }
    }

    fn base_url(&self) -> Result<&str> {
        self.base_url
            .as_deref()
            .context("TQMS_BASE_URL environment variable is not set")
    }

    /// Thread-safe token validation and refresh. Prevents expired token bugs
    /// across clones and avoids stampeding token requests during concurrent calls.
    async fn ensure_token(&self) -> Result<String> {
        {
            let state = self.auth.read().await;
            if let (Some(token), Some(ts)) = (&state.token, state.refreshed_at) {
                if !token.is_empty() && ts.elapsed() <= Duration::from_secs(600) {
                    return Ok(token.clone());
                }
            }
        }

        let mut state = self.auth.write().await;
        // Double-check in case another task refreshed while waiting for the write lock
        if let (Some(token), Some(ts)) = (&state.token, state.refreshed_at) {
            if !token.is_empty() && ts.elapsed() <= Duration::from_secs(600) {
                return Ok(token.clone());
            }
        }

        let base_url = self.base_url()?;
        let new_token = fetch_token(&self.http, base_url).await?;
        state.token = Some(new_token.clone());
        state.refreshed_at = Some(Instant::now());

        Ok(new_token)
    }

    /// Unified authenticated GET helper eliminating boilerplate and server-crashing panics.
    async fn get_json(&self, endpoint: &str, query: Option<&[(&str, String)]>) -> Result<Value> {
        let token = self.ensure_token().await?;
        let base_url = self.base_url()?;
        let url = format!("{}/{}", base_url, endpoint.trim_start_matches('/'));

        let mut req = self.http.get(&url).bearer_auth(token);
        if let Some(q) = query {
            req = req.query(q);
        }

        let res = req.send().await?;
        let status = res.status();
        if !status.is_success() {
            bail!("TQMS request to {url} failed with HTTP status: {status}");
        }

        res.json().await.context("Failed to parse TQMS JSON response")
    }

    pub async fn get_vehicle_details(&self, vehicle_number: &str) -> Result<Value> {
        let path = format!("api/v1/cts/vehicle/{vehicle_number}");
        self.get_json(&path, None).await
    }

    pub async fn get_vehicle_documents(&self, vehicle_number: &str) -> Result<Value> {
        let path = format!("api/v1/cts/vehicle/{vehicle_number}/documents");
        self.get_json(&path, None).await
    }

    pub async fn validate_vehicles(&self, vehicle_number: &str) -> Result<Value> {
        let path = format!("api/v1/cts/vehicle/{vehicle_number}/validate");
        self.get_json(&path, None).await
    }

    pub async fn search_vehicles(
        &self,
        params: VehicleSearchParameters,
    ) -> Result<Value> {
        // Properly URL-encodes parameters (spaces, Nepali script, symbols, etc.)
        let query = [
            ("page", params.page.unwrap_or(1).to_string()),
            ("pageSize", params.page_size.unwrap_or(20).to_string()),
            ("vehicleNumber", params.vehicle_number.unwrap_or_default()),
            ("ownerName", params.owner_name.unwrap_or_default()),
            ("sanghBranch", params.sangh_branch.unwrap_or_default()),
            (
                "status",
                params
                    .status
                    .map(|s| s.as_str().to_owned())
                    .unwrap_or_default(),
            ),
        ];

        self.get_json("api/v1/cts/vehicle/search", Some(&query))
            .await
    }
}

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

fn extract_vehicle_norm_num(inner: &Value) -> String {
    inner
        .get("VehicleNumber")
        .or_else(|| inner.get("vehicleNumber"))
        .or_else(|| inner.get("NormalizedVehicleNumber"))
        .or_else(|| inner.get("normalizedVehicleNumber"))
        .and_then(Value::as_str)
        .map(normalize_vehicle_num)
        .unwrap_or_default()
}

fn matches_vehicle_number_field(val: Option<&Value>, q_raw: &str, q_norm: &str) -> bool {
    let Some(s) = val.and_then(Value::as_str) else {
        return false;
    };
    if contains_case_insensitive(s, q_raw) {
        return true;
    }
    !q_norm.is_empty() && normalize_vehicle_num(s).contains(q_norm)
}

fn vehicle_matches_search_params(
    inner: &Value,
    params: &VehicleSearchParameters,
    query_norm_num: &str,
) -> bool {
    if let Some(q_raw) = params
        .vehicle_number
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let num_match = matches_vehicle_number_field(inner.get("VehicleNumber"), q_raw, query_norm_num)
            || matches_vehicle_number_field(
                inner.get("NormalizedVehicleNumber"),
                q_raw,
                query_norm_num,
            );
        let id_match = extract_vehicle_id_str(inner).is_some_and(|id| id == q_raw);
        if !num_match && !id_match {
            return false;
        }
    }

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
    for item in tqms_items {
        if let Some(vid) = extract_vehicle_id_str(item) {
            target_vehicle_ids.insert(vid);
        }
    }

    // Pass 1: search cache directly with the provided search parameters
    for res in &cached_vehicles {
        let Some(inner) = &res.inner_content else {
            continue;
        };
        if vehicle_matches_search_params(inner, params, &query_norm_num) {
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
            if !target_vehicle_ids.contains(&vid) {
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
        id_match || num_match
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
/// (matched by `VehicleId` or normalized `VehicleNumber`). Vehicles present in DB
/// but missing from cache are synced into cache; vehicles absent from both DB and cache
/// are created as new `VehicleResource` records in DB and written into cache.
pub async fn ensure_tqms_vehicles_in_cache_and_db(
    s_data: &ServerData,
    tqms_items: &[Value],
) -> Vec<DatabaseResource> {
    if tqms_items.is_empty() {
        return Vec::new();
    }

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
                if find_matching_vehicle_resource(cached.iter().copied(), vid.as_deref(), &norm_num)
                    .is_some()
                {
                    None
                } else {
                    Some(clean_tqms_vehicle_value(item))
                }
            })
            .collect()
    };

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

        if let Some(existing_db_res) = find_matching_vehicle_resource(
            known_db_vehicles.iter(),
            vid.as_deref(),
            &norm_num,
        )
        .cloned()
        {
            if let Some(db_id) = existing_db_res.id {
                s_data
                    .cache
                    .write()
                    .await
                    .write(VehicleResource, db_id, existing_db_res);
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
    let mut tqms_norms: HashSet<String> = HashSet::with_capacity(tqms_items.len());
    for item in &tqms_items {
        if let Some(vid) = extract_vehicle_id_str(item) {
            tqms_ids.insert(vid);
        }
        let norm = extract_vehicle_norm_num(item);
        if !norm.is_empty() {
            tqms_norms.insert(norm);
        }
    }

    let mut cache_ids: HashSet<String> = HashSet::with_capacity(raw_cache_hits.len());
    let mut cache_norms: HashSet<String> = HashSet::with_capacity(raw_cache_hits.len());
    for (_, item) in &raw_cache_hits {
        if let Some(vid) = extract_vehicle_id_str(item) {
            cache_ids.insert(vid);
        }
        let norm = extract_vehicle_norm_num(item);
        if !norm.is_empty() {
            cache_norms.insert(norm);
        }
    }

    let mut cache_results: Vec<Value> = Vec::with_capacity(raw_cache_hits.len());
    for (cache_id, mut item) in raw_cache_hits {
        let in_tqms = extract_vehicle_id_str(&item).is_some_and(|vid| tqms_ids.contains(&vid))
            || {
                let norm = extract_vehicle_norm_num(&item);
                !norm.is_empty() && tqms_norms.contains(&norm)
            };
        insert_vehicle_flags(&mut item, "cache", true, in_tqms, cache_id);
        cache_results.push(item);
    }

    let mut tqms_results: Vec<Value> = Vec::with_capacity(tqms_items.len());
    for mut item in tqms_items {
        let in_billing = extract_vehicle_id_str(&item).is_some_and(|vid| cache_ids.contains(&vid))
            || {
                let norm = extract_vehicle_norm_num(&item);
                !norm.is_empty() && cache_norms.contains(&norm)
            };
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
    let mut last_error = None;

    for attempt in 1..=max_attempts {
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

        if attempt < max_attempts {
            let delay_ms = 500_u64 * 2_u64.pow(attempt - 1);
            /*eprintln!(
                "Retrying TQMS page {} after attempt {}/{} in {} ms: {}",
                page,
                attempt,
                max_attempts,
                delay_ms,
                last_error.as_deref().unwrap_or("unknown error")
            );*/
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }
    }

    bail!(
        "Failed to fetch TQMS page {} after {} attempts: {}",
        page,
        max_attempts,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::Cache;

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
                InsertVehicle,
            }

            let mut current_query = QueryKind::InsertVehicle;
            let mut pending_insert: (Option<Value>, Option<Value>) = (None, None);
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
                        let sql = String::from_utf8_lossy(&body);
                        current_query = if sql.contains("INSERT INTO") {
                            QueryKind::InsertVehicle
                        } else {
                            QueryKind::SelectVehicles
                        };
                        resp_buf.extend_from_slice(&[b'1', 0, 0, 0, 4]);
                    }
                    b'D' => {
                        if body.first() == Some(&b'S') {
                            match current_query {
                                QueryKind::SelectVehicles => {
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
                                QueryKind::InsertVehicle => {
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
}