use crate::cache::Cache;
use crate::resource::{DatabaseResource, ResourceName};
use crate::ServerData;
use actix_web::{web, HttpResponse};
use anyhow::{anyhow, bail, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::env;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

pub const DEFAULT_SMS_BASE_URL: &str = "https://samayasms.com.np/smsapi/index.php";
pub const DEFAULT_SMS_API_KEY: &str = "366979AD75A35B";
pub const DEFAULT_SMS_CAMPAIGN_ID: &str = "8572";
pub const DEFAULT_SMS_ROUTE_ID: &str = "10244";
pub const MAX_SMS_CHAR_LEN: usize = 720;
/// SamayaSMS rate limit: 2 requests per second per user (500ms minimum spacing).
const MIN_REQUEST_INTERVAL: Duration = Duration::from_millis(500);

pub const PAID_TEMPLATE: &str =
    "तपाईंको सवारी नम्बर {vehicle_number} को रकम रु {amount} प्राप्त भएको छ। धन्यवाद। {sangh_branch} म्याद सकिने मिति: {expiry_bs}";

pub const PAYMENT_REMAINDER_TEMPLATE: &str =
    "नमस्ते तपाईंको सवारी नम्बर {vehicle_number} को मासिक शुल्कको म्याद सकिएकोले {due_day_bs} गते भित्रमा मासिक नवीकरण गर्नुहोस्। {sangh_branch}";

pub const URGENT_TEMPLATE: &str =
    "नमस्ते तपाईंको सवारी नम्बर {vehicle_number} को मासिक शुल्कको म्याद सकिएकोले {due_day_bs} गते भित्रमा मासिक नवीकरण गर्नुहोस्। {sangh_branch}";

#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SmsType {
    #[serde(alias = "paid", alias = "Paid")]
    PAID,
    #[serde(
        alias = "PAYMENT_REMINDER",
        alias = "payment_remainder",
        alias = "payment_reminder"
    )]
    PAYMENT_REMAINDER,
    #[serde(alias = "urgent", alias = "Urgent")]
    URGENT,
}

impl SmsType {
    pub const fn default_format(&self) -> &'static str {
        match self {
            Self::PAID => PAID_TEMPLATE,
            Self::PAYMENT_REMAINDER => PAYMENT_REMAINDER_TEMPLATE,
            Self::URGENT => URGENT_TEMPLATE,
        }
    }

    pub fn setting_template_ids(&self) -> &'static [&'static str] {
        match self {
            Self::PAID => &["payment-received", "paid", "payment_received"],
            Self::PAYMENT_REMAINDER => &[
                "payment-reminder",
                "payment-remainder",
                "payment_reminder",
                "payment_remainder",
            ],
            Self::URGENT => &["urgent-reminder", "urgent", "urgent_reminder"],
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SettingSmsTemplate {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default = "default_true")]
    pub active: bool,
    #[serde(default)]
    pub channel: Option<String>,
    pub content: String,
    #[serde(default)]
    pub category: Option<String>,
}

/// Finds the matching SMS template inside a collection of `ResourceName::SettingResource` records
/// (under `inner_content.smsSettings.templates`). Prefers active templates and newer resource IDs
/// while falling back to inactive matches if no active template matches.
pub fn find_sms_template_in_settings<'a, I>(
    settings: I,
    sms_type: SmsType,
    template_id_override: Option<&str>,
) -> Option<SettingSmsTemplate>
where
    I: IntoIterator<Item = &'a DatabaseResource>,
{
    let target_override = template_id_override
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let type_ids = sms_type.setting_template_ids();

    let mut best: Option<(bool, i32, SettingSmsTemplate)> = None;

    for res in settings {
        let Some(inner) = res.inner_content.as_ref() else {
            continue;
        };
        let Some(templates) = inner
            .get("smsSettings")
            .or_else(|| inner.get("sms_settings"))
            .and_then(|s| s.get("templates"))
            .and_then(Value::as_array)
        else {
            continue;
        };

        let res_id = res.id.unwrap_or(0);
        for raw_tpl in templates {
            let Ok(tpl) = serde_json::from_value::<SettingSmsTemplate>(raw_tpl.clone()) else {
                continue;
            };
            if tpl.content.trim().is_empty() {
                continue;
            }
            if let Some(ch) = tpl.channel.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                if !ch.eq_ignore_ascii_case("SMS") {
                    continue;
                }
            }

            let tpl_id = tpl.id.trim();
            let matches = match target_override {
                Some(override_id) => tpl_id.eq_ignore_ascii_case(override_id),
                None => type_ids.iter().any(|candidate| tpl_id.eq_ignore_ascii_case(candidate)),
            };
            if !matches {
                continue;
            }

            let should_replace = match &best {
                None => true,
                Some((best_active, best_id, _)) => (tpl.active, res_id) > (*best_active, *best_id),
            };
            if should_replace {
                best = Some((tpl.active, res_id, tpl));
            }
        }
    }

    best.map(|(_, _, tpl)| tpl)
}

/// Looks up the matching SMS template in `Cache` under `ResourceName::SettingResource`.
pub fn find_sms_template_in_cache(
    cache: &Cache,
    sms_type: SmsType,
    template_id_override: Option<&str>,
) -> Option<SettingSmsTemplate> {
    let settings = cache.read_all(ResourceName::SettingResource).ok()?;
    find_sms_template_in_settings(settings, sms_type, template_id_override)
}

/// Looks up the matching SMS template from `s_data.cache` (`ResourceName::SettingResource`),
/// falling back to `s_data.db.read_all_resource(ResourceName::SettingResource)` when cache misses.
pub async fn find_sms_template_in_server_data(
    s_data: &ServerData,
    sms_type: SmsType,
    template_id_override: Option<&str>,
) -> Option<SettingSmsTemplate> {
    {
        let cache = s_data.cache.read().await;
        if let Some(tpl) = find_sms_template_in_cache(&cache, sms_type, template_id_override) {
            return Some(tpl);
        }
    }

    let db_settings = s_data
        .db
        .read_all_resource(ResourceName::SettingResource)
        .await
        .ok()?;
    find_sms_template_in_settings(db_settings.iter(), sms_type, template_id_override)
}

/// Allows `receiver` in JSON to be supplied either as a string (`"9841XXXXXX"` or
/// `"9841XXXXXX,9801XXXXXX"`) or as a JSON integer (`9841123456`).
fn deserialize_receiver<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ReceiverValue {
        Str(String),
        Num(u64),
    }

    match ReceiverValue::deserialize(deserializer)? {
        ReceiverValue::Str(s) => Ok(s),
        ReceiverValue::Num(n) => Ok(n.to_string()),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmsTemplate {
    #[serde(deserialize_with = "deserialize_receiver")]
    pub receiver: String,
    pub sms_type: SmsType,
    #[serde(default)]
    pub template_id: Option<String>,
    #[serde(default)]
    pub sms_format: Option<String>,
    pub vehicle_number: String,
    pub sangh_branch: String,
    #[serde(default)]
    pub expiry_bs: Option<String>,
    #[serde(default, alias = "due_day_bs")]
    pub due_date_bs: Option<String>,
    #[serde(default)]
    pub amount: Option<f64>,
    /// Optional scheduled delivery time in `YYYY-MM-DD HH:MM` format.
    #[serde(default, alias = "time")]
    pub scheduled_time: Option<String>,
}

impl SmsTemplate {
    pub fn new_paid(
        receiver: impl Into<String>,
        vehicle_number: impl Into<String>,
        sangh_branch: impl Into<String>,
        amount: f64,
        expiry_bs: impl Into<String>,
    ) -> Self {
        Self {
            receiver: receiver.into(),
            sms_type: SmsType::PAID,
            template_id: None,
            sms_format: None,
            vehicle_number: vehicle_number.into(),
            sangh_branch: sangh_branch.into(),
            expiry_bs: Some(expiry_bs.into()),
            due_date_bs: None,
            amount: Some(amount),
            scheduled_time: None,
        }
    }

    pub fn new_payment_remainder(
        receiver: impl Into<String>,
        vehicle_number: impl Into<String>,
        sangh_branch: impl Into<String>,
        due_date_bs: impl Into<String>,
    ) -> Self {
        Self {
            receiver: receiver.into(),
            sms_type: SmsType::PAYMENT_REMAINDER,
            template_id: None,
            sms_format: None,
            vehicle_number: vehicle_number.into(),
            sangh_branch: sangh_branch.into(),
            expiry_bs: None,
            due_date_bs: Some(due_date_bs.into()),
            amount: None,
            scheduled_time: None,
        }
    }

    pub fn new_urgent(
        receiver: impl Into<String>,
        vehicle_number: impl Into<String>,
        sangh_branch: impl Into<String>,
        due_date_bs: impl Into<String>,
    ) -> Self {
        Self {
            receiver: receiver.into(),
            sms_type: SmsType::URGENT,
            template_id: None,
            sms_format: None,
            vehicle_number: vehicle_number.into(),
            sangh_branch: sangh_branch.into(),
            expiry_bs: None,
            due_date_bs: Some(due_date_bs.into()),
            amount: None,
            scheduled_time: None,
        }
    }

    pub fn with_template_id(mut self, template_id: impl Into<String>) -> Self {
        self.template_id = Some(template_id.into());
        self
    }

    pub fn with_amount(mut self, amount: f64) -> Self {
        self.amount = Some(amount);
        self
    }

    pub fn with_custom_format(mut self, format: impl Into<String>) -> Self {
        self.sms_format = Some(format.into());
        self
    }

    pub fn with_scheduled_time(mut self, time: impl Into<String>) -> Self {
        self.scheduled_time = Some(time.into());
        self
    }

    /// Normalizes comma-separated recipient phone numbers and validates that at least one is present.
    pub fn normalized_contacts(&self) -> Result<String> {
        normalize_contacts(&self.receiver)
    }

    /// Renders the final SMS text using `sms_format` (if provided and non-empty), otherwise
    /// `format_override` (e.g. from `SettingResource`), otherwise the default format for `sms_type`,
    /// substituting both snake_case and camelCase placeholders and enforcing the 720-character limit.
    pub fn render_with_format(&self, format_override: Option<&str>) -> Result<String> {
        let vehicle_number = self.vehicle_number.trim();
        if vehicle_number.is_empty() {
            bail!("vehicle_number is required");
        }
        let sangh_branch = self.sangh_branch.trim();
        if sangh_branch.is_empty() {
            bail!("sangh_branch is required");
        }

        let template = self
            .sms_format
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or_else(|| format_override.map(str::trim).filter(|s| !s.is_empty()))
            .unwrap_or_else(|| self.sms_type.default_format());

        let mut msg = template
            .replace("{vehicle_number}", vehicle_number)
            .replace("{vehicleNumber}", vehicle_number)
            .replace("{sangh_branch}", sangh_branch)
            .replace("{sanghBranch}", sangh_branch)
            .replace("{organizationName}", sangh_branch)
            .replace("{organization_name}", sangh_branch);

        if msg.contains("{amount}") {
            let amount = self
                .amount
                .ok_or_else(|| anyhow!("amount is required for {:?} SMS template", self.sms_type))?;
            msg = msg.replace("{amount}", &format_amount(amount));
        }

        let has_expiry_placeholder = msg.contains("{expiry_bs}")
            || msg.contains("{expiryBs}")
            || msg.contains("{expiryDate}")
            || msg.contains("{expiry_date}");
        if has_expiry_placeholder {
            let expiry_bs = self
                .expiry_bs
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    anyhow!("expiry_bs is required for {:?} SMS template", self.sms_type)
                })?;
            msg = msg
                .replace("{expiry_bs}", expiry_bs)
                .replace("{expiryBs}", expiry_bs)
                .replace("{expiryDate}", expiry_bs)
                .replace("{expiry_date}", expiry_bs);
        }

        let has_due_placeholder = msg.contains("{due_day_bs}")
            || msg.contains("{due_date_bs}")
            || msg.contains("{dueDayBs}")
            || msg.contains("{dueDate}")
            || msg.contains("{due_date}");
        if has_due_placeholder {
            let due = self
                .due_date_bs
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    anyhow!(
                        "due_date_bs is required for {:?} SMS template",
                        self.sms_type
                    )
                })?;
            msg = msg
                .replace("{due_day_bs}", due)
                .replace("{due_date_bs}", due)
                .replace("{dueDayBs}", due)
                .replace("{dueDate}", due)
                .replace("{due_date}", due);
        }

        let char_count = msg.chars().count();
        if char_count == 0 {
            bail!("rendered SMS message is empty");
        }
        if char_count > MAX_SMS_CHAR_LEN {
            bail!(
                "rendered SMS message exceeds maximum length of {MAX_SMS_CHAR_LEN} characters (got {char_count})"
            );
        }

        Ok(msg)
    }

    /// Renders the final SMS text using `sms_format` (if provided and non-empty) or the default
    /// format for `sms_type`, substituting all placeholders and validating required fields and
    /// the 720-character API limit.
    pub fn render(&self) -> Result<String> {
        self.render_with_format(None)
    }

    /// Renders the template using formats configured inside `SettingResource` items.
    pub fn render_with_settings<'a, I>(&self, settings: I) -> Result<String>
    where
        I: IntoIterator<Item = &'a DatabaseResource>,
    {
        let setting_tpl =
            find_sms_template_in_settings(settings, self.sms_type, self.template_id.as_deref());
        self.render_with_format(setting_tpl.as_ref().map(|t| t.content.as_str()))
    }

    /// Renders the template using `ResourceName::SettingResource` entries stored in `Cache`.
    pub fn render_with_cache(&self, cache: &Cache) -> Result<String> {
        let setting_tpl =
            find_sms_template_in_cache(cache, self.sms_type, self.template_id.as_deref());
        self.render_with_format(setting_tpl.as_ref().map(|t| t.content.as_str()))
    }
}

fn format_amount(amount: f64) -> String {
    if amount.fract() == 0.0 {
        format!("{}", amount as i64)
    } else {
        format!("{:.2}", amount)
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    }
}

pub fn normalize_contacts(raw: &str) -> Result<String> {
    let cleaned: Vec<String> = raw
        .split(',')
        .map(|part| {
            part.trim()
                .trim_start_matches('+')
                .chars()
                .filter(|c| c.is_ascii_digit())
                .collect::<String>()
        })
        .filter(|num| !num.is_empty())
        .collect();

    if cleaned.is_empty() {
        bail!("at least one valid receiver contact number is required");
    }

    Ok(cleaned.join(","))
}

#[derive(Debug, Clone)]
pub struct SmsConfig {
    pub base_url: String,
    pub api_key: String,
    pub campaign_id: Option<String>,
    pub route_id: Option<String>,
    pub sender_id: String,
    pub response_type: Option<String>,
    /// Optional explicit override for SamayaSMS `type` parameter (`"text"` or `"unicode"`).
    /// When `None`, automatically uses `"unicode"` for non-ASCII text (e.g. Nepali) and `"text"` for ASCII.
    pub sms_api_type: Option<String>,
}

impl SmsConfig {
    pub fn from_env() -> Self {
        let base_url = env::var("SMS_API_BASE_URL")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| DEFAULT_SMS_BASE_URL.to_string());

        let api_key = env::var("SMS_API_KEY")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| DEFAULT_SMS_API_KEY.to_string());

        let campaign_id = env::var("SMS_CAMPAIGN_ID")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .or_else(|| Some(DEFAULT_SMS_CAMPAIGN_ID.to_string()));

        let route_id = env::var("SMS_ROUTE_ID")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .or_else(|| Some(DEFAULT_SMS_ROUTE_ID.to_string()));

        let sender_id = env::var("SMS_SENDER_ID")
            .ok()
            .map(|v| v.trim().to_string())
            .unwrap_or_default();

        let response_type = env::var("SMS_RESPONSE_TYPE")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());

        let sms_api_type = env::var("SMS_API_TYPE")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());

        Self {
            base_url,
            api_key,
            campaign_id,
            route_id,
            sender_id,
            response_type,
            sms_api_type,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SmsSendResult {
    pub shoot_id: String,
    pub contacts: String,
    pub message: String,
    pub raw_response: String,
}

#[derive(Clone)]
pub struct SmsClient {
    http: Client,
    config: SmsConfig,
    last_sent_at: Arc<Mutex<Option<Instant>>>,
}

impl SmsClient {
    pub fn from_env() -> Self {
        Self::new(SmsConfig::from_env())
    }

    pub fn new(config: SmsConfig) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();

        Self {
            http,
            config,
            last_sent_at: Arc::new(Mutex::new(None)),
        }
    }

    /// Enforces the SamayaSMS 2 req/sec rate limit across concurrent callers.
    async fn wait_for_rate_limit(&self) {
        let mut last = self.last_sent_at.lock().await;
        if let Some(prev) = *last {
            let elapsed = prev.elapsed();
            if elapsed < MIN_REQUEST_INTERVAL {
                tokio::time::sleep(MIN_REQUEST_INTERVAL - elapsed).await;
            }
        }
        *last = Some(Instant::now());
    }

    fn build_form_params(
        &self,
        contacts: &str,
        message: &str,
        scheduled_time: Option<&str>,
    ) -> Vec<(&'static str, String)> {
        let sms_type = self
            .config
            .sms_api_type
            .clone()
            .unwrap_or_else(|| {
                if message.is_ascii() {
                    "text".to_string()
                } else {
                    "unicode".to_string()
                }
            });

        let mut params: Vec<(&'static str, String)> = Vec::with_capacity(9);
        params.push(("key", self.config.api_key.clone()));
        if let Some(ref campaign) = self.config.campaign_id {
            params.push(("campaign", campaign.clone()));
        }
        if let Some(ref route_id) = self.config.route_id {
            params.push(("routeid", route_id.clone()));
        }
        params.push(("type", sms_type));
        if let Some(ref response_type) = self.config.response_type {
            params.push(("responsetype", response_type.clone()));
        }
        params.push(("contacts", contacts.to_string()));
        params.push(("senderid", self.config.sender_id.clone()));
        params.push(("msg", message.to_string()));

        if let Some(t) = scheduled_time.map(str::trim).filter(|s| !s.is_empty()) {
            params.push(("time", t.to_string()));
        }

        params
    }

    /// Renders and sends an `SmsTemplate` through the SamayaSMS HTTP POST API.
    pub async fn send_template(&self, template: &SmsTemplate) -> Result<SmsSendResult> {
        let contacts = template.normalized_contacts()?;
        let message = template.render()?;
        self.send_raw(&contacts, &message, template.scheduled_time.as_deref())
            .await
    }

    /// Renders an `SmsTemplate` using `ResourceName::SettingResource` stored in `Cache`
    /// and sends it through the SamayaSMS HTTP POST API.
    pub async fn send_template_with_cache(
        &self,
        template: &SmsTemplate,
        cache: &Cache,
    ) -> Result<SmsSendResult> {
        let contacts = template.normalized_contacts()?;
        let message = template.render_with_cache(cache)?;
        self.send_raw(&contacts, &message, template.scheduled_time.as_deref())
            .await
    }

    /// Renders an `SmsTemplate` using `ResourceName::SettingResource` from `ServerData`
    /// (cache first, falling back to DB) and sends it through the SamayaSMS HTTP POST API.
    pub async fn send_template_with_server_data(
        &self,
        template: &SmsTemplate,
        s_data: &ServerData,
    ) -> Result<SmsSendResult> {
        let contacts = template.normalized_contacts()?;
        let setting_tpl = if template
            .sms_format
            .as_deref()
            .map(str::trim)
            .is_none_or(str::is_empty)
        {
            find_sms_template_in_server_data(
                s_data,
                template.sms_type,
                template.template_id.as_deref(),
            )
            .await
        } else {
            None
        };
        let message =
            template.render_with_format(setting_tpl.as_ref().map(|t| t.content.as_str()))?;
        self.send_raw(&contacts, &message, template.scheduled_time.as_deref())
            .await
    }

    /// Sends a raw SMS message to one or more comma-separated mobile numbers.
    pub async fn send_raw(
        &self,
        contacts: &str,
        message: &str,
        scheduled_time: Option<&str>,
    ) -> Result<SmsSendResult> {
        let normalized_contacts = normalize_contacts(contacts)?;
        let trimmed_msg = message.trim();
        if trimmed_msg.is_empty() {
            bail!("SMS message cannot be empty");
        }
        let char_count = trimmed_msg.chars().count();
        if char_count > MAX_SMS_CHAR_LEN {
            bail!(
                "SMS message exceeds maximum length of {MAX_SMS_CHAR_LEN} characters (got {char_count})"
            );
        }

        let params = self.build_form_params(&normalized_contacts, trimmed_msg, scheduled_time);

        self.wait_for_rate_limit().await;

        let response = self
            .http
            .post(&self.config.base_url)
            .form(&params)
            .send()
            .await
            .context("Failed to send HTTP request to SamayaSMS API")?;

        let status = response.status();
        let body = response
            .text()
            .await
            .context("Failed to read SamayaSMS API response body")?;

        let trimmed_body = body.trim();
        if !status.is_success() {
            bail!("SamayaSMS API returned HTTP {status}: {trimmed_body}");
        }

        let shoot_id = parse_samaya_sms_response(trimmed_body)?;
        Ok(SmsSendResult {
            shoot_id,
            contacts: normalized_contacts,
            message: trimmed_msg.to_string(),
            raw_response: trimmed_body.to_string(),
        })
    }
}

/// Parses SamayaSMS API response text:
/// - `SMS-SHOOT-ID/<id>` -> `Ok(<id>)`
/// - `ERR: <message>` -> `Err(...)`
pub fn parse_samaya_sms_response(body: &str) -> Result<String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        bail!("Empty response received from SamayaSMS API");
    }

    if let Some(err_msg) = trimmed.strip_prefix("ERR:") {
        bail!("SamayaSMS error: {}", err_msg.trim());
    }

    if let Some(shoot_id) = trimmed.strip_prefix("SMS-SHOOT-ID/") {
        let id = shoot_id.trim();
        if id.is_empty() {
            bail!("SamayaSMS returned empty shoot ID: {trimmed}");
        }
        return Ok(id.to_string());
    }

    // Also support JSON response when `responsetype=json` is configured.
    if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(id) = json_val
            .get("shoot_id")
            .or_else(|| json_val.get("SMS-SHOOT-ID"))
            .or_else(|| json_val.get("data"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Ok(id.to_string());
        }
        if let Some(err) = json_val
            .get("error")
            .or_else(|| json_val.get("message"))
            .and_then(|v| v.as_str())
        {
            bail!("SamayaSMS error: {err}");
        }
    }

    bail!("Unexpected SamayaSMS API response: {trimmed}");
}

#[derive(Debug, Deserialize)]
pub struct SendRawSmsRequest {
    #[serde(deserialize_with = "deserialize_receiver")]
    pub contacts: String,
    pub msg: String,
    #[serde(default, alias = "scheduled_time")]
    pub time: Option<String>,
}

/// POST /api/v1/sms/preview — Renders and validates an `SmsTemplate` without sending.
pub async fn preview_sms_template(
    template: web::Json<SmsTemplate>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let tpl = template.into_inner();
    let contacts = match tpl.normalized_contacts() {
        Ok(c) => c,
        Err(e) => return HttpResponse::BadRequest().json(serde_json::json!({ "error": e.to_string() })),
    };

    let has_custom_format = tpl
        .sms_format
        .as_deref()
        .map(str::trim)
        .is_some_and(|s| !s.is_empty());

    let setting_tpl = if has_custom_format {
        None
    } else {
        find_sms_template_in_server_data(&s_data, tpl.sms_type, tpl.template_id.as_deref()).await
    };

    let template_source = if has_custom_format {
        "custom"
    } else if setting_tpl.is_some() {
        "setting_resource"
    } else {
        "default"
    };

    match tpl.render_with_format(setting_tpl.as_ref().map(|t| t.content.as_str())) {
        Ok(message) => HttpResponse::Ok().json(serde_json::json!({
            "contacts": contacts,
            "sms_type": tpl.sms_type,
            "template_id": tpl.template_id.or_else(|| setting_tpl.map(|t| t.id)),
            "template_source": template_source,
            "message": message,
            "char_count": message.chars().count(),
            "scheduled_time": tpl.scheduled_time,
        })),
        Err(e) => HttpResponse::BadRequest().json(serde_json::json!({ "error": e.to_string() })),
    }
}

/// POST /api/v1/sms/send — Renders an `SmsTemplate` (using `SettingResource` templates) and sends it via SamayaSMS.
pub async fn send_template_sms(
    template: web::Json<SmsTemplate>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let tpl = template.into_inner();
    let client = SmsClient::from_env();
    match client.send_template_with_server_data(&tpl, &s_data).await {
        Ok(result) => HttpResponse::Ok().json(result),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("is required") || msg.contains("exceeds maximum length") || msg.contains("empty") {
                HttpResponse::BadRequest().json(serde_json::json!({ "error": msg }))
            } else {
                HttpResponse::BadGateway().json(serde_json::json!({ "error": msg }))
            }
        }
    }
}

/// POST /api/v1/sms/send-raw — Sends a raw SMS message to comma-separated contacts via SamayaSMS.
pub async fn send_raw_sms(req: web::Json<SendRawSmsRequest>) -> HttpResponse {
    let body = req.into_inner();
    let client = SmsClient::from_env();
    match client
        .send_raw(&body.contacts, &body.msg, body.time.as_deref())
        .await
    {
        Ok(result) => HttpResponse::Ok().json(result),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("is required") || msg.contains("exceeds maximum length") || msg.contains("empty") {
                HttpResponse::BadRequest().json(serde_json::json!({ "error": msg }))
            } else {
                HttpResponse::BadGateway().json(serde_json::json!({ "error": msg }))
            }
        }
    }
}

pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.route("/preview", web::post().to(preview_sms_template))
        .route("/send", web::post().to(send_template_sms))
        .route("/send-raw", web::post().to(send_raw_sms));
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn test_render_paid_template() {
        let tpl = SmsTemplate::new_paid(
            "9841000000",
            "BA-01-PA-1234",
            "Main Branch",
            2000.0,
            "2083-08-30",
        );
        let rendered = tpl.render().expect("PAID template should render");
        assert_eq!(
            rendered,
            "तपाईंको सवारी नम्बर BA-01-PA-1234 को रकम रु 2000 प्राप्त भएको छ। धन्यवाद। Main Branch म्याद सकिने मिति: 2083-08-30"
        );
    }

    #[test]
    fn test_render_payment_remainder_and_urgent_templates() {
        let rem = SmsTemplate::new_payment_remainder(
            "9841000000",
            "NA-04-KHA-5050",
            "Hetauda Branch",
            "15",
        );
        assert_eq!(
            rem.render().unwrap(),
            "नमस्ते तपाईंको सवारी नम्बर NA-04-KHA-5050 को मासिक शुल्कको म्याद सकिएकोले 15 गते भित्रमा मासिक नवीकरण गर्नुहोस्। Hetauda Branch"
        );

        let urgent = SmsTemplate::new_urgent(
            "9841000000, 9801000000",
            "NA-04-KHA-5050",
            "Hetauda Branch",
            "20",
        );
        assert_eq!(
            urgent.render().unwrap(),
            "नमस्ते तपाईंको सवारी नम्बर NA-04-KHA-5050 को मासिक शुल्कको म्याद सकिएकोले 20 गते भित्रमा मासिक नवीकरण गर्नुहोस्। Hetauda Branch"
        );
        assert_eq!(
            urgent.normalized_contacts().unwrap(),
            "9841000000,9801000000"
        );
    }

    #[test]
    fn test_template_validation_errors() {
        // Missing amount & expiry_bs for PAID
        let invalid_paid = SmsTemplate {
            receiver: "9841000000".to_string(),
            sms_type: SmsType::PAID,
            template_id: None,
            sms_format: None,
            vehicle_number: "BA-01-PA-1234".to_string(),
            sangh_branch: "Main Branch".to_string(),
            expiry_bs: None,
            due_date_bs: None,
            amount: None,
            scheduled_time: None,
        };
        assert!(invalid_paid.render().is_err());

        // Missing due_date_bs for PAYMENT_REMAINDER
        let invalid_rem = SmsTemplate {
            receiver: "9841000000".to_string(),
            sms_type: SmsType::PAYMENT_REMAINDER,
            template_id: None,
            sms_format: None,
            vehicle_number: "BA-01-PA-1234".to_string(),
            sangh_branch: "Main Branch".to_string(),
            expiry_bs: None,
            due_date_bs: None,
            amount: None,
            scheduled_time: None,
        };
        assert!(invalid_rem.render().is_err());

        // Exceeds 720 chars
        let too_long = SmsTemplate::new_paid(
            "9841000000",
            "V1",
            "Branch",
            100.0,
            "2083-01-01",
        )
        .with_custom_format("a".repeat(MAX_SMS_CHAR_LEN + 1));
        assert!(too_long.render().is_err());
    }

    #[test]
    fn test_json_deserialization_numeric_or_string_receiver() {
        let json_num = r#"{
            "receiver": 9841234567,
            "sms_type": "PAID",
            "vehicle_number": "BA-01-PA-1234",
            "sangh_branch": "Main",
            "amount": 1500,
            "expiry_bs": "2083-09-01"
        }"#;
        let tpl: SmsTemplate = serde_json::from_str(json_num).unwrap();
        assert_eq!(tpl.receiver, "9841234567");
        assert_eq!(tpl.sms_type, SmsType::PAID);
        assert!(tpl.render().is_ok());

        let json_alias = r#"{
            "receiver": "9841234567,9801234567",
            "sms_type": "PAYMENT_REMINDER",
            "vehicle_number": "BA-01-PA-1234",
            "sangh_branch": "Main",
            "due_day_bs": "10"
        }"#;
        let tpl2: SmsTemplate = serde_json::from_str(json_alias).unwrap();
        assert_eq!(tpl2.sms_type, SmsType::PAYMENT_REMAINDER);
        assert_eq!(tpl2.due_date_bs.as_deref(), Some("10"));
    }

    #[test]
    fn test_parse_samaya_sms_response() {
        assert_eq!(
            parse_samaya_sms_response("SMS-SHOOT-ID/ABC123XYZ").unwrap(),
            "ABC123XYZ"
        );
        assert!(parse_samaya_sms_response("ERR: INVALID API KEY").is_err());
        assert!(parse_samaya_sms_response("ERR: Rate limit exceeded. Try again later.").is_err());
        assert!(parse_samaya_sms_response("").is_err());
    }

    async fn spawn_mock_sms_server(
        status_line: &'static str,
        response_body: &'static str,
    ) -> (String, tokio::sync::oneshot::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();

        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = vec![0u8; 8192];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let req_str = String::from_utf8_lossy(&buf[..n]).to_string();
                let _ = tx.send(req_str);

                let resp = format!(
                    "HTTP/1.1 {status_line}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                    response_body.len()
                );
                let _ = socket.write_all(resp.as_bytes()).await;
                let _ = socket.flush().await;
            }
        });

        (format!("http://{addr}/smsapi/index.php"), rx)
    }

    #[actix_web::test]
    async fn test_send_template_via_mock_samaya_sms_api() {
        let (mock_url, req_rx) =
            spawn_mock_sms_server("200 OK", "SMS-SHOOT-ID/SHOOT998877").await;

        let client = SmsClient::new(SmsConfig {
            base_url: mock_url,
            api_key: DEFAULT_SMS_API_KEY.to_string(),
            campaign_id: Some(DEFAULT_SMS_CAMPAIGN_ID.to_string()),
            route_id: Some(DEFAULT_SMS_ROUTE_ID.to_string()),
            sender_id: "NARAYANI".to_string(),
            response_type: None,
            sms_api_type: None,
        });

        let tpl = SmsTemplate::new_paid(
            "9841111111",
            "BA-01-PA-9999",
            "Main Branch",
            2500.0,
            "2083-10-15",
        )
        .with_scheduled_time("2026-11-19 14:30");

        let result = client.send_template(&tpl).await.expect("SMS send should succeed");
        assert_eq!(result.shoot_id, "SHOOT998877");
        assert_eq!(result.contacts, "9841111111");
        assert_eq!(result.raw_response, "SMS-SHOOT-ID/SHOOT998877");

        let raw_http = req_rx.await.unwrap();
        assert!(raw_http.starts_with("POST /smsapi/index.php HTTP/1.1"));
        assert!(raw_http.contains("key=366979AD75A35B"));
        assert!(raw_http.contains("campaign=8572"));
        assert!(raw_http.contains("routeid=10244"));
        assert!(raw_http.contains("type=unicode"));
        assert!(raw_http.contains("contacts=9841111111"));
        assert!(raw_http.contains("senderid=NARAYANI"));
        assert!(raw_http.contains("time=2026-11-19+14%3A30"));
    }

    #[actix_web::test]
    async fn test_send_raw_handles_samaya_error_and_rate_limit_status() {
        let (mock_url, _rx) = spawn_mock_sms_server(
            "429 Too Many Requests",
            "ERR: Rate limit exceeded. Try again later.",
        )
        .await;

        let client = SmsClient::new(SmsConfig {
            base_url: mock_url,
            api_key: DEFAULT_SMS_API_KEY.to_string(),
            campaign_id: Some(DEFAULT_SMS_CAMPAIGN_ID.to_string()),
            route_id: Some(DEFAULT_SMS_ROUTE_ID.to_string()),
            sender_id: "NARAYANI".to_string(),
            response_type: None,
            sms_api_type: None,
        });

        let err = client
            .send_raw("9841111111", "Hello test", None)
            .await
            .expect_err("HTTP 429 should return Err");
        assert!(err.to_string().contains("429"));
        assert!(err.to_string().contains("Rate limit exceeded"));
    }

    #[actix_web::test]
    async fn test_live_send_paid_template_to_lodbod_test_sangh() {
        dotenv::dotenv().ok();

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        let random_num = 1000 + (nanos % 9000);
        let vehicle_number = format!("BA-01-PA-{random_num}");

        let tpl = SmsTemplate::new_paid(
            "9861282176",
            &vehicle_number,
            "lodbod test sangh",
            0.0,
            "2083-12-30",
        );

        let client = SmsClient::from_env();
        let result = client
            .send_template(&tpl)
            .await
            .expect("Live SamayaSMS PAID template request should succeed");

        println!("Live SMS sent: {result:?}");
        assert!(!result.shoot_id.is_empty());
        assert_eq!(result.contacts, "9861282176");
        assert!(result.message.contains(&vehicle_number));
        assert!(result.message.contains("रु 0"));
        assert!(result.message.contains("lodbod test sangh"));
    }

    fn sample_setting_resource() -> DatabaseResource {
        DatabaseResource {
            id: Some(1),
            resource_name: Some(ResourceName::SettingResource),
            inner_content: Some(serde_json::json!({
                "vat": {
                    "rate": 13,
                    "enabled": true,
                    "billingEntityIds": ["1"]
                },
                "arrears": {
                    "monthlyPaid": {
                        "enabled": true,
                        "fiscalYearId": "15f21721-50e8-41af-9596-9d8cdb8d6a16",
                        "billingRuleId": "5"
                    },
                    "paidExpired": {
                        "enabled": true,
                        "billingRuleId": "5"
                    }
                },
                "fiscalYear": {
                    "selector": {
                        "yearsBack": 4,
                        "yearsForward": 1,
                        "archivedYears": []
                    },
                    "activeFiscalYears": []
                },
                "SanghBranch": "Loadboard sthaniya",
                "smsSettings": {
                    "templates": [
                        {
                            "id": "payment-received",
                            "name": "भुक्तानी प्राप्त",
                            "active": true,
                            "channel": "SMS",
                            "content": "तपाईंको सवारी नम्बर {vehicleNumber} को रकम रु {amount} प्राप्त भएको छ।\nधन्यवाद। {organizationName}\nम्याद सकिने मिति: {expiryDate}",
                            "category": "Billing"
                        },
                        {
                            "id": "payment-reminder",
                            "name": "भुक्तानी रिमाइन्डर",
                            "active": true,
                            "channel": "SMS",
                            "content": "नमस्कार, सवारी नम्बर {vehicleNumber} को रु {amount} भुक्तानी बाँकी छ। कृपया {dueDate} भित्र भुक्तानी गर्नुहोस्।\n{organizationName}",
                            "category": "Billing"
                        },
                        {
                            "id": "urgent-reminder",
                            "name": "अत्यावश्यक रिमाइन्डर",
                            "active": false,
                            "channel": "SMS",
                            "content": "अत्यावश्यक सूचना: सवारी नम्बर {vehicleNumber} को भुक्तानी म्याद {dueDate} मा सकिँदैछ। सेवा निरन्तरताका लागि समयमै भुक्तानी गर्नुहोस्।\n{organizationName}",
                            "category": "Billing"
                        }
                    ],
                    "automationRules": [
                        {
                            "id": "rule-bill",
                            "name": "बिल तयार भएपछि",
                            "enabled": true,
                            "template": "बिल तयार भएको सूचना",
                            "description": "नयाँ बिल सफलतापूर्वक तयार हुँदा सम्बन्धित व्यक्तिलाई सूचना पठाउने।"
                        },
                        {
                            "id": "rule-payment",
                            "name": "भुक्तानी प्राप्त भएपछि",
                            "enabled": true,
                            "template": "भुक्तानी प्राप्त",
                            "description": "भुक्तानी प्राप्त भएपछि रसिद वा भुक्तानी पुष्टि सन्देश पठाउने।"
                        },
                        {
                            "id": "rule-overdue",
                            "name": "भुक्तानी म्याद नजिकिँदा",
                            "enabled": false,
                            "template": "भुक्तानी रिमाइन्डर",
                            "description": "भुक्तानीको अन्तिम मिति अघि सम्झना सन्देश पठाउने।"
                        }
                    ]
                },
                "schemaVersion": 1,
                "linkedFeeRules": {
                    "rules": [
                        {
                            "id": "f72c97c7-ddd4-4d35-bbdd-86e4746e57db",
                            "active": true,
                            "fiscalYearId": "15f21721-50e8-41af-9596-9d8cdb8d6a16",
                            "linkedFeeHeadId": "21",
                            "sourceFeeHeadId": "3",
                            "oncePerFiscalYear": true
                        },
                        {
                            "id": "f604ebf4-8c7a-4e48-b9c5-40be292dcb7d",
                            "active": true,
                            "fiscalYearId": "15f21721-50e8-41af-9596-9d8cdb8d6a16",
                            "linkedFeeHeadId": "36",
                            "sourceFeeHeadId": "25",
                            "oncePerFiscalYear": true
                        }
                    ],
                    "enabled": true
                },
                "displacedTagging": {
                    "enabled": true
                },
                "receiptCancelWindow": {
                    "hours": 24,
                    "enabled": false
                },
                "insuranceEligibility": {
                    "enabled": false,
                    "ruleIds": [],
                    "minPaidRules": 1
                }
            })),
            external_content: None,
        }
    }

    #[test]
    fn test_render_templates_from_setting_resource_cache() {
        let mut cache = Cache::new();
        cache.write(ResourceName::SettingResource, 1, sample_setting_resource());

        // PAID -> payment-received
        let paid_tpl = SmsTemplate::new_paid(
            "9841000000",
            "BA-01-PA-1234",
            "Loadboard sthaniya",
            2000.0,
            "2083-08-30",
        );
        assert_eq!(
            paid_tpl.render_with_cache(&cache).unwrap(),
            "तपाईंको सवारी नम्बर BA-01-PA-1234 को रकम रु 2000 प्राप्त भएको छ।\nधन्यवाद। Loadboard sthaniya\nम्याद सकिने मिति: 2083-08-30"
        );

        // PAYMENT_REMAINDER -> payment-reminder (uses {vehicleNumber}, {amount}, {dueDate}, {organizationName})
        let rem_tpl = SmsTemplate::new_payment_remainder(
            "9841000000",
            "BA-01-PA-1234",
            "Loadboard sthaniya",
            "2083-09-15",
        )
        .with_amount(500.0);
        assert_eq!(
            rem_tpl.render_with_cache(&cache).unwrap(),
            "नमस्कार, सवारी नम्बर BA-01-PA-1234 को रु 500 भुक्तानी बाँकी छ। कृपया 2083-09-15 भित्र भुक्तानी गर्नुहोस्।\nLoadboard sthaniya"
        );

        // URGENT -> urgent-reminder
        let urgent_tpl = SmsTemplate::new_urgent(
            "9841000000",
            "BA-01-PA-1234",
            "Loadboard sthaniya",
            "2083-09-20",
        );
        assert_eq!(
            urgent_tpl.render_with_cache(&cache).unwrap(),
            "अत्यावश्यक सूचना: सवारी नम्बर BA-01-PA-1234 को भुक्तानी म्याद 2083-09-20 मा सकिँदैछ। सेवा निरन्तरताका लागि समयमै भुक्तानी गर्नुहोस्।\nLoadboard sthaniya"
        );

        // Falls back to default template when SettingResource is absent in cache
        let empty_cache = Cache::new();
        assert_eq!(
            paid_tpl.render_with_cache(&empty_cache).unwrap(),
            paid_tpl.render().unwrap()
        );
    }

    #[actix_web::test]
    async fn test_live_send_paid_sms_using_setting_resource_template() {
        dotenv::dotenv().ok();

        let mut cache = Cache::new();
        cache.write(ResourceName::SettingResource, 1, sample_setting_resource());

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        let random_num = 1000 + (nanos % 9000);
        let vehicle_number = format!("BA-01-PA-{random_num}");

        let tpl = SmsTemplate::new_paid(
            "9861282176",
            &vehicle_number,
            "lodbod test sangh",
            0.0,
            "2083-12-30",
        );

        let client = SmsClient::from_env();
        let result = client
            .send_template_with_cache(&tpl, &cache)
            .await
            .expect("Live SamayaSMS request with SettingResource PAID template should succeed");

        println!("Live SettingResource SMS sent: {result:?}");
        assert!(!result.shoot_id.is_empty());
        assert_eq!(result.contacts, "9861282176");
        assert_eq!(
            result.message,
            format!(
                "तपाईंको सवारी नम्बर {vehicle_number} को रकम रु 0 प्राप्त भएको छ।\nधन्यवाद। lodbod test sangh\nम्याद सकिने मिति: 2083-12-30"
            )
        );
    }
}
