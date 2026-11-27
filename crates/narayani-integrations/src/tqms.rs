use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub enum Status {
    ACTIVE,
    SUSPENDED,
    DEREGISTERED,
}

impl Status {
    pub const fn as_str(&self) -> &'static str {
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

#[allow(dead_code)]
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

    pub fn with_base_url_and_token(base_url: String, token: String) -> Self {
        let http = Client::builder()
            .pool_max_idle_per_host(100)
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();

        Self {
            http,
            base_url: Some(base_url.trim_end_matches('/').to_string()),
            auth: Arc::new(RwLock::new(TokenState {
                token: Some(token),
                refreshed_at: Some(Instant::now()),
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
    pub async fn ensure_token(&self) -> Result<String> {
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
