use anyhow::{anyhow, bail, Context, Result};
use narayani_core::resource::{DatabaseResource, ResourceName};
pub use narayani_core::{extract_template_placeholders, SettingSmsTemplate, SmsType};
use narayani_storage::Cache;
use reqwest::Client;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::HashMap;
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

pub fn raw_sms_templates_from_value(inner: &Value) -> Option<&Vec<Value>> {
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


/// Extracts `SmsNotificationSettings` from `SettingResource` items (preferring `notifications.sms`,
/// falling back to legacy `smsSettings`).
pub fn extract_sms_notification_settings<'a, I>(
    settings: I,
) -> Option<narayani_settings::SmsNotificationSettings>
where
    I: IntoIterator<Item = &'a DatabaseResource>,
{
    let mut best: Option<(i32, narayani_settings::SmsNotificationSettings)> = None;
    for res in settings {
        let Some(inner) = res.inner_content.as_ref() else {
            continue;
        };
        if inner.pointer("/notifications/sms").is_some()
            || inner.get("smsSettings").is_some()
            || inner.get("sms_settings").is_some()
        {
            let res_id = res.id.unwrap_or(0);
            let parsed = narayani_settings::AppSettings::from_inner_value(inner)
                .notifications
                .sms;
            if best.as_ref().is_none_or(|(best_id, _)| res_id >= *best_id) {
                best = Some((res_id, parsed));
            }
        }
    }
    best.map(|(_, s)| s)
}


/// Allows `receiver` in JSON to be supplied either as a string (`"9841XXXXXX"` or
/// `"9841XXXXXX,9801XXXXXX"`) or as a JSON integer (`9841123456`).
pub fn deserialize_receiver<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
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

pub fn resolve_route_and_campaign(
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
        setting_sms: Option<&narayani_settings::SmsNotificationSettings>,
    ) -> Self {
        Self::from_env_and_setting(setting_sms)
    }

    fn from_env_and_setting(
        setting_sms: Option<&narayani_settings::SmsNotificationSettings>,
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
    pub config: SmsConfig,
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

    pub async fn send_raw_with_contacts(
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

