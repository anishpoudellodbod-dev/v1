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
#[derive(Debug, Serialize, Deserialize)]
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

pub async fn search_vehicles(
    vehicle_search_parameters: web::Json<VehicleSearchParameters>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let params = vehicle_search_parameters.into_inner();
    if params.is_empty() {
        return HttpResponse::BadRequest().body("At least one search parameter must be provided");
    }

    //Search in vehicle cache first and if found send both cache and tqms result, if not found search in tqms and send result and insert a flag that is in billing but not in tqms


    match s_data.tqms_client.search_vehicles(params).await {
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
        Ok(res) => HttpResponse::Ok().json(res),
        Err(e) => HttpResponse::InternalServerError()
            .body(format!("Failed to get vehicle details: {e:?}")),
    }
}