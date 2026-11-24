use crate::cache::Cache;
use crate::resource::{DatabaseResource, ResourceName};
use crate::ServerData;
use actix_web::{web, HttpMessage, HttpResponse};
use anyhow::{anyhow, bail, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::env;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/*



Sends a standard text (GSM) SMS to one or more recipients. All parameters are passed as GET query strings or POST fields.

Request Parameters
Parameter	Required	Description
key	Required	Your API Key, used to authenticate every request. You can regenerate it from the top of this page.
campaign	Optional	ID of the campaign to be used, as defined in your account.
Default CampaignID = 8572
routeid	Optional	ID of the route to be used, as defined in your account.
FTTRANS_RTID = 10244
type	Required	SMS type. Set to text for a standard SMS.
responsetype	Optional	Response format for the API call: http, json or xml. Defaults to http when omitted.
contacts	Required	Recipient mobile numbers separated by commas, e.g. 9841XXXXXX,9801XXXXXX.
senderid	Required	Any sender ID approved for your account.
msg	Required	URL-encoded SMS text. Maximum length: 720 characters.
time	Optional	Scheduled delivery time in the format YYYY-MM-DD H:i (e.g. 2025-12-01 14:30). Leave blank to send the SMS immediately.
API Responses
API Response	Description
ERR: {<MESSAGE>}	An error occurred while processing the request. The cause is described inside the braces — for example, ERR: INVALID API KEY means the API key is expired or does not belong to any user. Error messages are plain text and self-explanatory.
SMS-SHOOT-ID/{<alpha-numeric string>}	The SMS was submitted successfully. The returned shoot ID (the part after the slash) can be used later to fetch delivery reports via the DLR API.
Sample API Calls
Send a Text SMS

                                $api_url = "https://samayasms.com.np/smsapi/index.php?key=366979AD75A35B&campaign=XXXXXX&routeid=XXXXXX&type=text&contacts=9841XXXXXX,9801XXXXXX&senderid=XXXXXX&msg=Hello%2C+this+is+a+test+SMS.";
                            
Schedule a Text SMS

                                $api_url = "https://samayasms.com.np/smsapi/index.php?key=366979AD75A35B&campaign=XXXXXX&routeid=XXXXXX&type=text&contacts=9841XXXXXX,9801XXXXXX&senderid=XXXXXX&msg=Hello%2C+this+is+a+test+SMS.&time=2026-11-23+14%3A59";
                            
 */

pub const DEFAULT_SMS_BASE_URL: &str = "https://samayasms.com.np/smsapi/index.php";
pub const DEFAULT_SMS_API_KEY: &str = "366979AD75A35B";
pub const DEFAULT_SMS_CAMPAIGN_ID: &str = "8572";
pub const DEFAULT_SMS_ROUTE_ID: &str = "10244";
pub const MAX_SMS_CHAR_LEN: usize = 720;
/// SamayaSMS rate limit: 2 requests per second per user (500ms minimum spacing).
const MIN_REQUEST_INTERVAL: Duration = Duration::from_millis(500);

#[allow(non_camel_case_types)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmsType {
    PAID,
    PAYMENT_REMAINDER,
    URGENT,
    CUSTOM(String),
}

impl Default for SmsType {
    fn default() -> Self {
        Self::CUSTOM(String::new())
    }
}

impl SmsType {
    pub fn from_id(raw: &str) -> Self {
        let trimmed = raw.trim();
        let norm: String = trimmed
            .chars()
            .map(|c| {
                if c == ' ' || c == '-' {
                    '_'
                } else {
                    c.to_ascii_uppercase()
                }
            })
            .collect();
        match norm.as_str() {
            "PAID" | "PAYMENT_RECEIVED" => Self::PAID,
            "PAYMENT_REMAINDER" | "PAYMENT_REMINDER" => Self::PAYMENT_REMAINDER,
            "URGENT" | "URGENT_REMINDER" => Self::URGENT,
            _ => Self::CUSTOM(trimmed.to_string()),
        }
    }

    pub fn setting_template_ids(&self) -> Vec<&str> {
        match self {
            Self::PAID => vec!["PAID", "payment-received", "paid", "payment_received"],
            Self::PAYMENT_REMAINDER => vec![
                "PAYMENT REMAINDER",
                "PAYMENT REMINDER",
                "PAYMENT_REMAINDER",
                "PAYMENT_REMINDER",
                "payment-reminder",
                "payment-remainder",
                "payment_reminder",
                "payment_remainder",
            ],
            Self::URGENT => vec!["URGENT", "urgent-reminder", "urgent", "urgent_reminder"],
            Self::CUSTOM(id) => {
                let t = id.trim();
                if t.is_empty() {
                    Vec::new()
                } else {
                    vec![t]
                }
            }
        }
    }

    pub fn matches_template_id(&self, tpl_id: &str) -> bool {
        let target = tpl_id.trim();
        self.setting_template_ids()
            .into_iter()
            .any(|candidate| target.eq_ignore_ascii_case(candidate))
    }
}

impl Serialize for SmsType {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::PAID => serializer.serialize_str("PAID"),
            Self::PAYMENT_REMAINDER => serializer.serialize_str("PAYMENT_REMAINDER"),
            Self::URGENT => serializer.serialize_str("URGENT"),
            Self::CUSTOM(id) => serializer.serialize_str(id),
        }
    }
}

impl<'de> Deserialize<'de> for SmsType {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(Self::from_id(&raw))
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

/// Extracts unique `{placeholder}` variable names in appearance order from an SMS template body.
pub fn extract_template_placeholders(content: &str) -> Vec<String> {
    let mut placeholders = Vec::new();
    let mut seen = HashSet::new();
    let mut rest = content;

    while let Some(open_idx) = rest.find('{') {
        let after_open = &rest[open_idx + 1..];
        let Some(close_idx) = after_open.find('}') else {
            break;
        };
        let candidate = &after_open[..close_idx];
        if let Some(nested_open) = candidate.rfind('{') {
            rest = &after_open[nested_open..];
            continue;
        }
        if !candidate.is_empty() && !candidate.chars().any(char::is_whitespace) {
            if seen.insert(candidate.to_string()) {
                placeholders.push(candidate.to_string());
            }
        }
        rest = &after_open[close_idx + 1..];
    }

    placeholders
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GeneratedSmsTemplate {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub content: String,
    pub placeholders: Vec<String>,
    pub sms_type: SmsType,
}

impl GeneratedSmsTemplate {
    pub fn from_setting_template(tpl: SettingSmsTemplate) -> Self {
        let placeholders = extract_template_placeholders(&tpl.content);
        let sms_type = SmsType::from_id(&tpl.id);
        Self {
            id: tpl.id,
            name: tpl.name,
            active: tpl.active,
            channel: tpl.channel,
            category: tpl.category,
            content: tpl.content,
            placeholders,
            sms_type,
        }
    }

    pub fn render_with_variables(&self, variables: &HashMap<String, Value>) -> Result<String> {
        render_content_with_variables(&self.content, &self.sms_type, variables)
    }
}

fn raw_sms_templates_from_value(inner: &Value) -> Option<&Vec<Value>> {
    inner
        .pointer("/notifications/sms/templates")
        .and_then(Value::as_array)
        .or_else(|| {
            inner
                .get("smsSettings")
                .or_else(|| inner.get("sms_settings"))
                .and_then(|s| s.get("templates"))
                .and_then(Value::as_array)
        })
}

/// Generates all valid SMS templates configured inside a single `SettingResource` JSON payload
/// (`notifications.sms.templates` preferred, falling back to `smsSettings.templates`).
pub fn generate_templates_from_value(inner: &Value) -> Vec<GeneratedSmsTemplate> {
    let Some(raw_templates) = raw_sms_templates_from_value(inner) else {
        return Vec::new();
    };

    raw_templates
        .iter()
        .filter_map(|raw_tpl| serde_json::from_value::<SettingSmsTemplate>(raw_tpl.clone()).ok())
        .filter(|tpl| !tpl.content.trim().is_empty())
        .filter(|tpl| {
            tpl.channel
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .is_none_or(|ch| ch.eq_ignore_ascii_case("SMS"))
        })
        .map(GeneratedSmsTemplate::from_setting_template)
        .collect()
}

/// Generates SMS templates from the latest matching `ResourceName::SettingResource` entry.
pub fn generate_templates_from_settings<'a, I>(settings: I) -> Vec<GeneratedSmsTemplate>
where
    I: IntoIterator<Item = &'a DatabaseResource>,
{
    let mut best: Option<(bool, i32, Vec<GeneratedSmsTemplate>)> = None;

    for res in settings {
        let Some(inner) = res.inner_content.as_ref() else {
            continue;
        };
        if raw_sms_templates_from_value(inner).is_none() {
            continue;
        }
        let generated = generate_templates_from_value(inner);
        let is_nested = inner.pointer("/notifications/sms/templates").is_some();
        let res_id = res.id.unwrap_or(0);
        let replace = match &best {
            None => true,
            Some((best_nested, best_id, _)) => (is_nested, res_id) >= (*best_nested, *best_id),
        };
        if replace {
            best = Some((is_nested, res_id, generated));
        }
    }

    best.map(|(_, _, list)| list).unwrap_or_default()
}

pub fn generate_templates_from_cache(cache: &Cache) -> Vec<GeneratedSmsTemplate> {
    let Ok(settings) = cache.read_all(ResourceName::SettingResource) else {
        return Vec::new();
    };
    generate_templates_from_settings(settings)
}

pub async fn generate_templates_from_server_data(
    s_data: &ServerData,
) -> Vec<GeneratedSmsTemplate> {
    {
        let cache = s_data.cache.read().await;
        let cached = generate_templates_from_cache(&cache);
        if !cached.is_empty() {
            return cached;
        }
    }

    let Ok(db_settings) = s_data
        .db
        .read_all_resource(ResourceName::SettingResource)
        .await
    else {
        return Vec::new();
    };
    generate_templates_from_settings(db_settings.iter())
}

/// Finds the matching SMS template inside a collection of `ResourceName::SettingResource` records
/// (`inner_content.notifications.sms.templates`, falling back to `inner_content.smsSettings.templates`).
/// Prefers active templates and newer resource IDs while falling back to inactive matches if no active template matches.
pub fn find_sms_template_in_settings<'a, I>(
    settings: I,
    sms_type: &SmsType,
    template_id_override: Option<&str>,
) -> Option<SettingSmsTemplate>
where
    I: IntoIterator<Item = &'a DatabaseResource>,
{
    let target_override = template_id_override
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let mut best: Option<(bool, i32, SettingSmsTemplate)> = None;

    for res in settings {
        let Some(inner) = res.inner_content.as_ref() else {
            continue;
        };
        let Some(templates) = raw_sms_templates_from_value(inner) else {
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
                None => sms_type.matches_template_id(tpl_id),
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
    sms_type: &SmsType,
    template_id_override: Option<&str>,
) -> Option<SettingSmsTemplate> {
    let settings = cache.read_all(ResourceName::SettingResource).ok()?;
    find_sms_template_in_settings(settings, sms_type, template_id_override)
}

/// Looks up the matching SMS template from `s_data.cache` (`ResourceName::SettingResource`),
/// falling back to `s_data.db.read_all_resource(ResourceName::SettingResource)` when cache misses.
pub async fn find_sms_template_in_server_data(
    s_data: &ServerData,
    sms_type: &SmsType,
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

/// Extracts `SmsNotificationSettings` from `SettingResource` items (preferring `notifications.sms`,
/// falling back to legacy `smsSettings`).
pub fn extract_sms_notification_settings<'a, I>(
    settings: I,
) -> Option<crate::settings::SmsNotificationSettings>
where
    I: IntoIterator<Item = &'a DatabaseResource>,
{
    let mut best: Option<(i32, crate::settings::SmsNotificationSettings)> = None;
    for res in settings {
        let Some(inner) = res.inner_content.as_ref() else {
            continue;
        };
        if inner.pointer("/notifications/sms").is_some()
            || inner.get("smsSettings").is_some()
            || inner.get("sms_settings").is_some()
        {
            let res_id = res.id.unwrap_or(0);
            let parsed = crate::settings::AppSettings::from_inner_value(inner)
                .notifications
                .sms;
            if best.as_ref().is_none_or(|(best_id, _)| res_id >= *best_id) {
                best = Some((res_id, parsed));
            }
        }
    }
    best.map(|(_, s)| s)
}

pub async fn find_sms_notification_settings_in_server_data(
    s_data: &ServerData,
) -> Option<crate::settings::SmsNotificationSettings> {
    {
        let cache = s_data.cache.read().await;
        if let Ok(settings) = cache.read_all(ResourceName::SettingResource) {
            if let Some(found) = extract_sms_notification_settings(settings) {
                return Some(found);
            }
        }
    }
    let db_settings = s_data
        .db
        .read_all_resource(ResourceName::SettingResource)
        .await
        .ok()?;
    extract_sms_notification_settings(db_settings.iter())
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
    #[serde(default, alias = "smsType")]
    pub sms_type: SmsType,
    #[serde(default, alias = "templateId", alias = "id")]
    pub template_id: Option<String>,
    #[serde(default, alias = "vehicleNumber")]
    pub vehicle_number: String,
    #[serde(
        default,
        alias = "sanghBranch",
        alias = "organizationName",
        alias = "organization_name"
    )]
    pub sangh_branch: String,
    #[serde(default, alias = "expiryDate", alias = "expiryBs", alias = "expiry_date")]
    pub expiry_bs: Option<String>,
    #[serde(
        default,
        alias = "due_day_bs",
        alias = "dueDate",
        alias = "dueDayBs",
        alias = "due_date"
    )]
    pub due_date_bs: Option<String>,
    #[serde(default)]
    pub amount: Option<f64>,
    /// Optional scheduled delivery time in `YYYY-MM-DD HH:MM` format.
    #[serde(default, alias = "time", alias = "scheduledTime")]
    pub scheduled_time: Option<String>,
    #[serde(default)]
    pub variables: HashMap<String, Value>,
}

impl SmsTemplate {
    pub fn for_template_id(receiver: impl Into<String>, template_id: impl Into<String>) -> Self {
        let tid = template_id.into();
        Self {
            receiver: receiver.into(),
            sms_type: SmsType::from_id(&tid),
            template_id: Some(tid),
            vehicle_number: String::new(),
            sangh_branch: String::new(),
            expiry_bs: None,
            due_date_bs: None,
            amount: None,
            scheduled_time: None,
            variables: HashMap::new(),
        }
    }

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
            vehicle_number: vehicle_number.into(),
            sangh_branch: sangh_branch.into(),
            expiry_bs: Some(expiry_bs.into()),
            due_date_bs: None,
            amount: Some(amount),
            scheduled_time: None,
            variables: HashMap::new(),
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
            vehicle_number: vehicle_number.into(),
            sangh_branch: sangh_branch.into(),
            expiry_bs: None,
            due_date_bs: Some(due_date_bs.into()),
            amount: None,
            scheduled_time: None,
            variables: HashMap::new(),
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
            vehicle_number: vehicle_number.into(),
            sangh_branch: sangh_branch.into(),
            expiry_bs: None,
            due_date_bs: Some(due_date_bs.into()),
            amount: None,
            scheduled_time: None,
            variables: HashMap::new(),
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

    pub fn with_scheduled_time(mut self, time: impl Into<String>) -> Self {
        self.scheduled_time = Some(time.into());
        self
    }

    pub fn with_variable(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.variables.insert(key.into(), value.into());
        self
    }

    /// Normalizes comma-separated recipient phone numbers and validates that at least one is present.
    pub fn normalized_contacts(&self) -> Result<String> {
        normalize_contacts(&self.receiver)
    }

    fn effective_variables(&self) -> HashMap<String, Value> {
        let mut vars = self.variables.clone();

        let vn = self.vehicle_number.trim();
        if !vn.is_empty() {
            vars.entry("vehicleNumber".to_string())
                .or_insert_with(|| Value::String(vn.to_string()));
            vars.entry("vehicle_number".to_string())
                .or_insert_with(|| Value::String(vn.to_string()));
        }

        let sb = self.sangh_branch.trim();
        if !sb.is_empty() {
            for alias in [
                "organizationName",
                "organization_name",
                "sanghBranch",
                "sangh_branch",
            ] {
                vars.entry(alias.to_string())
                    .or_insert_with(|| Value::String(sb.to_string()));
            }
        }

        if let Some(amount) = self.amount {
            vars.entry("amount".to_string())
                .or_insert_with(|| Value::String(format_amount(amount)));
        }

        if let Some(exp) = self
            .expiry_bs
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            for alias in ["expiryDate", "expiry_date", "expiryBs", "expiry_bs"] {
                vars.entry(alias.to_string())
                    .or_insert_with(|| Value::String(exp.to_string()));
            }
        }

        if let Some(due) = self
            .due_date_bs
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            for alias in [
                "dueDate",
                "due_date",
                "dueDayBs",
                "due_date_bs",
                "due_day_bs",
            ] {
                vars.entry(alias.to_string())
                    .or_insert_with(|| Value::String(due.to_string()));
            }
        }

        vars
    }

    /// Renders the final SMS text using a template string loaded from `SettingResource`,
    /// substituting both structured fields and dynamic `{placeholder}` variables while enforcing
    /// the 720-character limit.
    pub fn render_with_template(&self, template_content: &str) -> Result<String> {
        let vars = self.effective_variables();
        render_content_with_variables(template_content, &self.sms_type, &vars)
    }

    /// Renders the template using formats configured inside `SettingResource` items.
    pub fn render_with_settings<'a, I>(&self, settings: I) -> Result<String>
    where
        I: IntoIterator<Item = &'a DatabaseResource>,
    {
        let setting_tpl = find_sms_template_in_settings(
            settings,
            &self.sms_type,
            self.template_id.as_deref(),
        )
        .ok_or_else(|| {
            anyhow!(
                "SMS template for {:?} not found in SettingResource",
                self.sms_type
            )
        })?;
        self.render_with_template(&setting_tpl.content)
    }

    /// Renders the template using `ResourceName::SettingResource` entries stored in `Cache`.
    pub fn render_with_cache(&self, cache: &Cache) -> Result<String> {
        let setting_tpl =
            find_sms_template_in_cache(cache, &self.sms_type, self.template_id.as_deref())
                .ok_or_else(|| {
                    anyhow!(
                        "SMS template for {:?} not found in SettingResource",
                        self.sms_type
                    )
                })?;
        self.render_with_template(&setting_tpl.content)
    }
}

fn format_variable_value(val: &Value) -> Option<String> {
    match val {
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Value::Number(n) => Some(n.as_f64().map(format_amount).unwrap_or_else(|| n.to_string())),
        Value::Bool(b) => Some(b.to_string()),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

fn placeholder_aliases(key: &str) -> &'static [&'static str] {
    match key {
        "vehicleNumber" | "vehicle_number" => &["vehicleNumber", "vehicle_number"],
        "organizationName" | "organization_name" | "sanghBranch" | "sangh_branch" => &[
            "organizationName",
            "organization_name",
            "sanghBranch",
            "sangh_branch",
        ],
        "expiryDate" | "expiry_date" | "expiryBs" | "expiry_bs" => {
            &["expiryDate", "expiry_date", "expiryBs", "expiry_bs"]
        }
        "dueDate" | "due_date" | "dueDayBs" | "due_date_bs" | "due_day_bs" => &[
            "dueDate",
            "due_date",
            "dueDayBs",
            "due_date_bs",
            "due_day_bs",
        ],
        _ => &[],
    }
}

fn normalize_var_key(k: &str) -> String {
    k.chars()
        .filter(|c| *c != '_' && *c != '-')
        .flat_map(char::to_lowercase)
        .collect()
}

fn lookup_variable(key: &str, variables: &HashMap<String, Value>) -> Option<String> {
    if let Some(val) = variables.get(key).and_then(format_variable_value) {
        return Some(val);
    }
    for alias in placeholder_aliases(key) {
        if let Some(val) = variables.get(*alias).and_then(format_variable_value) {
            return Some(val);
        }
    }
    let norm_target = normalize_var_key(key);
    for (k, v) in variables {
        if normalize_var_key(k) == norm_target {
            if let Some(val) = format_variable_value(v) {
                return Some(val);
            }
        }
    }
    None
}

fn missing_variable_error(placeholder: &str, sms_type: &SmsType) -> anyhow::Error {
    match placeholder {
        "vehicleNumber" | "vehicle_number" => anyhow!("vehicle_number is required"),
        "organizationName" | "organization_name" | "sanghBranch" | "sangh_branch" => {
            anyhow!("sangh_branch is required")
        }
        "amount" => anyhow!("amount is required for {:?} SMS template", sms_type),
        "expiryDate" | "expiry_date" | "expiryBs" | "expiry_bs" => {
            anyhow!("expiry_bs is required for {:?} SMS template", sms_type)
        }
        "dueDate" | "due_date" | "dueDayBs" | "due_date_bs" | "due_day_bs" => {
            anyhow!("due_date_bs is required for {:?} SMS template", sms_type)
        }
        other => anyhow!("template variable '{other}' is required for SMS template"),
    }
}

pub fn render_content_with_variables(
    template_content: &str,
    sms_type: &SmsType,
    variables: &HashMap<String, Value>,
) -> Result<String> {
    let template = template_content.trim();
    if template.is_empty() {
        bail!("SMS template content in SettingResource is empty");
    }

    let mut msg = template.to_string();
    for placeholder in extract_template_placeholders(template) {
        let value = lookup_variable(&placeholder, variables)
            .ok_or_else(|| missing_variable_error(&placeholder, sms_type))?;
        msg = msg.replace(&format!("{{{placeholder}}}"), &value);
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
    normalize_contacts_with_country_code(raw, None)
}

pub fn normalize_contacts_with_country_code(
    raw: &str,
    default_country_code: Option<&str>,
) -> Result<String> {
    let prefix_digits: Option<String> = default_country_code
        .map(|cc| {
            cc.chars()
                .filter(|c| c.is_ascii_digit())
                .collect::<String>()
        })
        .filter(|d| !d.is_empty());

    let cleaned: Vec<String> = raw
        .split(',')
        .filter_map(|part| {
            let digits: String = part
                .trim()
                .trim_start_matches('+')
                .chars()
                .filter(|c| c.is_ascii_digit())
                .collect();
            if digits.is_empty() {
                return None;
            }
            if let Some(ref prefix) = prefix_digits {
                if !digits.starts_with(prefix) {
                    let stripped = digits.strip_prefix('0').unwrap_or(&digits);
                    return Some(format!("{prefix}{stripped}"));
                }
            }
            Some(digits)
        })
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
    pub provider: String,
    pub api_secret: String,
    pub webhook_url: Option<String>,
    pub default_country_code: Option<String>,
    pub enabled: bool,
}

impl Default for SmsConfig {
    fn default() -> Self {
        Self::from_env()
    }
}

fn read_env_non_empty(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn resolve_route_and_campaign(
    effective_api_key: &str,
    setting_route_id: Option<&str>,
    setting_campaign_id: Option<&str>,
    env_route_id: Option<String>,
    env_campaign_id: Option<String>,
) -> (Option<String>, Option<String>) {
    let clean_ref = |opt: Option<&str>| {
        opt.map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    };
    let clean_owned = |opt: Option<String>| {
        opt.map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let use_default_account_ids = effective_api_key.trim() == DEFAULT_SMS_API_KEY;

    let route_id = clean_ref(setting_route_id)
        .or_else(|| clean_owned(env_route_id))
        .or_else(|| use_default_account_ids.then(|| DEFAULT_SMS_ROUTE_ID.to_string()));

    let campaign_id = clean_ref(setting_campaign_id)
        .or_else(|| clean_owned(env_campaign_id))
        .or_else(|| use_default_account_ids.then(|| DEFAULT_SMS_CAMPAIGN_ID.to_string()));

    (route_id, campaign_id)
}

impl SmsConfig {
    pub fn from_env() -> Self {
        Self::from_env_and_setting(None)
    }

    pub fn from_setting_or_env(
        setting_sms: Option<&crate::settings::SmsNotificationSettings>,
    ) -> Self {
        Self::from_env_and_setting(setting_sms)
    }

    fn from_env_and_setting(
        setting_sms: Option<&crate::settings::SmsNotificationSettings>,
    ) -> Self {
        let mut base_url =
            read_env_non_empty("SMS_API_BASE_URL").unwrap_or_else(|| DEFAULT_SMS_BASE_URL.to_string());
        let mut api_key =
            read_env_non_empty("SMS_API_KEY").unwrap_or_else(|| DEFAULT_SMS_API_KEY.to_string());
        let mut sender_id = read_env_non_empty("SMS_SENDER_ID").unwrap_or_default();
        let response_type = read_env_non_empty("SMS_RESPONSE_TYPE");
        let sms_api_type = read_env_non_empty("SMS_API_TYPE");
        let mut provider = "samayasms".to_string();
        let mut api_secret = read_env_non_empty("SMS_API_SECRET").unwrap_or_default();
        let mut webhook_url = read_env_non_empty("SMS_WEBHOOK_URL");
        let mut default_country_code = read_env_non_empty("SMS_DEFAULT_COUNTRY_CODE");
        let mut enabled = true;
        let mut setting_route_id: Option<&str> = None;
        let mut setting_campaign_id: Option<&str> = None;

        if let Some(s) = setting_sms {
            enabled = s.enabled;
            if !s.provider.trim().is_empty() {
                provider = s.provider.trim().to_string();
            }
            if !s.base_url.trim().is_empty() {
                base_url = s.base_url.trim().to_string();
            }
            if !s.api_key.trim().is_empty() {
                api_key = s.api_key.trim().to_string();
            }
            if !s.sender_id.trim().is_empty() {
                sender_id = s.sender_id.trim().to_string();
            }
            if !s.api_secret.trim().is_empty() {
                api_secret = s.api_secret.trim().to_string();
            }
            if !s.webhook_url.trim().is_empty() {
                webhook_url = Some(s.webhook_url.trim().to_string());
            }
            if !s.default_country_code.trim().is_empty() {
                default_country_code = Some(s.default_country_code.trim().to_string());
            }
            setting_route_id = s.route_id.as_deref();
            setting_campaign_id = s.campaign_id.as_deref();
        }

        let (route_id, campaign_id) = resolve_route_and_campaign(
            &api_key,
            setting_route_id,
            setting_campaign_id,
            read_env_non_empty("SMS_ROUTE_ID"),
            read_env_non_empty("SMS_CAMPAIGN_ID"),
        );

        Self {
            base_url,
            api_key,
            campaign_id,
            route_id,
            sender_id,
            response_type,
            sms_api_type,
            provider,
            api_secret,
            webhook_url,
            default_country_code,
            enabled,
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

        let mut params: Vec<(&'static str, String)> = Vec::with_capacity(10);
        params.push(("key", self.config.api_key.clone()));
        if !self.config.api_secret.trim().is_empty() {
            params.push(("secret", self.config.api_secret.clone()));
        }
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

    fn spawn_webhook_notification(
        &self,
        contacts: &str,
        shoot_id: Option<&str>,
        error_msg: Option<&str>,
    ) {
        let Some(ref webhook_url) = self.config.webhook_url else {
            return;
        };
        let url = webhook_url.trim().to_string();
        if url.is_empty() {
            return;
        }
        let http = self.http.clone();
        let payload = serde_json::json!({
            "event": if error_msg.is_none() { "sms_sent" } else { "sms_failed" },
            "provider": self.config.provider,
            "contacts": contacts,
            "shootId": shoot_id,
            "status": if error_msg.is_none() { "SENT" } else { "FAILED" },
            "error": error_msg,
        });
        tokio::spawn(async move {
            let _ = http.post(&url).json(&payload).send().await;
        });
    }

    /// Renders an `SmsTemplate` using `ResourceName::SettingResource` stored in `Cache`
    /// and sends it through the configured SMS provider.
    pub async fn send_template_with_cache(
        &self,
        template: &SmsTemplate,
        cache: &Cache,
    ) -> Result<SmsSendResult> {
        let setting_sms = cache
            .read_all(ResourceName::SettingResource)
            .ok()
            .and_then(extract_sms_notification_settings);
        if let Some(ref sms_cfg) = setting_sms {
            if !sms_cfg.enabled {
                bail!("SMS notifications are disabled in SettingResource");
            }
        }
        let settings = cache
            .read_all(ResourceName::SettingResource)
            .ok()
            .unwrap_or_default();
        let setting_tpl = find_sms_template_in_settings(
            settings,
            &template.sms_type,
            template.template_id.as_deref(),
        )
        .ok_or_else(|| {
            anyhow!(
                "SMS template for {:?} not found in SettingResource",
                template.sms_type
            )
        })?;
        if !setting_tpl.active {
            bail!(
                "SMS template '{}' is inactive in SettingResource",
                setting_tpl.id
            );
        }

        let effective_cc = self.config.default_country_code.as_deref().or_else(|| {
            setting_sms
                .as_ref()
                .map(|s| s.default_country_code.as_str())
                .filter(|s| !s.trim().is_empty())
        });
        let contacts =
            normalize_contacts_with_country_code(&template.receiver, effective_cc)?;
        let message = template.render_with_template(&setting_tpl.content)?;
        self.send_raw_with_contacts(&contacts, &message, template.scheduled_time.as_deref())
            .await
    }

    /// Renders an `SmsTemplate` using `ResourceName::SettingResource` from `ServerData`
    /// (cache first, falling back to DB) and sends it through the configured SMS provider.
    pub async fn send_template_with_server_data(
        &self,
        template: &SmsTemplate,
        s_data: &ServerData,
    ) -> Result<SmsSendResult> {
        let setting_sms = find_sms_notification_settings_in_server_data(s_data).await;
        if let Some(ref sms_cfg) = setting_sms {
            if !sms_cfg.enabled {
                bail!("SMS notifications are disabled in SettingResource");
            }
        }
        let setting_tpl = find_sms_template_in_server_data(
            s_data,
            &template.sms_type,
            template.template_id.as_deref(),
        )
        .await
        .ok_or_else(|| {
            anyhow!(
                "SMS template for {:?} not found in SettingResource",
                template.sms_type
            )
        })?;
        if !setting_tpl.active {
            bail!(
                "SMS template '{}' is inactive in SettingResource",
                setting_tpl.id
            );
        }

        let effective_cc = self.config.default_country_code.as_deref().or_else(|| {
            setting_sms
                .as_ref()
                .map(|s| s.default_country_code.as_str())
                .filter(|s| !s.trim().is_empty())
        });
        let contacts =
            normalize_contacts_with_country_code(&template.receiver, effective_cc)?;
        let message = template.render_with_template(&setting_tpl.content)?;
        self.send_raw_with_contacts(&contacts, &message, template.scheduled_time.as_deref())
            .await
    }

    /// Sends a raw SMS message to one or more comma-separated mobile numbers.
    pub async fn send_raw(
        &self,
        contacts: &str,
        message: &str,
        scheduled_time: Option<&str>,
    ) -> Result<SmsSendResult> {
        let normalized_contacts = normalize_contacts_with_country_code(
            contacts,
            self.config.default_country_code.as_deref(),
        )?;
        self.send_raw_with_contacts(&normalized_contacts, message, scheduled_time)
            .await
    }

    async fn send_raw_with_contacts(
        &self,
        normalized_contacts: &str,
        message: &str,
        scheduled_time: Option<&str>,
    ) -> Result<SmsSendResult> {
        if !self.config.enabled {
            bail!("SMS notifications are disabled in SettingResource");
        }
        if !self.config.provider.trim().eq_ignore_ascii_case("samayasms") {
            bail!(
                "Unsupported SMS provider '{}' configured in SettingResource",
                self.config.provider
            );
        }
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

        let params = self.build_form_params(normalized_contacts, trimmed_msg, scheduled_time);

        self.wait_for_rate_limit().await;

        let response = match self
            .http
            .post(&self.config.base_url)
            .form(&params)
            .send()
            .await
        {
            Ok(res) => res,
            Err(err) => {
                let err_str = format!("Failed to send HTTP request to SamayaSMS API: {err}");
                self.spawn_webhook_notification(normalized_contacts, None, Some(&err_str));
                bail!("{err_str}");
            }
        };

        let status = response.status();
        let body = response
            .text()
            .await
            .context("Failed to read SamayaSMS API response body")?;

        let trimmed_body = body.trim();
        if !status.is_success() {
            let err_str = format!("SamayaSMS API returned HTTP {status}: {trimmed_body}");
            self.spawn_webhook_notification(normalized_contacts, None, Some(&err_str));
            bail!("{err_str}");
        }

        match parse_samaya_sms_response(trimmed_body) {
            Ok(shoot_id) => {
                self.spawn_webhook_notification(normalized_contacts, Some(&shoot_id), None);
                Ok(SmsSendResult {
                    shoot_id,
                    contacts: normalized_contacts.to_string(),
                    message: trimmed_msg.to_string(),
                    raw_response: trimmed_body.to_string(),
                })
            }
            Err(err) => {
                let err_str = err.to_string();
                self.spawn_webhook_notification(normalized_contacts, None, Some(&err_str));
                Err(err)
            }
        }
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

/// GET /api/v1/sms/templates — Lists SMS templates generated dynamically from `SettingResource`.
pub async fn list_sms_templates(s_data: web::Data<Arc<ServerData>>) -> HttpResponse {
    let templates = generate_templates_from_server_data(&s_data).await;
    let setting_sms = find_sms_notification_settings_in_server_data(&s_data).await;
    let resolved_cfg = SmsConfig::from_setting_or_env(setting_sms.as_ref());

    HttpResponse::Ok().json(serde_json::json!({
        "source": "SettingResource",
        "enabled": resolved_cfg.enabled,
        "provider": resolved_cfg.provider,
        "senderId": resolved_cfg.sender_id,
        "baseUrl": resolved_cfg.base_url,
        "routeId": resolved_cfg.route_id,
        "campaignId": resolved_cfg.campaign_id,
        "defaultCountryCode": resolved_cfg.default_country_code.unwrap_or_default(),
        "templates": templates,
    }))
}

/// GET /api/v1/sms/templates/{template_id} — Returns a generated SMS template by ID or alias.
pub async fn get_sms_template_by_id(
    path: web::Path<String>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let raw_id = path.into_inner();
    let trimmed_id = raw_id.trim();
    if trimmed_id.is_empty() {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "error": "template_id is required"
        }));
    }

    let templates = generate_templates_from_server_data(&s_data).await;
    let requested_type = SmsType::from_id(trimmed_id);
    let found = templates
        .iter()
        .find(|t| t.id.trim().eq_ignore_ascii_case(trimmed_id))
        .or_else(|| {
            templates
                .iter()
                .find(|t| requested_type.matches_template_id(&t.id))
        });

    match found {
        Some(tpl) => HttpResponse::Ok().json(tpl),
        None => HttpResponse::NotFound().json(serde_json::json!({
            "error": format!("SMS template '{trimmed_id}' not found in SettingResource")
        })),
    }
}

/// POST /api/v1/sms/preview — Renders and validates an `SmsTemplate` using `SettingResource` without sending.
pub async fn preview_sms_template(
    template: web::Json<SmsTemplate>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let tpl = template.into_inner();
    let setting_sms = find_sms_notification_settings_in_server_data(&s_data).await;
    let default_cc = setting_sms
        .as_ref()
        .map(|s| s.default_country_code.as_str())
        .filter(|s| !s.trim().is_empty());
    let contacts = match normalize_contacts_with_country_code(&tpl.receiver, default_cc) {
        Ok(c) => c,
        Err(e) => return HttpResponse::BadRequest().json(serde_json::json!({ "error": e.to_string() })),
    };

    let Some(setting_tpl) =
        find_sms_template_in_server_data(&s_data, &tpl.sms_type, tpl.template_id.as_deref()).await
    else {
        return HttpResponse::NotFound().json(serde_json::json!({
            "error": format!("SMS template for {:?} not found in SettingResource", tpl.sms_type)
        }));
    };

    let resolved_sms_type = if matches!(tpl.sms_type, SmsType::CUSTOM(ref id) if id.is_empty()) {
        SmsType::from_id(&setting_tpl.id)
    } else {
        tpl.sms_type.clone()
    };

    match tpl.render_with_template(&setting_tpl.content) {
        Ok(message) => HttpResponse::Ok().json(serde_json::json!({
            "contacts": contacts,
            "sms_type": resolved_sms_type,
            "template_id": setting_tpl.id,
            "template_name": setting_tpl.name,
            "template_active": setting_tpl.active,
            "placeholders": extract_template_placeholders(&setting_tpl.content),
            "sms_enabled": setting_sms.as_ref().is_none_or(|s| s.enabled),
            "template_source": "setting_resource",
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
    http_req: actix_web::HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let started_at = Instant::now();
    if http_req.extensions().get::<crate::auth::Claims>().is_some() {
        if let crate::permissions::PermissionCheckOutcome::Denied { message, .. } =
            crate::permissions::enforce_request_permission(
                &http_req,
                &s_data,
                "SettingResource",
                crate::permissions::action::MANAGE_SMS,
                crate::permissions::extract_branch_id_from_req(&http_req),
            )
            .await
        {
            return HttpResponse::Forbidden().json(serde_json::json!({ "error": message }));
        }
    }
    let tpl = template.into_inner();
    let setting_sms = find_sms_notification_settings_in_server_data(&s_data).await;
    let cfg = SmsConfig::from_setting_or_env(setting_sms.as_ref());
    let provider = cfg.provider.clone();
    let client = SmsClient::new(cfg);

    let (response, summary, meta) = match client.send_template_with_server_data(&tpl, &s_data).await
    {
        Ok(result) => {
            let meta = serde_json::json!({
                "templateId": tpl.template_id,
                "receiver": result.contacts,
                "shootId": result.shoot_id,
                "provider": provider,
                "vehicleNumber": tpl.vehicle_number,
            });
            let summary = format!("Sent template SMS to {}", result.contacts);
            (HttpResponse::Ok().json(result), summary, meta)
        }
        Err(e) => {
            let msg = e.to_string();
            let resp = if msg.contains("disabled") || msg.contains("inactive") {
                HttpResponse::Forbidden().json(serde_json::json!({ "error": msg }))
            } else if msg.contains("not found in SettingResource") {
                HttpResponse::NotFound().json(serde_json::json!({ "error": msg }))
            } else if msg.contains("is required")
                || msg.contains("exceeds maximum length")
                || msg.contains("empty")
                || msg.contains("Unsupported SMS provider")
            {
                HttpResponse::BadRequest().json(serde_json::json!({ "error": msg }))
            } else {
                HttpResponse::BadGateway().json(serde_json::json!({ "error": msg }))
            };
            let meta = serde_json::json!({
                "templateId": tpl.template_id,
                "receiver": tpl.receiver,
                "provider": provider,
                "vehicleNumber": tpl.vehicle_number,
                "error": msg,
            });
            (resp, format!("Template SMS send failed: {}", msg), meta)
        }
    };

    crate::audit::record_http_audit_async(crate::audit::HttpAuditEvent {
        req: Some(&http_req),
        category: crate::audit::AuditCategory::Notification,
        action: crate::audit::AuditAction::SmsSend,
        status_code: response.status().as_u16(),
        started_at: Some(started_at),
        resource_name: Some("SettingResource".to_string()),
        summary,
        branch_override: Some(tpl.sangh_branch),
        metadata: Some(meta),
        ..Default::default()
    });

    response
}

/// POST /api/v1/sms/send-raw — Sends a raw SMS message to comma-separated contacts via SamayaSMS.
pub async fn send_raw_sms(
    req: web::Json<SendRawSmsRequest>,
    http_req: actix_web::HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let started_at = Instant::now();
    if http_req.extensions().get::<crate::auth::Claims>().is_some() {
        if let crate::permissions::PermissionCheckOutcome::Denied { message, .. } =
            crate::permissions::enforce_request_permission(
                &http_req,
                &s_data,
                "SettingResource",
                crate::permissions::action::MANAGE_SMS,
                crate::permissions::extract_branch_id_from_req(&http_req),
            )
            .await
        {
            return HttpResponse::Forbidden().json(serde_json::json!({ "error": message }));
        }
    }
    let body = req.into_inner();
    let setting_sms = find_sms_notification_settings_in_server_data(&s_data).await;
    let cfg = SmsConfig::from_setting_or_env(setting_sms.as_ref());
    let provider = cfg.provider.clone();
    let client = SmsClient::new(cfg);

    let (response, summary, meta) = match client
        .send_raw(&body.contacts, &body.msg, body.time.as_deref())
        .await
    {
        Ok(result) => {
            let meta = serde_json::json!({
                "contacts": result.contacts,
                "shootId": result.shoot_id,
                "provider": provider,
                "scheduledTime": body.time,
            });
            let summary = format!("Sent raw SMS to {}", result.contacts);
            (HttpResponse::Ok().json(result), summary, meta)
        }
        Err(e) => {
            let msg = e.to_string();
            let resp = if msg.contains("disabled") {
                HttpResponse::Forbidden().json(serde_json::json!({ "error": msg }))
            } else if msg.contains("is required")
                || msg.contains("exceeds maximum length")
                || msg.contains("empty")
                || msg.contains("Unsupported SMS provider")
            {
                HttpResponse::BadRequest().json(serde_json::json!({ "error": msg }))
            } else {
                HttpResponse::BadGateway().json(serde_json::json!({ "error": msg }))
            };
            let meta = serde_json::json!({
                "contacts": body.contacts,
                "provider": provider,
                "scheduledTime": body.time,
                "error": msg,
            });
            (resp, format!("Raw SMS send failed: {}", msg), meta)
        }
    };

    crate::audit::record_http_audit_async(crate::audit::HttpAuditEvent {
        req: Some(&http_req),
        category: crate::audit::AuditCategory::Notification,
        action: crate::audit::AuditAction::SmsSend,
        status_code: response.status().as_u16(),
        started_at: Some(started_at),
        resource_name: Some("SettingResource".to_string()),
        summary,
        metadata: Some(meta),
        ..Default::default()
    });

    response
}

pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.route("/templates", web::get().to(list_sms_templates))
        .route("/templates/{template_id}", web::get().to(get_sms_template_by_id))
        .route("/preview", web::post().to(preview_sms_template))
        .route("/send", web::post().to(send_template_sms))
        .route("/send-raw", web::post().to(send_raw_sms));
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn test_template_validation_errors() {
        let mut cache = Cache::new();
        cache.write(ResourceName::SettingResource, 1, sample_setting_resource());

        // Missing amount & expiry_bs for PAID
        let invalid_paid = SmsTemplate {
            receiver: "9841000000".to_string(),
            sms_type: SmsType::PAID,
            template_id: None,
            vehicle_number: "BA-01-PA-1234".to_string(),
            sangh_branch: "Main Branch".to_string(),
            expiry_bs: None,
            due_date_bs: None,
            amount: None,
            scheduled_time: None,
            variables: HashMap::new(),
        };
        assert!(invalid_paid.render_with_cache(&cache).is_err());

        // Missing due_date_bs for PAYMENT_REMAINDER
        let invalid_rem = SmsTemplate {
            receiver: "9841000000".to_string(),
            sms_type: SmsType::PAYMENT_REMAINDER,
            template_id: None,
            vehicle_number: "BA-01-PA-1234".to_string(),
            sangh_branch: "Main Branch".to_string(),
            expiry_bs: None,
            due_date_bs: None,
            amount: None,
            scheduled_time: None,
            variables: HashMap::new(),
        };
        assert!(invalid_rem.render_with_cache(&cache).is_err());

        // Exceeds 720 chars in SettingResource template
        let mut max_len_cache = Cache::new();
        max_len_cache.write(
            ResourceName::SettingResource,
            2,
            DatabaseResource {
                id: Some(2),
                resource_name: Some(ResourceName::SettingResource),
                inner_content: Some(serde_json::json!({
                    "smsSettings": {
                        "templates": [{
                            "id": "payment-received",
                            "active": true,
                            "channel": "SMS",
                            "content": "a".repeat(MAX_SMS_CHAR_LEN + 1)
                        }]
                    }
                })),
                external_content: None,
            },
        );
        let too_long =
            SmsTemplate::new_paid("9841000000", "V1", "Branch", 100.0, "2083-01-01");
        assert!(too_long.render_with_cache(&max_len_cache).is_err());
    }

    #[test]
    fn test_json_deserialization_numeric_or_string_receiver() {
        let mut cache = Cache::new();
        cache.write(ResourceName::SettingResource, 1, sample_setting_resource());

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
        assert!(tpl.render_with_cache(&cache).is_ok());

        let json_alias = r#"{
            "receiver": "9841234567,9801234567",
            "sms_type": "PAYMENT_REMINDER",
            "vehicle_number": "BA-01-PA-1234",
            "sangh_branch": "Main",
            "amount": 500,
            "due_day_bs": "10"
        }"#;
        let tpl2: SmsTemplate = serde_json::from_str(json_alias).unwrap();
        assert_eq!(tpl2.sms_type, SmsType::PAYMENT_REMAINDER);
        assert_eq!(tpl2.due_date_bs.as_deref(), Some("10"));
        assert!(tpl2.render_with_cache(&cache).is_ok());
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
            ..SmsConfig::default()
        });

        let mut cache = Cache::new();
        cache.write(ResourceName::SettingResource, 1, sample_setting_resource());

        let tpl = SmsTemplate::new_paid(
            "9841111111",
            "BA-01-PA-9999",
            "Main Branch",
            2500.0,
            "2083-10-15",
        )
        .with_scheduled_time("2026-11-19 14:30");

        let result = client
            .send_template_with_cache(&tpl, &cache)
            .await
            .expect("SMS send should succeed");
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
            ..SmsConfig::default()
        });

        let err = client
            .send_raw("9841111111", "Hello test", None)
            .await
            .expect_err("HTTP 429 should return Err");
        assert!(err.to_string().contains("429"));
        assert!(err.to_string().contains("Rate limit exceeded"));
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
            "9841000000, 9801000000",
            "BA-01-PA-1234",
            "Loadboard sthaniya",
            "2083-09-20",
        );
        assert_eq!(
            urgent_tpl.normalized_contacts().unwrap(),
            "9841000000,9801000000"
        );
        assert_eq!(
            urgent_tpl.render_with_cache(&cache).unwrap(),
            "अत्यावश्यक सूचना: सवारी नम्बर BA-01-PA-1234 को भुक्तानी म्याद 2083-09-20 मा सकिँदैछ। सेवा निरन्तरताका लागि समयमै भुक्तानी गर्नुहोस्।\nLoadboard sthaniya"
        );

        // Errors when SettingResource is absent in cache (no hardcoded fallback)
        let empty_cache = Cache::new();
        assert!(paid_tpl.render_with_cache(&empty_cache).is_err());
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

    fn nested_notifications_setting(enabled: bool, webhook_url: &str, provider: &str) -> DatabaseResource {
        DatabaseResource {
            id: Some(10),
            resource_name: Some(ResourceName::SettingResource),
            inner_content: Some(serde_json::json!({
                "integrations": { "tqms": { "SanghBranch": "Loadboard sthaniya" } },
                "notifications": {
                    "sms": {
                        "apiKey": "36A1291E165814",
                        "baseUrl": "https://samayasms.com.np/smsapi/index.php",
                        "enabled": enabled,
                        "provider": provider,
                        "senderId": "",
                        "apiSecret": "SECRET_999",
                        "templates": [
                            {
                                "id": "PAID",
                                "name": "भुक्तानी प्राप्त",
                                "active": true,
                                "content": "Test gariyeko template तपाईंको सवारी नम्बर {vehicleNumber} को रकम रु {amount} प्राप्त भएको छ।\nधन्यवाद। {organizationName}\nम्याद सकिने मिति: {expiryDate}"
                            },
                            {
                                "id": "PAYMENT REMAINDER",
                                "name": "भुक्तानी रिमाइन्डर",
                                "active": true,
                                "content": "नमस्कार, सवारी नम्बर {vehicleNumber} को रु {amount} भुक्तानी बाँकी छ। कृपया {dueDate} भित्र भुक्तानी गर्नुहोस्।\n{organizationName}"
                            },
                            {
                                "id": "URGENT",
                                "name": "अत्यावश्यक रिमाइन्डर",
                                "active": false,
                                "content": "अत्यावश्यक सूचना: सवारी नम्बर {vehicleNumber} को भुक्तानी म्याद {dueDate} मा सकिँदैछ। सेवा निरन्तरताका लागि समयमै भुक्तानी गर्नुहोस्।\n{organizationName}"
                            }
                        ],
                        "webhookUrl": webhook_url,
                        "defaultCountryCode": "+977"
                    }
                }
            })),
            external_content: None,
        }
    }

    #[test]
    fn test_nested_notifications_sms_templates_and_country_code() {
        let mut cache = Cache::new();
        cache.write(
            ResourceName::SettingResource,
            10,
            nested_notifications_setting(true, "", "samayasms"),
        );

        let paid = SmsTemplate::new_paid("9841000000", "BA-01-PA-1", "Loadboard", 100.0, "2083-12-30");
        assert!(paid.render_with_cache(&cache).unwrap().starts_with("Test gariyeko template"));

        let rem = SmsTemplate::new_payment_remainder("9841000000", "BA-01-PA-1", "Loadboard", "2083-12-30")
            .with_amount(250.0);
        assert!(rem.render_with_cache(&cache).unwrap().contains("रु 250"));

        let urg = SmsTemplate::new_urgent("9841000000", "BA-01-PA-1", "Loadboard", "2083-12-30");
        assert!(urg.render_with_cache(&cache).unwrap().contains("अत्यावश्यक सूचना"));

        assert_eq!(
            normalize_contacts_with_country_code("9861282176, 09841000000, +9779801112233", Some("+977"))
                .unwrap(),
            "9779861282176,9779841000000,9779801112233"
        );
    }

    #[actix_web::test]
    async fn test_notifications_sms_enabled_active_provider_secret_and_webhook_enforcement() {
        let (mock_url, req_rx) = spawn_mock_sms_server("200 OK", "SMS-SHOOT-ID/SHOOT-NESTED").await;
        let (webhook_url, webhook_rx) = spawn_mock_sms_server("200 OK", "OK").await;

        let mut cache = Cache::new();
        // 1. Disabled SMS blocks sending (even for active template)
        cache.write(
            ResourceName::SettingResource,
            10,
            nested_notifications_setting(false, "", "samayasms"),
        );
        let client_disabled = SmsClient::new(SmsConfig::from_setting_or_env(
            extract_sms_notification_settings(cache.read_all(ResourceName::SettingResource).unwrap()).as_ref(),
        ));
        let paid = SmsTemplate::new_paid("9861282176", "BA-01-PA-1", "Loadboard", 0.0, "2083-12-30");
        let disabled_err = client_disabled
            .send_template_with_cache(&paid, &cache)
            .await
            .unwrap_err();
        assert!(disabled_err.to_string().contains("disabled"));

        // 2. Enabled SMS, inactive template ("URGENT", active: false) blocked on send
        cache.write(
            ResourceName::SettingResource,
            10,
            nested_notifications_setting(true, &webhook_url, "samayasms"),
        );
        let mut cfg = SmsConfig::from_setting_or_env(
            extract_sms_notification_settings(cache.read_all(ResourceName::SettingResource).unwrap()).as_ref(),
        );
        cfg.base_url = mock_url;
        let client = SmsClient::new(cfg);

        let urg = SmsTemplate::new_urgent("9861282176", "BA-01-PA-1", "Loadboard", "2083-12-30");
        let inactive_err = client.send_template_with_cache(&urg, &cache).await.unwrap_err();
        assert!(inactive_err.to_string().contains("inactive"));

        // 3. Unsupported provider rejected
        let mut bad_provider_cfg = SmsConfig::default();
        bad_provider_cfg.provider = "twilio".to_string();
        let bad_provider_err = SmsClient::new(bad_provider_cfg)
            .send_raw("9861282176", "Hi", None)
            .await
            .unwrap_err();
        assert!(bad_provider_err.to_string().contains("Unsupported SMS provider"));

        // 4. Valid active PAID template sends with +977 country code, apiSecret, and triggers webhook
        let res = client
            .send_template_with_cache(&paid, &cache)
            .await
            .expect("active PAID template should send");
        assert_eq!(res.contacts, "9779861282176");
        assert_eq!(res.shoot_id, "SHOOT-NESTED");

        let raw_http = req_rx.await.unwrap();
        assert!(raw_http.contains("key=36A1291E165814"));
        assert!(raw_http.contains("secret=SECRET_999"));
        assert!(raw_http.contains("contacts=9779861282176"));
        assert!(
            !raw_http.contains("routeid="),
            "custom apiKey without routeId must not send default routeid=10244"
        );
        assert!(
            !raw_http.contains("campaign="),
            "custom apiKey without campaignId must not send default campaign=8572"
        );

        let webhook_http = tokio::time::timeout(Duration::from_secs(2), webhook_rx)
            .await
            .expect("webhook should fire")
            .unwrap();
        assert!(webhook_http.contains("SHOOT-NESTED"));
        assert!(webhook_http.contains("9779861282176"));
    }

    async fn spawn_multi_mock_sms_server(
        expected_requests: usize,
        response_prefix: &'static str,
    ) -> (String, tokio::sync::mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::mpsc::channel(expected_requests.max(4));

        tokio::spawn(async move {
            for idx in 1..=expected_requests {
                if let Ok((mut socket, _)) = listener.accept().await {
                    let mut buf = vec![0u8; 8192];
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    let req_str = String::from_utf8_lossy(&buf[..n]).to_string();
                    let _ = tx.send(req_str).await;

                    let body = format!("{response_prefix}-{idx}");
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                    let _ = socket.flush().await;
                }
            }
        });

        (format!("http://{addr}/smsapi/index.php"), rx)
    }

    fn user_exact_notifications_setting(base_url: &str) -> DatabaseResource {
        DatabaseResource {
            id: Some(1),
            resource_name: Some(ResourceName::SettingResource),
            inner_content: Some(serde_json::json!({
                "schemaVersion": 1,
                "notifications": {
                    "sms": {
                        "apiKey": "36A1291E165814",
                        "baseUrl": base_url,
                        "enabled": true,
                        "provider": "samayasms",
                        "senderId": "Bit_Alert",
                        "apiSecret": "",
                        "templates": [
                            {
                                "id": "PAID",
                                "name": "भुक्तानी प्राप्त",
                                "active": true,
                                "content": "Test gariyeko template तपाईंको सवारी नम्बर {vehicleNumber} को रकम रु {amount} प्राप्त भएको छ।\nधन्यवाद। {organizationName}\nम्याद सकिने मिति: {expiryDate}"
                            },
                            {
                                "id": "PAYMENT REMAINDER",
                                "name": "भुक्तानी रिमाइन्डर",
                                "active": true,
                                "content": "नमस्कार, सवारी नम्बर {vehicleNumber} को रु {amount} भुक्तानी बाँकी छ। कृपया {dueDate} भित्र भुक्तानी गर्नुहोस्।\n{organizationName}"
                            },
                            {
                                "id": "URGENT",
                                "name": "अत्यावश्यक रिमाइन्डर",
                                "active": false,
                                "content": "अत्यावश्यक सूचना: सवारी नम्बर {vehicleNumber} को भुक्तानी म्याद {dueDate} मा सकिँदैछ। सेवा निरन्तरताका लागि समयमै भुक्तानी गर्नुहोस्।\n{organizationName}"
                            }
                        ],
                        "webhookUrl": "",
                        "defaultCountryCode": "+977"
                    }
                }
            })),
            external_content: None,
        }
    }

    #[actix_web::test]
    async fn test_generate_and_render_templates_from_user_notifications_sms_setting_resource() {
        let mut state = crate::access::test_helpers::MockDbState::default();
        let initial_setting =
            user_exact_notifications_setting("https://samayasms.com.np/smsapi/index.php");
        state.seed(ResourceName::SettingResource, initial_setting.clone());
        let (s_data, _) =
            crate::access::test_helpers::create_mock_server_data_with_state(state).await;

        let app = actix_web::test::init_service(
            actix_web::App::new()
                .app_data(s_data.clone())
                .service(web::scope("/api/v1/sms").configure(configure_routes)),
        )
        .await;

        // 1. GET /api/v1/sms/templates generates all 3 templates from SettingResource (DB fallback works on cold cache)
        let list_resp = actix_web::test::call_service(
            &app,
            actix_web::test::TestRequest::get()
                .uri("/api/v1/sms/templates")
                .to_request(),
        )
        .await;
        assert_eq!(list_resp.status(), actix_web::http::StatusCode::OK);
        let list_body: Value = actix_web::test::read_body_json(list_resp).await;
        assert_eq!(list_body["enabled"], true);
        assert_eq!(list_body["provider"], "samayasms");
        assert_eq!(list_body["senderId"], "Bit_Alert");
        assert_eq!(list_body["defaultCountryCode"], "+977");
        assert!(list_body.get("apiKey").is_none(), "apiKey must not be exposed");
        assert!(
            list_body.get("apiSecret").is_none(),
            "apiSecret must not be exposed"
        );

        let templates = list_body["templates"].as_array().unwrap();
        assert_eq!(templates.len(), 3);
        assert_eq!(templates[0]["id"], "PAID");
        assert_eq!(templates[0]["active"], true);
        assert_eq!(templates[0]["sms_type"], "PAID");
        assert_eq!(
            templates[0]["placeholders"],
            serde_json::json!(["vehicleNumber", "amount", "organizationName", "expiryDate"])
        );

        assert_eq!(templates[1]["id"], "PAYMENT REMAINDER");
        assert_eq!(templates[1]["active"], true);
        assert_eq!(templates[1]["sms_type"], "PAYMENT_REMAINDER");
        assert_eq!(
            templates[1]["placeholders"],
            serde_json::json!(["vehicleNumber", "amount", "dueDate", "organizationName"])
        );

        assert_eq!(templates[2]["id"], "URGENT");
        assert_eq!(templates[2]["active"], false);
        assert_eq!(templates[2]["sms_type"], "URGENT");
        assert_eq!(
            templates[2]["placeholders"],
            serde_json::json!(["vehicleNumber", "dueDate", "organizationName"])
        );

        // 2. GET /api/v1/sms/templates/{template_id} (including URL-encoded space in "PAYMENT REMAINDER")
        for (uri, expected_id, expected_active) in [
            ("/api/v1/sms/templates/PAID", "PAID", true),
            (
                "/api/v1/sms/templates/PAYMENT%20REMAINDER",
                "PAYMENT REMAINDER",
                true,
            ),
            ("/api/v1/sms/templates/URGENT", "URGENT", false),
        ] {
            let resp = actix_web::test::call_service(
                &app,
                actix_web::test::TestRequest::get().uri(uri).to_request(),
            )
            .await;
            assert_eq!(resp.status(), actix_web::http::StatusCode::OK);
            let tpl_json: Value = actix_web::test::read_body_json(resp).await;
            assert_eq!(tpl_json["id"], expected_id);
            assert_eq!(tpl_json["active"], expected_active);
        }

        let missing_resp = actix_web::test::call_service(
            &app,
            actix_web::test::TestRequest::get()
                .uri("/api/v1/sms/templates/DOES_NOT_EXIST")
                .to_request(),
        )
        .await;
        assert_eq!(missing_resp.status(), actix_web::http::StatusCode::NOT_FOUND);

        // 3. POST /api/v1/sms/preview works for PAID, PAYMENT REMAINDER, and inactive URGENT
        let preview_paid = actix_web::test::call_service(
            &app,
            actix_web::test::TestRequest::post()
                .uri("/api/v1/sms/preview")
                .set_json(serde_json::json!({
                    "receiver": "9861282176",
                    "sms_type": "PAID",
                    "vehicleNumber": "LU-01-KA-1234",
                    "organizationName": "Narayani Sangh",
                    "amount": 1500,
                    "expiryDate": "2083-12-30"
                }))
                .to_request(),
        )
        .await;
        assert_eq!(preview_paid.status(), actix_web::http::StatusCode::OK);
        let paid_body: Value = actix_web::test::read_body_json(preview_paid).await;
        assert_eq!(paid_body["contacts"], "9779861282176");
        assert_eq!(paid_body["template_id"], "PAID");
        assert_eq!(paid_body["template_active"], true);
        assert_eq!(
            paid_body["message"],
            "Test gariyeko template तपाईंको सवारी नम्बर LU-01-KA-1234 को रकम रु 1500 प्राप्त भएको छ।\nधन्यवाद। Narayani Sangh\nम्याद सकिने मिति: 2083-12-30"
        );

        let preview_rem = actix_web::test::call_service(
            &app,
            actix_web::test::TestRequest::post()
                .uri("/api/v1/sms/preview")
                .set_json(serde_json::json!({
                    "receiver": "9861282176",
                    "sms_type": "PAYMENT REMAINDER",
                    "variables": {
                        "vehicleNumber": "LU-01-KA-1234",
                        "amount": 2500,
                        "dueDate": "2083-12-25",
                        "organizationName": "Narayani Sangh"
                    }
                }))
                .to_request(),
        )
        .await;
        assert_eq!(preview_rem.status(), actix_web::http::StatusCode::OK);
        let rem_body: Value = actix_web::test::read_body_json(preview_rem).await;
        assert_eq!(rem_body["template_id"], "PAYMENT REMAINDER");
        assert_eq!(
            rem_body["message"],
            "नमस्कार, सवारी नम्बर LU-01-KA-1234 को रु 2500 भुक्तानी बाँकी छ। कृपया 2083-12-25 भित्र भुक्तानी गर्नुहोस्।\nNarayani Sangh"
        );

        let preview_urg = actix_web::test::call_service(
            &app,
            actix_web::test::TestRequest::post()
                .uri("/api/v1/sms/preview")
                .set_json(serde_json::json!({
                    "receiver": "9861282176",
                    "sms_type": "URGENT",
                    "vehicle_number": "LU-01-KA-1234",
                    "sangh_branch": "Narayani Sangh",
                    "due_date_bs": "2083-12-28"
                }))
                .to_request(),
        )
        .await;
        assert_eq!(preview_urg.status(), actix_web::http::StatusCode::OK);
        let urg_body: Value = actix_web::test::read_body_json(preview_urg).await;
        assert_eq!(urg_body["template_id"], "URGENT");
        assert_eq!(urg_body["template_active"], false);

        // 4. POST /api/v1/sms/send using cache-populated SettingResource pointed to mock SamayaSMS server
        let (mock_url, mut rx) = spawn_multi_mock_sms_server(2, "SMS-SHOOT-ID/USER").await;
        s_data.cache.write().await.write(
            ResourceName::SettingResource,
            1,
            user_exact_notifications_setting(&mock_url),
        );

        let send_paid = actix_web::test::call_service(
            &app,
            actix_web::test::TestRequest::post()
                .uri("/api/v1/sms/send")
                .set_json(serde_json::json!({
                    "receiver": "9861282176",
                    "sms_type": "PAID",
                    "vehicleNumber": "LU-01-KA-1234",
                    "organizationName": "Narayani Sangh",
                    "amount": 1500,
                    "expiryDate": "2083-12-30"
                }))
                .to_request(),
        )
        .await;
        assert_eq!(send_paid.status(), actix_web::http::StatusCode::OK);
        let send_paid_res: SmsSendResult = actix_web::test::read_body_json(send_paid).await;
        assert_eq!(send_paid_res.shoot_id, "USER-1");
        assert_eq!(send_paid_res.contacts, "9779861282176");

        let raw_paid_http = rx.recv().await.unwrap();
        assert!(raw_paid_http.contains("key=36A1291E165814"));
        assert!(raw_paid_http.contains("senderid=Bit_Alert"));
        assert!(raw_paid_http.contains("type=unicode"));
        assert!(raw_paid_http.contains("contacts=9779861282176"));
        assert!(raw_paid_http.contains("msg=Test+gariyeko+template+"));
        assert!(
            !raw_paid_http.contains("routeid="),
            "custom SettingResource apiKey without routeId must not inject default routeid"
        );
        assert!(
            !raw_paid_http.contains("campaign="),
            "custom SettingResource apiKey without campaignId must not inject default campaign"
        );

        let send_rem = actix_web::test::call_service(
            &app,
            actix_web::test::TestRequest::post()
                .uri("/api/v1/sms/send")
                .set_json(serde_json::json!({
                    "receiver": "9861282176",
                    "sms_type": "PAYMENT REMAINDER",
                    "vehicleNumber": "LU-01-KA-1234",
                    "organizationName": "Narayani Sangh",
                    "amount": 2500,
                    "dueDate": "2083-12-25"
                }))
                .to_request(),
        )
        .await;
        assert_eq!(send_rem.status(), actix_web::http::StatusCode::OK);
        let send_rem_res: SmsSendResult = actix_web::test::read_body_json(send_rem).await;
        assert_eq!(send_rem_res.shoot_id, "USER-2");
        let raw_rem_http = rx.recv().await.unwrap();
        assert!(raw_rem_http.contains("senderid=Bit_Alert"));

        // URGENT is active: false -> 403 Forbidden
        let send_urg = actix_web::test::call_service(
            &app,
            actix_web::test::TestRequest::post()
                .uri("/api/v1/sms/send")
                .set_json(serde_json::json!({
                    "receiver": "9861282176",
                    "sms_type": "URGENT",
                    "vehicleNumber": "LU-01-KA-1234",
                    "organizationName": "Narayani Sangh",
                    "dueDate": "2083-12-28"
                }))
                .to_request(),
        )
        .await;
        assert_eq!(send_urg.status(), actix_web::http::StatusCode::FORBIDDEN);
    }

    #[actix_web::test]
    async fn test_dynamic_custom_template_generated_from_setting_resource() {
        let mut setting = user_exact_notifications_setting(DEFAULT_SMS_BASE_URL);
        if let Some(arr) = setting
            .inner_content
            .as_mut()
            .and_then(|v| v.pointer_mut("/notifications/sms/templates"))
            .and_then(Value::as_array_mut)
        {
            arr.push(serde_json::json!({
                "id": "CUSTOM_WELCOME",
                "name": "Custom Welcome",
                "active": true,
                "content": "Namaste {memberName}, your code is {otp} for {organizationName}"
            }));
        }

        let (s_data, _) = crate::access::test_helpers::create_mock_server_data_with_state(
            crate::access::test_helpers::MockDbState::default(),
        )
        .await;
        s_data
            .cache
            .write()
            .await
            .write(ResourceName::SettingResource, 1, setting);

        let generated = generate_templates_from_cache(&*s_data.cache.read().await);
        let custom = generated
            .iter()
            .find(|t| t.id == "CUSTOM_WELCOME")
            .expect("CUSTOM_WELCOME template should be generated");
        assert_eq!(
            custom.placeholders,
            vec!["memberName", "otp", "organizationName"]
        );
        assert_eq!(
            custom.sms_type,
            SmsType::CUSTOM("CUSTOM_WELCOME".to_string())
        );

        let mut vars = HashMap::new();
        vars.insert("memberName".to_string(), Value::String("Sita".to_string()));
        vars.insert("otp".to_string(), serde_json::json!(4321));
        vars.insert(
            "organizationName".to_string(),
            Value::String("Narayani".to_string()),
        );
        assert_eq!(
            custom.render_with_variables(&vars).unwrap(),
            "Namaste Sita, your code is 4321 for Narayani"
        );

        let app = actix_web::test::init_service(
            actix_web::App::new()
                .app_data(s_data)
                .service(web::scope("/api/v1/sms").configure(configure_routes)),
        )
        .await;

        // Render via template_id without sms_type
        let preview_by_tid = actix_web::test::call_service(
            &app,
            actix_web::test::TestRequest::post()
                .uri("/api/v1/sms/preview")
                .set_json(serde_json::json!({
                    "receiver": "9861282176",
                    "template_id": "CUSTOM_WELCOME",
                    "variables": {
                        "memberName": "Hari",
                        "otp": 9876,
                        "organizationName": "Narayani Sangh"
                    }
                }))
                .to_request(),
        )
        .await;
        assert_eq!(preview_by_tid.status(), actix_web::http::StatusCode::OK);
        let body_by_tid: Value = actix_web::test::read_body_json(preview_by_tid).await;
        assert_eq!(body_by_tid["sms_type"], "CUSTOM_WELCOME");
        assert_eq!(
            body_by_tid["message"],
            "Namaste Hari, your code is 9876 for Narayani Sangh"
        );

        // Render via sms_type: "CUSTOM_WELCOME"
        let preview_by_type = actix_web::test::call_service(
            &app,
            actix_web::test::TestRequest::post()
                .uri("/api/v1/sms/preview")
                .set_json(serde_json::json!({
                    "receiver": "9861282176",
                    "sms_type": "CUSTOM_WELCOME",
                    "variables": {
                        "memberName": "Gita",
                        "otp": "1122",
                        "organizationName": "Narayani Sangh"
                    }
                }))
                .to_request(),
        )
        .await;
        assert_eq!(preview_by_type.status(), actix_web::http::StatusCode::OK);
        let body_by_type: Value = actix_web::test::read_body_json(preview_by_type).await;
        assert_eq!(
            body_by_type["message"],
            "Namaste Gita, your code is 1122 for Narayani Sangh"
        );

        // Missing custom placeholder -> 400 Bad Request
        let missing_otp = actix_web::test::call_service(
            &app,
            actix_web::test::TestRequest::post()
                .uri("/api/v1/sms/preview")
                .set_json(serde_json::json!({
                    "receiver": "9861282176",
                    "template_id": "CUSTOM_WELCOME",
                    "variables": {
                        "memberName": "Hari",
                        "organizationName": "Narayani Sangh"
                    }
                }))
                .to_request(),
        )
        .await;
        assert_eq!(missing_otp.status(), actix_web::http::StatusCode::BAD_REQUEST);
    }

    #[actix_web::test]
    async fn test_sms_config_route_and_campaign_resolution_for_custom_api_key() {
        // 1. Default account API key retains default route & campaign when none specified
        let (default_route, default_campaign) =
            resolve_route_and_campaign(DEFAULT_SMS_API_KEY, None, None, None, None);
        assert_eq!(default_route.as_deref(), Some(DEFAULT_SMS_ROUTE_ID));
        assert_eq!(default_campaign.as_deref(), Some(DEFAULT_SMS_CAMPAIGN_ID));

        // 2. Custom API key without routeId/campaignId omits both (prevents "Pricing is not set for your account for this route")
        let (custom_route, custom_campaign) =
            resolve_route_and_campaign("36A1291E165814", None, None, None, None);
        assert_eq!(custom_route, None);
        assert_eq!(custom_campaign, None);

        // 3. Env overrides apply to custom API key when SettingResource omits them
        let (env_route, env_campaign) = resolve_route_and_campaign(
            "36A1291E165814",
            None,
            None,
            Some("30100".to_string()),
            Some("7788".to_string()),
        );
        assert_eq!(env_route.as_deref(), Some("30100"));
        assert_eq!(env_campaign.as_deref(), Some("7788"));

        // 4. SettingResource routeId/campaignId override env
        let (setting_route, setting_campaign) = resolve_route_and_campaign(
            "36A1291E165814",
            Some("20500"),
            Some("9901"),
            Some("30100".to_string()),
            Some("7788".to_string()),
        );
        assert_eq!(setting_route.as_deref(), Some("20500"));
        assert_eq!(setting_campaign.as_deref(), Some("9901"));

        // 5. Numeric and string routeId/campaignId in SettingResource.notifications.sms deserialize & send
        let (mock_url, req_rx) =
            spawn_mock_sms_server("200 OK", "SMS-SHOOT-ID/CUSTOM-ROUTE-OK").await;
        let setting = DatabaseResource {
            id: Some(1),
            resource_name: Some(ResourceName::SettingResource),
            inner_content: Some(serde_json::json!({
                "schemaVersion": 1,
                "notifications": {
                    "sms": {
                        "apiKey": "36A1291E165814",
                        "baseUrl": mock_url,
                        "enabled": true,
                        "provider": "samayasms",
                        "senderId": "Bit_Alert",
                        "routeId": 20500,
                        "campaignId": "9901",
                        "defaultCountryCode": "+977",
                        "templates": [
                            {
                                "id": "PAID",
                                "active": true,
                                "content": "Paid {vehicleNumber} {amount} {organizationName} {expiryDate}"
                            }
                        ]
                    }
                }
            })),
            external_content: None,
        };

        let sms_settings = extract_sms_notification_settings([&setting]).unwrap();
        assert_eq!(sms_settings.route_id.as_deref(), Some("20500"));
        assert_eq!(sms_settings.campaign_id.as_deref(), Some("9901"));

        let cfg = SmsConfig::from_setting_or_env(Some(&sms_settings));
        assert_eq!(cfg.api_key, "36A1291E165814");
        assert_eq!(cfg.route_id.as_deref(), Some("20500"));
        assert_eq!(cfg.campaign_id.as_deref(), Some("9901"));

        let mut cache = Cache::new();
        cache.write(ResourceName::SettingResource, 1, setting);

        let client = SmsClient::new(cfg);
        let tpl = SmsTemplate::new_paid("9861282176", "BA-01-PA-1", "Narayani", 500.0, "2083-12-30");
        let sent = client.send_template_with_cache(&tpl, &cache).await.unwrap();
        assert_eq!(sent.shoot_id, "CUSTOM-ROUTE-OK");

        let raw_http = req_rx.await.unwrap();
        assert!(raw_http.contains("key=36A1291E165814"));
        assert!(raw_http.contains("routeid=20500"));
        assert!(raw_http.contains("campaign=9901"));
        assert!(raw_http.contains("senderid=Bit_Alert"));
    }
}
