use crate::actions::is_admin;
use crate::cache::Cache;
use crate::dashboard_statistics::{
    ad_date_str_to_bs_ymd, extract_bill_inferred_fiscal_year,
    extract_bill_period_start_fiscal_year, extract_bill_vehicle, extract_string_or_int,
    is_bill_ignored, normalize_vehicle_number, parse_fiscal_year_start,
};
use crate::date::DateMapper;
use crate::resource::{DatabaseResource, ResourceName};
use crate::sms::SettingSmsTemplate;
use crate::ServerData;
use actix_web::{web, HttpRequest, HttpResponse};
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, Utc};
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

fn default_true() -> bool {
    true
}

fn default_schema_version() -> u32 {
    1
}

fn default_vat_rate() -> f64 {
    13.0
}

fn default_cancel_hours() -> u64 {
    24
}

fn default_min_paid_rules() -> usize {
    1
}

fn default_years_back() -> u32 {
    4
}

fn default_years_forward() -> u32 {
    1
}

fn default_sms_provider() -> String {
    "samayasms".to_string()
}

fn deserialize_opt_string_id<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let val = Option::<Value>::deserialize(deserializer)?;
    Ok(val.and_then(|v| match v {
        Value::Null => None,
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }))
}

fn deserialize_string_id<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let val = Value::deserialize(deserializer)?;
    match val {
        Value::String(s) => Ok(s.trim().to_string()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Null => Ok(String::new()),
        other => Err(de::Error::custom(format!(
            "expected string or numeric id, got {other}"
        ))),
    }
}

fn deserialize_string_id_vec<'de, D>(deserializer: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let val = Option::<Value>::deserialize(deserializer)?;
    let Some(Value::Array(items)) = val else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        match item {
            Value::String(s) => {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    out.push(trimmed.to_string());
                }
            }
            Value::Number(n) => out.push(n.to_string()),
            _ => {}
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VatSettings {
    #[serde(default = "default_vat_rate")]
    pub rate: f64,
    #[serde(default)]
    pub enabled: bool,
    #[serde(
        rename = "billingEntityIds",
        default,
        deserialize_with = "deserialize_string_id_vec"
    )]
    pub billing_entity_ids: Vec<String>,
}

impl Default for VatSettings {
    fn default() -> Self {
        Self {
            rate: default_vat_rate(),
            enabled: false,
            billing_entity_ids: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MonthlyPaidArrearsSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(
        rename = "fiscalYear",
        alias = "fiscalYearId",
        default,
        deserialize_with = "deserialize_opt_string_id"
    )]
    pub fiscal_year: Option<String>,
    #[serde(
        rename = "billingRuleId",
        default,
        deserialize_with = "deserialize_opt_string_id"
    )]
    pub billing_rule_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PaidExpiredArrearsSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(
        rename = "billingRuleId",
        default,
        deserialize_with = "deserialize_opt_string_id"
    )]
    pub billing_rule_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ArrearsSettings {
    #[serde(rename = "monthlyPaid", default)]
    pub monthly_paid: MonthlyPaidArrearsSettings,
    #[serde(rename = "paidExpired", default)]
    pub paid_expired: PaidExpiredArrearsSettings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkedFeeRule {
    #[serde(default, deserialize_with = "deserialize_string_id")]
    pub id: String,
    #[serde(default = "default_true")]
    pub active: bool,
    #[serde(
        rename = "fiscalYear",
        alias = "fiscalYearId",
        default,
        deserialize_with = "deserialize_opt_string_id"
    )]
    pub fiscal_year: Option<String>,
    #[serde(rename = "linkedFeeHeadId", deserialize_with = "deserialize_string_id")]
    pub linked_fee_head_id: String,
    #[serde(rename = "sourceFeeHeadId", deserialize_with = "deserialize_string_id")]
    pub source_fee_head_id: String,
    #[serde(rename = "oncePerFiscalYear", default)]
    pub once_per_fiscal_year: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LinkedFeeRulesSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub rules: Vec<LinkedFeeRule>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct BillingSettings {
    #[serde(default)]
    pub vat: VatSettings,
    #[serde(default)]
    pub arrears: ArrearsSettings,
    #[serde(rename = "linkedFeeRules", default)]
    pub linked_fee_rules: LinkedFeeRulesSettings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptCancellationSettings {
    #[serde(default = "default_cancel_hours")]
    pub hours: u64,
    #[serde(default)]
    pub enabled: bool,
}

impl Default for ReceiptCancellationSettings {
    fn default() -> Self {
        Self {
            hours: default_cancel_hours(),
            enabled: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ReceiptsSettings {
    #[serde(default)]
    pub cancellation: ReceiptCancellationSettings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InsuranceEligibilitySettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(
        rename = "ruleIds",
        default,
        deserialize_with = "deserialize_string_id_vec"
    )]
    pub rule_ids: Vec<String>,
    #[serde(rename = "minPaidRules", default = "default_min_paid_rules")]
    pub min_paid_rules: usize,
}

impl Default for InsuranceEligibilitySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            rule_ids: Vec::new(),
            min_paid_rules: default_min_paid_rules(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct InsuranceSettings {
    #[serde(default)]
    pub eligibility: InsuranceEligibilitySettings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FiscalYearSelectorSettings {
    #[serde(rename = "yearsBack", default = "default_years_back")]
    pub years_back: u32,
    #[serde(rename = "yearsForward", default = "default_years_forward")]
    pub years_forward: u32,
    #[serde(
        rename = "archivedYears",
        default,
        deserialize_with = "deserialize_string_id_vec"
    )]
    pub archived_years: Vec<String>,
}

impl Default for FiscalYearSelectorSettings {
    fn default() -> Self {
        Self {
            years_back: default_years_back(),
            years_forward: default_years_forward(),
            archived_years: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FiscalYearAccessEntry {
    pub status: String,
    #[serde(rename = "updatedAt", default)]
    pub updated_at: Option<String>,
    #[serde(rename = "updatedBy", default)]
    pub updated_by: Option<String>,
    #[serde(rename = "fiscalYear")]
    pub fiscal_year: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FiscalYearBillingAccessSettings {
    #[serde(rename = "nextYear", default)]
    pub next_year: Option<FiscalYearAccessEntry>,
    #[serde(rename = "previousYear", default)]
    pub previous_year: Option<FiscalYearAccessEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FiscalYearSettings {
    #[serde(default)]
    pub selector: FiscalYearSelectorSettings,
    #[serde(rename = "billingAccess", default)]
    pub billing_access: FiscalYearBillingAccessSettings,
    #[serde(rename = "activeFiscalYears", default)]
    pub active_fiscal_years: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TqmsIntegrationSettings {
    #[serde(
        rename = "SanghBranch",
        alias = "sanghBranch",
        alias = "sthaniyaBranch",
        default,
        deserialize_with = "deserialize_opt_string_id"
    )]
    pub sangh_branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct IntegrationsSettings {
    #[serde(default)]
    pub tqms: TqmsIntegrationSettings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmsNotificationSettings {
    #[serde(rename = "apiKey", default)]
    pub api_key: String,
    #[serde(rename = "baseUrl", default)]
    pub base_url: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_sms_provider")]
    pub provider: String,
    #[serde(rename = "senderId", default)]
    pub sender_id: String,
    #[serde(rename = "apiSecret", default)]
    pub api_secret: String,
    #[serde(
        rename = "routeId",
        alias = "routeid",
        alias = "route_id",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_string_id"
    )]
    pub route_id: Option<String>,
    #[serde(
        rename = "campaignId",
        alias = "campaign",
        alias = "campaign_id",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_string_id"
    )]
    pub campaign_id: Option<String>,
    #[serde(default)]
    pub templates: Vec<SettingSmsTemplate>,
    #[serde(rename = "webhookUrl", default)]
    pub webhook_url: String,
    #[serde(rename = "defaultCountryCode", default)]
    pub default_country_code: String,
}

impl Default for SmsNotificationSettings {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            base_url: String::new(),
            enabled: true,
            provider: default_sms_provider(),
            sender_id: String::new(),
            api_secret: String::new(),
            route_id: None,
            campaign_id: None,
            templates: Vec::new(),
            webhook_url: String::new(),
            default_country_code: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct NotificationsSettings {
    #[serde(default)]
    pub sms: SmsNotificationSettings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplacedTaggingSettings {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for DisplacedTaggingSettings {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct VehicleManagementSettings {
    #[serde(rename = "displacedTagging", default)]
    pub displaced_tagging: DisplacedTaggingSettings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(rename = "schemaVersion", default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub billing: BillingSettings,
    #[serde(default)]
    pub receipts: ReceiptsSettings,
    #[serde(default)]
    pub insurance: InsuranceSettings,
    #[serde(rename = "fiscalYear", default)]
    pub fiscal_year: FiscalYearSettings,
    #[serde(default)]
    pub integrations: IntegrationsSettings,
    #[serde(default)]
    pub notifications: NotificationsSettings,
    #[serde(rename = "vehicleManagement", default)]
    pub vehicle_management: VehicleManagementSettings,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            schema_version: default_schema_version(),
            billing: BillingSettings::default(),
            receipts: ReceiptsSettings::default(),
            insurance: InsuranceSettings::default(),
            fiscal_year: FiscalYearSettings::default(),
            integrations: IntegrationsSettings::default(),
            notifications: NotificationsSettings::default(),
            vehicle_management: VehicleManagementSettings::default(),
        }
    }
}

impl AppSettings {
    /// Parses both the new unified nested `SettingResource` schema and legacy flat keys.
    pub fn from_inner_value(inner: &Value) -> Self {
        let mut settings: AppSettings =
            serde_json::from_value(inner.clone()).unwrap_or_default();

        if inner.pointer("/billing/vat").is_none() {
            if let Some(v) = inner.get("vat") {
                if let Ok(parsed) = serde_json::from_value::<VatSettings>(v.clone()) {
                    settings.billing.vat = parsed;
                }
            }
        }
        if inner.pointer("/billing/arrears").is_none() {
            if let Some(a) = inner.get("arrears") {
                if let Ok(parsed) = serde_json::from_value::<ArrearsSettings>(a.clone()) {
                    settings.billing.arrears = parsed;
                }
            }
        }
        if inner.pointer("/billing/linkedFeeRules").is_none() {
            if let Some(l) = inner.get("linkedFeeRules") {
                if let Ok(parsed) = serde_json::from_value::<LinkedFeeRulesSettings>(l.clone()) {
                    settings.billing.linked_fee_rules = parsed;
                }
            }
        }
        if inner.pointer("/receipts/cancellation").is_none() {
            if let Some(c) = inner.get("receiptCancelWindow") {
                if let Ok(parsed) =
                    serde_json::from_value::<ReceiptCancellationSettings>(c.clone())
                {
                    settings.receipts.cancellation = parsed;
                }
            }
        }
        if inner.pointer("/insurance/eligibility").is_none() {
            if let Some(e) = inner.get("insuranceEligibility") {
                if let Ok(parsed) =
                    serde_json::from_value::<InsuranceEligibilitySettings>(e.clone())
                {
                    settings.insurance.eligibility = parsed;
                }
            }
        }
        if settings.integrations.tqms.sangh_branch.is_none() {
            settings.integrations.tqms.sangh_branch = resolve_tqms_sangh_branch_from_value(inner);
        }
        if inner.pointer("/notifications/sms").is_none() {
            if let Some(s) = inner.get("smsSettings").or_else(|| inner.get("sms_settings")) {
                if let Ok(parsed) = serde_json::from_value::<SmsNotificationSettings>(s.clone()) {
                    settings.notifications.sms = parsed;
                }
            }
        }
        if inner.pointer("/vehicleManagement/displacedTagging").is_none() {
            if let Some(d) = inner.get("displacedTagging") {
                if let Ok(parsed) = serde_json::from_value::<DisplacedTaggingSettings>(d.clone()) {
                    settings.vehicle_management.displaced_tagging = parsed;
                }
            }
        }

        settings
    }
}

fn is_structured_setting_object(inner: &Value) -> bool {
    let Some(map) = inner.as_object() else {
        return false;
    };
    [
        "schemaVersion",
        "billing",
        "receipts",
        "insurance",
        "fiscalYear",
        "integrations",
        "notifications",
        "vehicleManagement",
        "vat",
        "arrears",
        "linkedFeeRules",
        "receiptCancelWindow",
        "insuranceEligibility",
        "smsSettings",
        "displacedTagging",
    ]
    .iter()
    .any(|k| map.contains_key(*k))
}

/// Loads the highest-priority `AppSettings` from a slice/iterator of `SettingResource` items.
pub fn load_settings_from_resources<'a, I>(resources: I) -> Option<AppSettings>
where
    I: IntoIterator<Item = &'a DatabaseResource>,
{
    let mut best: Option<(bool, i32, AppSettings)> = None;
    for res in resources {
        let Some(inner) = res.inner_content.as_ref() else {
            continue;
        };
        if !inner.is_object() {
            continue;
        }
        let res_id = res.id.unwrap_or(0);
        let structured = is_structured_setting_object(inner);
        let should_replace = match &best {
            None => true,
            Some((best_structured, best_id, _)) => (structured, res_id) > (*best_structured, *best_id),
        };
        if should_replace {
            best = Some((structured, res_id, AppSettings::from_inner_value(inner)));
        }
    }
    best.map(|(_, _, s)| s)
}

pub fn load_settings_from_cache(cache: &Cache) -> Option<AppSettings> {
    let settings = cache.read_all(ResourceName::SettingResource).ok()?;
    load_settings_from_resources(settings)
}

pub async fn load_settings_from_server_data(s_data: &ServerData) -> Option<AppSettings> {
    {
        let cache = s_data.cache.read().await;
        if let Some(settings) = load_settings_from_cache(&cache) {
            return Some(settings);
        }
    }
    let db_settings = s_data
        .db
        .read_all_resource(ResourceName::SettingResource)
        .await
        .ok()?;
    load_settings_from_resources(db_settings.iter())
}

pub fn resolve_tqms_sangh_branch_from_value(inner: &Value) -> Option<String> {
    for ptr in [
        "/integrations/tqms/SanghBranch",
        "/integrations/tqms/sanghBranch",
        "/integrations/tqms/sthaniyaBranch",
    ] {
        if let Some(s) = inner.pointer(ptr).and_then(Value::as_str) {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    for key in ["SanghBranch", "sanghBranch", "sthaniyaBranch"] {
        if let Some(s) = inner.get(key).and_then(Value::as_str) {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

pub fn resolve_tqms_sangh_branch_from_cache(cache: &Cache) -> Option<String> {
    let settings = cache.read_all(ResourceName::SettingResource).ok()?;
    let mut best: Option<(i32, String)> = None;
    for res in settings {
        let Some(inner) = res.inner_content.as_ref() else {
            continue;
        };
        if let Some(branch) = resolve_tqms_sangh_branch_from_value(inner) {
            let res_id = res.id.unwrap_or(0);
            if best.as_ref().is_none_or(|(best_id, _)| res_id >= *best_id) {
                best = Some((res_id, branch));
            }
        }
    }
    best.map(|(_, branch)| branch)
}

pub fn is_displaced_tagging_enabled_in_resources<'a, I>(resources: I) -> bool
where
    I: IntoIterator<Item = &'a DatabaseResource>,
{
    let mut best: Option<(i32, bool)> = None;
    for res in resources {
        let Some(inner) = res.inner_content.as_ref() else {
            continue;
        };
        let explicit = inner
            .pointer("/vehicleManagement/displacedTagging/enabled")
            .and_then(Value::as_bool)
            .or_else(|| {
                inner
                    .get("displacedTagging")
                    .and_then(|d| d.get("enabled"))
                    .and_then(Value::as_bool)
            });
        if let Some(enabled) = explicit {
            let res_id = res.id.unwrap_or(0);
            if best.as_ref().is_none_or(|(best_id, _)| res_id >= *best_id) {
                best = Some((res_id, enabled));
            }
        }
    }
    best.map(|(_, enabled)| enabled).unwrap_or(true)
}

pub fn is_displaced_tagging_enabled_in_cache(cache: &Cache) -> bool {
    cache
        .read_all(ResourceName::SettingResource)
        .map(|s| is_displaced_tagging_enabled_in_resources(s))
        .unwrap_or(true)
}

// =========================================================================
// Domain 1: Billing VAT Calculation
// =========================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VatCalculationResult {
    pub enabled: bool,
    pub subtotal: f64,
    pub taxable_subtotal: f64,
    pub discount: f64,
    pub vat_rate_pct: f64,
    pub vat_amount: f64,
    pub total: f64,
    pub vat_billing_entity_ids: Vec<String>,
}

fn round2(val: f64) -> f64 {
    (val * 100.0).round() / 100.0
}

pub fn calculate_vat(vat: &VatSettings, items: &[Value], discount: f64) -> VatCalculationResult {
    let mut subtotal = 0.0_f64;
    let mut taxable_subtotal = 0.0_f64;
    let discount_clamped = discount.max(0.0);

    for item in items {
        let amount = item
            .get("amount")
            .and_then(|v| {
                v.as_f64()
                    .or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
            })
            .unwrap_or(0.0);
        subtotal += amount;

        let entity_id = extract_string_or_int(
            item.get("billingEntityId")
                .or_else(|| item.get("billing_entity_id")),
        );
        if vat.enabled {
            if let Some(ref eid) = entity_id {
                if vat
                    .billing_entity_ids
                    .iter()
                    .any(|allowed| allowed.trim() == eid.trim())
                {
                    taxable_subtotal += amount;
                }
            }
        }
    }

    let subtotal = round2(subtotal);
    let taxable_subtotal = round2(taxable_subtotal);
    let vat_rate_pct = if vat.enabled { vat.rate } else { 0.0 };
    let vat_amount = if vat.enabled && taxable_subtotal > 0.0 {
        round2(taxable_subtotal * vat.rate / 100.0)
    } else {
        0.0
    };
    let net_after_discount = (subtotal - discount_clamped).max(0.0);
    let total = round2(net_after_discount + vat_amount);

    VatCalculationResult {
        enabled: vat.enabled,
        subtotal,
        taxable_subtotal,
        discount: round2(discount_clamped),
        vat_rate_pct,
        vat_amount,
        total,
        vat_billing_entity_ids: if vat.enabled {
            vat.billing_entity_ids.clone()
        } else {
            Vec::new()
        },
    }
}

// =========================================================================
// Shared Vehicle / Date / Bill Matching Helpers
// =========================================================================

fn fiscal_years_canonically_match(a: &str, b: &str) -> bool {
    let ta = a.trim();
    let tb = b.trim();
    if ta.eq_ignore_ascii_case(tb) {
        return true;
    }
    match (parse_fiscal_year_start(ta), parse_fiscal_year_start(tb)) {
        (Some(ya), Some(yb)) => ya == yb,
        _ => false,
    }
}

fn bill_matches_vehicle(
    inner: &Value,
    target_id: Option<&str>,
    target_norm_num: Option<&str>,
) -> bool {
    let (v_id, v_num) = extract_bill_vehicle(inner);
    if let (Some(tid), Some(bid)) = (target_id.map(str::trim).filter(|s| !s.is_empty()), v_id) {
        if bid.trim() == tid {
            return true;
        }
    }
    if let (Some(tnorm), Some(bnum)) = (
        target_norm_num,
        v_num.and_then(normalize_vehicle_number),
    ) {
        if bnum == tnorm {
            return true;
        }
    }
    false
}

fn extract_bill_canonical_fiscal_year(inner: &Value) -> Option<String> {
    if let Some(period_fy) = extract_bill_period_start_fiscal_year(inner) {
        return Some(period_fy);
    }
    if let Some(explicit) =
        extract_string_or_int(inner.get("fiscalYear")).or_else(|| extract_string_or_int(inner.get("fiscalYearId")))
    {
        if parse_fiscal_year_start(&explicit).is_some() {
            return Some(explicit);
        }
    }
    extract_bill_inferred_fiscal_year(inner)
}

fn resolve_fee_head_id_for_billing_rule(
    rule_id: &str,
    billing_rules: &[&DatabaseResource],
) -> Option<String> {
    let target = rule_id.trim();
    for rule in billing_rules {
        if rule.id.map(|id| id.to_string()).as_deref() == Some(target) {
            if let Some(inner) = rule.inner_content.as_ref() {
                if let Some(fid) = extract_string_or_int(
                    inner.pointer("/feeHead/id").or_else(|| inner.get("feeHeadId")),
                ) {
                    return Some(fid);
                }
            }
        }
    }
    None
}

fn bill_item_matches_rule(
    item: &Value,
    target_rule_id: &str,
    target_fee_head_id: Option<&str>,
) -> bool {
    let item_rule_id =
        extract_string_or_int(item.get("ruleId").or_else(|| item.get("billingRuleId")));
    if item_rule_id.as_deref() == Some(target_rule_id.trim()) {
        return true;
    }
    if item_rule_id.is_none() {
        if let Some(fid) = target_fee_head_id {
            let item_fid =
                extract_string_or_int(item.get("feeHeadId").or_else(|| item.get("fee_head_id")));
            if item_fid.as_deref() == Some(fid) {
                return true;
            }
        }
    }
    false
}

fn get_shared_date_mapper() -> &'static DateMapper {
    static MAPPER: OnceLock<DateMapper> = OnceLock::new();
    MAPPER.get_or_init(|| {
        let mut mapper = DateMapper::empty();
        if mapper.load_from_json_file("date_range.json").is_err() {
            let _ = mapper.load_from_json_file("date_mappings.json");
        }
        mapper
    })
}

fn parse_naive_date_converting_bs_to_ad(raw: &str) -> Option<NaiveDate> {
    let trimmed = raw.trim();
    let date_part = trimmed.split(&['T', ' '][..]).next()?;
    let d = NaiveDate::parse_from_str(date_part, "%Y-%m-%d").ok()?;
    if d.year() >= 2050 {
        let key = format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day());
        if let Some(ad_str) = get_shared_date_mapper().bs_to_ad.get(&key) {
            if let Ok(ad_date) = NaiveDate::parse_from_str(ad_str.trim(), "%Y-%m-%d") {
                return Some(ad_date);
            }
        }
        // Fallback approximation if date_range.json is absent
        let approx_year = d.year() - 57;
        return NaiveDate::from_ymd_opt(approx_year, d.month(), d.day().min(28));
    }
    Some(d)
}

// =========================================================================
// Domain 2: Billing Arrears (monthlyPaid & paidExpired)
// =========================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonthlyPaidArrearsResult {
    pub enabled: bool,
    pub fiscal_year: Option<String>,
    pub billing_rule_id: Option<String>,
    pub paid_months: usize,
    pub remaining_months: usize,
    pub is_paid_in_fiscal_year: bool,
    pub is_fully_paid_12_months: bool,
    pub has_arrears: bool,
    pub matching_bill_ids: Vec<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaidExpiredArrearsResult {
    pub enabled: bool,
    pub billing_rule_id: Option<String>,
    pub has_paid_history: bool,
    pub latest_period_end: Option<String>,
    pub is_expired: bool,
    pub matching_bill_ids: Vec<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArrearsEvaluationResult {
    pub has_arrears: bool,
    pub monthly_paid: MonthlyPaidArrearsResult,
    pub paid_expired: PaidExpiredArrearsResult,
}

pub fn evaluate_vehicle_arrears(
    arrears: &ArrearsSettings,
    vehicle_id: Option<&str>,
    vehicle_number: Option<&str>,
    bills: &[&DatabaseResource],
    billing_rules: &[&DatabaseResource],
    current_date_ad: NaiveDate,
) -> ArrearsEvaluationResult {
    let target_norm = vehicle_number.and_then(normalize_vehicle_number);
    let vehicle_bills: Vec<&DatabaseResource> = bills
        .iter()
        .copied()
        .filter(|bill| {
            bill.inner_content
                .as_ref()
                .is_some_and(|inner| {
                    !is_bill_ignored(inner)
                        && bill_matches_vehicle(inner, vehicle_id, target_norm.as_deref())
                })
        })
        .collect();

    // 1. monthlyPaid
    let monthly_paid = if arrears.monthly_paid.enabled {
        let rule_id = arrears.monthly_paid.billing_rule_id.as_deref();
        let fee_head_id = rule_id.and_then(|rid| resolve_fee_head_id_for_billing_rule(rid, billing_rules));
        let mut paid_months = 0usize;
        let mut matching_bill_ids = Vec::new();

        for bill in &vehicle_bills {
            let Some(inner) = bill.inner_content.as_ref() else {
                continue;
            };

            if let Some(ref target_fy) = arrears.monthly_paid.fiscal_year {
                let Some(bill_fy) = extract_bill_canonical_fiscal_year(inner) else {
                    continue;
                };
                if !fiscal_years_canonically_match(&bill_fy, target_fy) {
                    continue;
                }
            }

            let rule_matches = match rule_id {
                Some(rid) => inner
                    .get("items")
                    .and_then(Value::as_array)
                    .is_some_and(|items| {
                        items
                            .iter()
                            .any(|item| bill_item_matches_rule(item, rid, fee_head_id.as_deref()))
                    }),
                None => true,
            };

            if !rule_matches {
                continue;
            }

            let months = inner
                .get("billingMonths")
                .and_then(Value::as_u64)
                .filter(|&m| m > 0)
                .unwrap_or(1) as usize;
            paid_months += months;
            if let Some(bid) = bill.id {
                matching_bill_ids.push(bid);
            }
        }

        let remaining_months = 12usize.saturating_sub(paid_months);
        let is_paid_in_fiscal_year = !matching_bill_ids.is_empty();
        let is_fully_paid_12_months = paid_months >= 12;

        MonthlyPaidArrearsResult {
            enabled: true,
            fiscal_year: arrears.monthly_paid.fiscal_year.clone(),
            billing_rule_id: arrears.monthly_paid.billing_rule_id.clone(),
            paid_months,
            remaining_months,
            is_paid_in_fiscal_year,
            is_fully_paid_12_months,
            has_arrears: !is_fully_paid_12_months,
            matching_bill_ids,
        }
    } else {
        MonthlyPaidArrearsResult {
            enabled: false,
            fiscal_year: arrears.monthly_paid.fiscal_year.clone(),
            billing_rule_id: arrears.monthly_paid.billing_rule_id.clone(),
            paid_months: 0,
            remaining_months: 0,
            is_paid_in_fiscal_year: false,
            is_fully_paid_12_months: true,
            has_arrears: false,
            matching_bill_ids: Vec::new(),
        }
    };

    // 2. paidExpired
    let paid_expired = if arrears.paid_expired.enabled {
        let rule_id = arrears.paid_expired.billing_rule_id.as_deref();
        let fee_head_id = rule_id.and_then(|rid| resolve_fee_head_id_for_billing_rule(rid, billing_rules));
        let mut latest_date: Option<NaiveDate> = None;
        let mut latest_raw: Option<String> = None;
        let mut matching_bill_ids = Vec::new();

        for bill in &vehicle_bills {
            let Some(inner) = bill.inner_content.as_ref() else {
                continue;
            };

            let rule_matches = match rule_id {
                Some(rid) => inner
                    .get("items")
                    .and_then(Value::as_array)
                    .is_some_and(|items| {
                        items
                            .iter()
                            .any(|item| bill_item_matches_rule(item, rid, fee_head_id.as_deref()))
                    }),
                None => true,
            };

            if !rule_matches {
                continue;
            }

            if let Some(bid) = bill.id {
                matching_bill_ids.push(bid);
            }

            for key in ["periodEnd", "expiryDate", "billingDate", "paidAt"] {
                if let Some(raw) = inner.get(key).and_then(Value::as_str) {
                    if let Some(parsed) = parse_naive_date_converting_bs_to_ad(raw) {
                        if latest_date.is_none_or(|prev| parsed > prev) {
                            latest_date = Some(parsed);
                            latest_raw = Some(parsed.format("%Y-%m-%d").to_string());
                        }
                        break;
                    }
                }
            }
        }

        let has_paid_history = !matching_bill_ids.is_empty();
        let is_expired = has_paid_history && latest_date.is_some_and(|d| d < current_date_ad);

        PaidExpiredArrearsResult {
            enabled: true,
            billing_rule_id: arrears.paid_expired.billing_rule_id.clone(),
            has_paid_history,
            latest_period_end: latest_raw,
            is_expired,
            matching_bill_ids,
        }
    } else {
        PaidExpiredArrearsResult {
            enabled: false,
            billing_rule_id: arrears.paid_expired.billing_rule_id.clone(),
            has_paid_history: false,
            latest_period_end: None,
            is_expired: false,
            matching_bill_ids: Vec::new(),
        }
    };

    ArrearsEvaluationResult {
        has_arrears: monthly_paid.has_arrears || paid_expired.is_expired,
        monthly_paid,
        paid_expired,
    }
}

// =========================================================================
// Domain 3: Linked Fee Rules
// =========================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissingLinkedFee {
    pub rule_id: String,
    pub source_fee_head_id: String,
    pub linked_fee_head_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linked_fee_head_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fiscal_year: Option<String>,
    pub once_per_fiscal_year: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkedFeeEvaluationResult {
    pub enabled: bool,
    pub valid: bool,
    pub missing_linked_fee_heads: Vec<MissingLinkedFee>,
    pub satisfied_linked_fee_heads: Vec<String>,
    pub skipped_already_paid_fee_heads: Vec<String>,
    pub duplicate_once_per_fy_fee_heads: Vec<String>,
}

pub fn extract_requested_fee_head_ids_from_items(
    items: &[Value],
    billing_rules: &[&DatabaseResource],
) -> Vec<String> {
    let mut ids = Vec::with_capacity(items.len());
    for item in items {
        if let Some(fid) = extract_string_or_int(
            item.get("feeHeadId")
                .or_else(|| item.get("fee_head_id")),
        ) {
            ids.push(fid);
        } else if let Some(rid) =
            extract_string_or_int(item.get("ruleId").or_else(|| item.get("billingRuleId")))
        {
            if let Some(fid) = resolve_fee_head_id_for_billing_rule(&rid, billing_rules) {
                ids.push(fid);
            }
        }
    }
    ids
}

fn bill_paid_fee_head_in_fiscal_year(
    bills: &[&DatabaseResource],
    billing_rules: &[&DatabaseResource],
    target_fee_head_id: &str,
    target_fy: Option<&str>,
) -> bool {
    for bill in bills {
        let Some(inner) = bill.inner_content.as_ref() else {
            continue;
        };
        if is_bill_ignored(inner) {
            continue;
        }
        if let Some(req_fy) = target_fy {
            let Some(bill_fy) = extract_bill_canonical_fiscal_year(inner) else {
                continue;
            };
            if !fiscal_years_canonically_match(&bill_fy, req_fy) {
                continue;
            }
        }
        let Some(items) = inner.get("items").and_then(Value::as_array) else {
            continue;
        };
        for item in items {
            if let Some(fid) = extract_string_or_int(
                item.get("feeHeadId")
                    .or_else(|| item.get("fee_head_id")),
            ) {
                if fid == target_fee_head_id.trim() {
                    return true;
                }
            } else if let Some(rid) =
                extract_string_or_int(item.get("ruleId").or_else(|| item.get("billingRuleId")))
            {
                if resolve_fee_head_id_for_billing_rule(&rid, billing_rules).as_deref()
                    == Some(target_fee_head_id.trim())
                {
                    return true;
                }
            }
        }
    }
    false
}

fn resolve_fee_head_name_by_id(
    fee_head_id: &str,
    billing_rules: &[&DatabaseResource],
) -> Option<String> {
    let target = fee_head_id.trim();
    for rule in billing_rules {
        let Some(inner) = rule.inner_content.as_ref() else {
            continue;
        };
        if inner
            .get("resourceType")
            .and_then(Value::as_str)
            .is_some_and(|t| t.eq_ignore_ascii_case("FEE_HEAD"))
            && rule.id.map(|id| id.to_string()).as_deref() == Some(target)
        {
            if let Some(name) = inner.get("name").and_then(Value::as_str) {
                return Some(name.to_string());
            }
        }
        if extract_string_or_int(inner.pointer("/feeHead/id")).as_deref() == Some(target) {
            if let Some(name) = inner.pointer("/feeHead/name").and_then(Value::as_str) {
                return Some(name.to_string());
            }
        }
    }
    None
}

pub fn evaluate_linked_fee_rules(
    cfg: &LinkedFeeRulesSettings,
    fiscal_year: Option<&str>,
    requested_fee_head_ids: &[String],
    vehicle_active_bills: &[&DatabaseResource],
    billing_rules: &[&DatabaseResource],
) -> LinkedFeeEvaluationResult {
    if !cfg.enabled {
        return LinkedFeeEvaluationResult {
            enabled: false,
            valid: true,
            missing_linked_fee_heads: Vec::new(),
            satisfied_linked_fee_heads: Vec::new(),
            skipped_already_paid_fee_heads: Vec::new(),
            duplicate_once_per_fy_fee_heads: Vec::new(),
        };
    }

    let req_set: HashSet<&str> = requested_fee_head_ids.iter().map(|s| s.trim()).collect();
    let mut missing_linked_fee_heads = Vec::new();
    let mut satisfied_linked_fee_heads = Vec::new();
    let mut skipped_already_paid_fee_heads = Vec::new();
    let mut duplicate_once_per_fy_fee_heads = Vec::new();

    for rule in &cfg.rules {
        if !rule.active {
            continue;
        }
        if let Some(rule_fy) = rule.fiscal_year.as_deref() {
            match fiscal_year {
                Some(req_fy) if fiscal_years_canonically_match(rule_fy, req_fy) => {}
                Some(_) => continue,
                None => {}
            }
        }

        let target_fy_for_history = fiscal_year.or(rule.fiscal_year.as_deref());
        let linked_id = rule.linked_fee_head_id.trim();
        let source_id = rule.source_fee_head_id.trim();

        let already_paid = bill_paid_fee_head_in_fiscal_year(
            vehicle_active_bills,
            billing_rules,
            linked_id,
            target_fy_for_history,
        );

        if rule.once_per_fiscal_year && already_paid && req_set.contains(linked_id) {
            if !duplicate_once_per_fy_fee_heads
                .iter()
                .any(|x| x == linked_id)
            {
                duplicate_once_per_fy_fee_heads.push(linked_id.to_string());
            }
            continue;
        }

        if req_set.contains(source_id) {
            if rule.once_per_fiscal_year && already_paid {
                if !skipped_already_paid_fee_heads
                    .iter()
                    .any(|x| x == linked_id)
                {
                    skipped_already_paid_fee_heads.push(linked_id.to_string());
                }
            } else if req_set.contains(linked_id) {
                if !satisfied_linked_fee_heads.iter().any(|x| x == linked_id) {
                    satisfied_linked_fee_heads.push(linked_id.to_string());
                }
            } else {
                missing_linked_fee_heads.push(MissingLinkedFee {
                    rule_id: rule.id.clone(),
                    source_fee_head_id: source_id.to_string(),
                    linked_fee_head_id: linked_id.to_string(),
                    linked_fee_head_name: resolve_fee_head_name_by_id(linked_id, billing_rules),
                    fiscal_year: rule.fiscal_year.clone(),
                    once_per_fiscal_year: rule.once_per_fiscal_year,
                });
            }
        }
    }

    let valid = missing_linked_fee_heads.is_empty() && duplicate_once_per_fy_fee_heads.is_empty();

    LinkedFeeEvaluationResult {
        enabled: true,
        valid,
        missing_linked_fee_heads,
        satisfied_linked_fee_heads,
        skipped_already_paid_fee_heads,
        duplicate_once_per_fy_fee_heads,
    }
}

// =========================================================================
// Domain 4: Receipts Cancellation Window
// =========================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReceiptCancellationCheck {
    pub enabled: bool,
    pub allowed: bool,
    pub window_hours: u64,
    pub elapsed_hours: Option<f64>,
    pub remaining_seconds: Option<i64>,
    pub receipt_timestamp: Option<String>,
    pub reason: String,
}

fn parse_utc_timestamp(raw: &str) -> Option<DateTime<Utc>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(trimmed) {
        return Some(dt.with_timezone(&Utc));
    }
    for fmt in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S"] {
        if let Ok(ndt) = NaiveDateTime::parse_from_str(trimmed, fmt) {
            return Some(ndt.and_utc());
        }
    }
    if let Ok(date) = NaiveDate::parse_from_str(trimmed, "%Y-%m-%d") {
        if let Some(ndt) = date.and_hms_opt(0, 0, 0) {
            return Some(ndt.and_utc());
        }
    }
    None
}

fn extract_bill_receipt_timestamp(bill: &DatabaseResource) -> Option<DateTime<Utc>> {
    if let Some(inner) = bill.inner_content.as_ref() {
        for key in ["paidAt", "createdAt", "paymentVerifiedAt"] {
            if let Some(dt) = inner.get(key).and_then(Value::as_str).and_then(parse_utc_timestamp) {
                return Some(dt);
            }
        }
    }
    if let Some(ext) = bill.external_content.as_ref() {
        if let Some(dt) = ext
            .get("createdTime")
            .and_then(Value::as_str)
            .and_then(parse_utc_timestamp)
        {
            return Some(dt);
        }
    }
    if let Some(inner) = bill.inner_content.as_ref() {
        if let Some(dt) = inner
            .get("billingDate")
            .and_then(Value::as_str)
            .and_then(parse_utc_timestamp)
        {
            return Some(dt);
        }
    }
    None
}

pub fn check_receipt_cancellation(
    cfg: &ReceiptCancellationSettings,
    bill: &DatabaseResource,
    now: DateTime<Utc>,
) -> ReceiptCancellationCheck {
    let receipt_ts = extract_bill_receipt_timestamp(bill);
    let receipt_ts_str = receipt_ts.map(|dt| dt.to_rfc3339());

    if !cfg.enabled {
        return ReceiptCancellationCheck {
            enabled: false,
            allowed: true,
            window_hours: cfg.hours,
            elapsed_hours: receipt_ts.map(|ts| {
                (now.signed_duration_since(ts).num_seconds() as f64) / 3600.0
            }),
            remaining_seconds: None,
            receipt_timestamp: receipt_ts_str,
            reason: "Receipt cancellation window restriction is disabled".to_string(),
        };
    }

    let Some(ts) = receipt_ts else {
        return ReceiptCancellationCheck {
            enabled: true,
            allowed: false,
            window_hours: cfg.hours,
            elapsed_hours: None,
            remaining_seconds: Some(0),
            receipt_timestamp: None,
            reason: "Receipt timestamp missing; cannot verify cancellation window".to_string(),
        };
    };

    let elapsed = now.signed_duration_since(ts);
    let max_duration = Duration::hours(cfg.hours as i64);
    let elapsed_secs = elapsed.num_seconds();
    let elapsed_hours = ((elapsed_secs as f64 / 3600.0) * 100.0).round() / 100.0;

    if elapsed <= max_duration {
        let remaining_secs = (max_duration.num_seconds() - elapsed_secs).max(0);
        ReceiptCancellationCheck {
            enabled: true,
            allowed: true,
            window_hours: cfg.hours,
            elapsed_hours: Some(elapsed_hours),
            remaining_seconds: Some(remaining_secs),
            receipt_timestamp: receipt_ts_str,
            reason: format!("Within {}-hour cancellation window", cfg.hours),
        }
    } else {
        ReceiptCancellationCheck {
            enabled: true,
            allowed: false,
            window_hours: cfg.hours,
            elapsed_hours: Some(elapsed_hours),
            remaining_seconds: Some(0),
            receipt_timestamp: receipt_ts_str,
            reason: format!(
                "Receipt cancellation window of {} hours has expired ({:.2}h elapsed)",
                cfg.hours,
                elapsed_secs as f64 / 3600.0
            ),
        }
    }
}

// =========================================================================
// Domain 5: Insurance Eligibility
// =========================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InsuranceEligibilityResult {
    pub enabled: bool,
    pub eligible: bool,
    pub min_paid_rules: usize,
    pub required_rule_ids: Vec<String>,
    pub paid_rule_ids: Vec<String>,
    pub matched_rule_ids: Vec<String>,
    pub missing_rule_ids: Vec<String>,
    pub reason: String,
}

pub fn evaluate_insurance_eligibility(
    cfg: &InsuranceEligibilitySettings,
    vehicle_id: Option<&str>,
    vehicle_number: Option<&str>,
    fiscal_year: Option<&str>,
    bills: &[&DatabaseResource],
) -> InsuranceEligibilityResult {
    if !cfg.enabled {
        return InsuranceEligibilityResult {
            enabled: false,
            eligible: false,
            min_paid_rules: cfg.min_paid_rules,
            required_rule_ids: cfg.rule_ids.clone(),
            paid_rule_ids: Vec::new(),
            matched_rule_ids: Vec::new(),
            missing_rule_ids: cfg.rule_ids.clone(),
            reason: "Insurance eligibility check is disabled".to_string(),
        };
    }

    let target_norm = vehicle_number.and_then(normalize_vehicle_number);
    let mut paid_set: HashSet<String> = HashSet::new();

    for bill in bills {
        let Some(inner) = bill.inner_content.as_ref() else {
            continue;
        };
        if is_bill_ignored(inner) {
            continue;
        }
        if !bill_matches_vehicle(inner, vehicle_id, target_norm.as_deref()) {
            continue;
        }
        if let Some(target_fy) = fiscal_year {
            let Some(bill_fy) = extract_bill_canonical_fiscal_year(inner) else {
                continue;
            };
            if !fiscal_years_canonically_match(&bill_fy, target_fy) {
                continue;
            }
        }

        if let Some(items) = inner.get("items").and_then(Value::as_array) {
            for item in items {
                if let Some(rid) =
                    extract_string_or_int(item.get("ruleId").or_else(|| item.get("billingRuleId")))
                {
                    paid_set.insert(rid);
                }
            }
        }
        if let Some(rid) = extract_string_or_int(inner.get("billingRuleId")) {
            paid_set.insert(rid);
        }
    }

    let mut paid_rule_ids: Vec<String> = paid_set.into_iter().collect();
    paid_rule_ids.sort();

    let (matched_rule_ids, missing_rule_ids) = if cfg.rule_ids.is_empty() {
        (paid_rule_ids.clone(), Vec::new())
    } else {
        let mut matched = Vec::new();
        let mut missing = Vec::new();
        for req_id in &cfg.rule_ids {
            let trimmed = req_id.trim();
            if paid_rule_ids.iter().any(|p| p == trimmed) {
                matched.push(trimmed.to_string());
            } else {
                missing.push(trimmed.to_string());
            }
        }
        (matched, missing)
    };

    let eligible = matched_rule_ids.len() >= cfg.min_paid_rules;
    let reason = if eligible {
        format!(
            "Vehicle meets insurance eligibility ({} >= {} required paid rules)",
            matched_rule_ids.len(),
            cfg.min_paid_rules
        )
    } else {
        format!(
            "Vehicle has {} matching paid rule(s); requires at least {}",
            matched_rule_ids.len(),
            cfg.min_paid_rules
        )
    };

    InsuranceEligibilityResult {
        enabled: true,
        eligible,
        min_paid_rules: cfg.min_paid_rules,
        required_rule_ids: cfg.rule_ids.clone(),
        paid_rule_ids,
        matched_rule_ids,
        missing_rule_ids,
        reason,
    }
}

// =========================================================================
// Domain 6: Fiscal Year Selector & Billing Access
// =========================================================================

pub fn current_bs_fiscal_year_start_year() -> i32 {
    let today_str = chrono::Local::now().date_naive().format("%Y-%m-%d").to_string();
    if let Some((bs_year, bs_month, _)) = ad_date_str_to_bs_ymd(&today_str) {
        if bs_month >= 4 {
            bs_year
        } else {
            bs_year - 1
        }
    } else {
        2083
    }
}

pub fn format_bs_fiscal_year_short(start_year: i32) -> String {
    format!("{}/{:02}", start_year, (start_year + 1) % 100)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FiscalYearSelectorResult {
    pub current_fiscal_year: String,
    pub selectable_years: Vec<String>,
    pub archived_years: Vec<String>,
    pub all_window_years: Vec<String>,
}

pub fn generate_fiscal_year_selector(
    selector: &FiscalYearSelectorSettings,
    current_bs_start_year: i32,
) -> FiscalYearSelectorResult {
    let start = current_bs_start_year - (selector.years_back as i32);
    let end = current_bs_start_year + (selector.years_forward as i32);

    let archived_start_years: HashSet<i32> = selector
        .archived_years
        .iter()
        .filter_map(|y| parse_fiscal_year_start(y))
        .collect();

    let mut all_window_years = Vec::new();
    let mut selectable_years = Vec::new();

    for y in start..=end {
        let label = format_bs_fiscal_year_short(y);
        let is_archived = archived_start_years.contains(&y)
            || selector
                .archived_years
                .iter()
                .any(|a| fiscal_years_canonically_match(a, &label));
        all_window_years.push(label.clone());
        if !is_archived {
            selectable_years.push(label);
        }
    }

    FiscalYearSelectorResult {
        current_fiscal_year: format_bs_fiscal_year_short(current_bs_start_year),
        selectable_years,
        archived_years: selector.archived_years.clone(),
        all_window_years,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FiscalYearBillingAccessResult {
    pub fiscal_year: String,
    pub allowed: bool,
    pub status: String,
    pub code: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<String>,
}

pub fn check_fiscal_year_billing_access(
    fy_settings: &FiscalYearSettings,
    target_fiscal_year: &str,
    current_bs_start_year: i32,
) -> FiscalYearBillingAccessResult {
    let target = target_fiscal_year.trim();
    let target_start = parse_fiscal_year_start(target);

    if fy_settings
        .selector
        .archived_years
        .iter()
        .any(|a| fiscal_years_canonically_match(a, target))
    {
        return FiscalYearBillingAccessResult {
            fiscal_year: target.to_string(),
            allowed: false,
            status: "ARCHIVED".to_string(),
            code: "FISCAL_YEAR_ARCHIVED".to_string(),
            reason: format!("Fiscal year '{}' is archived and cannot be billed", target),
            updated_at: None,
            updated_by: None,
        };
    }

    if let Some(ref prev) = fy_settings.billing_access.previous_year {
        if fiscal_years_canonically_match(&prev.fiscal_year, target) {
            let is_open = prev.status.trim().eq_ignore_ascii_case("OPEN");
            return FiscalYearBillingAccessResult {
                fiscal_year: target.to_string(),
                allowed: is_open,
                status: prev.status.trim().to_uppercase(),
                code: if is_open {
                    "ALLOWED".to_string()
                } else {
                    "FISCAL_YEAR_BILLING_CLOSED".to_string()
                },
                reason: if is_open {
                    format!("Previous fiscal year '{}' billing access is OPEN", target)
                } else {
                    format!(
                        "Billing for previous fiscal year '{}' is {}",
                        target, prev.status
                    )
                },
                updated_at: prev.updated_at.clone(),
                updated_by: prev.updated_by.clone(),
            };
        }
    }

    if let Some(ref next) = fy_settings.billing_access.next_year {
        if fiscal_years_canonically_match(&next.fiscal_year, target) {
            let is_open = next.status.trim().eq_ignore_ascii_case("OPEN");
            return FiscalYearBillingAccessResult {
                fiscal_year: target.to_string(),
                allowed: is_open,
                status: next.status.trim().to_uppercase(),
                code: if is_open {
                    "ALLOWED".to_string()
                } else {
                    "FISCAL_YEAR_BILLING_CLOSED".to_string()
                },
                reason: if is_open {
                    format!("Next fiscal year '{}' billing access is OPEN", target)
                } else {
                    format!(
                        "Billing for next fiscal year '{}' is {}",
                        target, next.status
                    )
                },
                updated_at: next.updated_at.clone(),
                updated_by: next.updated_by.clone(),
            };
        }
    }

    if let Some(start) = target_start {
        if start == current_bs_start_year {
            return FiscalYearBillingAccessResult {
                fiscal_year: target.to_string(),
                allowed: true,
                status: "OPEN".to_string(),
                code: "ALLOWED".to_string(),
                reason: format!("Current fiscal year '{}' is open for billing", target),
                updated_at: None,
                updated_by: None,
            };
        } else if start > current_bs_start_year {
            return FiscalYearBillingAccessResult {
                fiscal_year: target.to_string(),
                allowed: false,
                status: "CLOSED".to_string(),
                code: "FISCAL_YEAR_BILLING_CLOSED".to_string(),
                reason: format!(
                    "Billing for future fiscal year '{}' has not been opened",
                    target
                ),
                updated_at: None,
                updated_by: None,
            };
        } else {
            return FiscalYearBillingAccessResult {
                fiscal_year: target.to_string(),
                allowed: false,
                status: "CLOSED".to_string(),
                code: "FISCAL_YEAR_BILLING_CLOSED".to_string(),
                reason: format!(
                    "Billing for past fiscal year '{}' is not open",
                    target
                ),
                updated_at: None,
                updated_by: None,
            };
        }
    }

    FiscalYearBillingAccessResult {
        fiscal_year: target.to_string(),
        allowed: true,
        status: "OPEN".to_string(),
        code: "ALLOWED".to_string(),
        reason: "Unrecognized fiscal year identifier allowed by default".to_string(),
        updated_at: None,
        updated_by: None,
    }
}

// =========================================================================
// HTTP Handlers & Routes (/api/v1/settings)
// =========================================================================

#[derive(Debug, Deserialize)]
pub struct VatRequest {
    pub items: Vec<Value>,
    #[serde(default)]
    pub discount: f64,
}

#[derive(Debug, Deserialize)]
pub struct ArrearsQuery {
    #[serde(rename = "asOfDate", default)]
    pub as_of_date: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LinkedFeeEvalRequest {
    #[serde(rename = "fiscalYear", default)]
    pub fiscal_year: Option<String>,
    #[serde(rename = "vehicleId", default)]
    pub vehicle_id: Option<String>,
    #[serde(rename = "vehicleNumber", default)]
    pub vehicle_number: Option<String>,
    #[serde(rename = "requestedFeeHeadIds", default)]
    pub requested_fee_head_ids: Vec<String>,
    #[serde(default)]
    pub items: Vec<Value>,
}

#[derive(Debug, Deserialize)]
pub struct InsuranceEligibilityQuery {
    #[serde(rename = "fiscalYear", default)]
    pub fiscal_year: Option<String>,
}

pub async fn get_resolved_settings_handler(
    req: HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let mut settings = load_settings_from_server_data(&s_data)
        .await
        .unwrap_or_default();

    if !is_admin(req) {
        if !settings.notifications.sms.api_key.trim().is_empty() {
            settings.notifications.sms.api_key = "[REDACTED]".to_string();
        }
        if !settings.notifications.sms.api_secret.trim().is_empty() {
            settings.notifications.sms.api_secret = "[REDACTED]".to_string();
        }
    }

    HttpResponse::Ok().json(settings)
}

pub async fn calculate_vat_handler(
    body: web::Json<VatRequest>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let settings = load_settings_from_server_data(&s_data)
        .await
        .unwrap_or_default();
    let payload = body.into_inner();
    let result = calculate_vat(&settings.billing.vat, &payload.items, payload.discount);
    HttpResponse::Ok().json(result)
}

pub async fn evaluate_vehicle_arrears_handler(
    path: web::Path<String>,
    query: web::Query<ArrearsQuery>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let vehicle = path.into_inner();
    let settings = load_settings_from_server_data(&s_data)
        .await
        .unwrap_or_default();

    let as_of_ad = query
        .as_of_date
        .as_deref()
        .and_then(parse_naive_date_converting_bs_to_ad)
        .unwrap_or_else(|| chrono::Local::now().date_naive());

    let cache = s_data.cache.read().await;
    let bills = cache.read_all(ResourceName::BillResource).unwrap_or_default();
    let billing_rules = cache
        .read_all(ResourceName::BillingRuleResource)
        .unwrap_or_default();

    let result = evaluate_vehicle_arrears(
        &settings.billing.arrears,
        Some(vehicle.trim()),
        Some(vehicle.trim()),
        &bills,
        &billing_rules,
        as_of_ad,
    );
    HttpResponse::Ok().json(result)
}

pub async fn evaluate_linked_fees_handler(
    body: web::Json<LinkedFeeEvalRequest>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let settings = load_settings_from_server_data(&s_data)
        .await
        .unwrap_or_default();
    let payload = body.into_inner();

    let cache = s_data.cache.read().await;
    let all_bills = cache.read_all(ResourceName::BillResource).unwrap_or_default();
    let billing_rules = cache
        .read_all(ResourceName::BillingRuleResource)
        .unwrap_or_default();

    let mut requested = payload.requested_fee_head_ids;
    if requested.is_empty() && !payload.items.is_empty() {
        requested = extract_requested_fee_head_ids_from_items(&payload.items, &billing_rules);
    }

    let target_norm = payload
        .vehicle_number
        .as_deref()
        .and_then(normalize_vehicle_number);
    let vehicle_bills: Vec<&DatabaseResource> =
        if payload.vehicle_id.is_some() || target_norm.is_some() {
            all_bills
                .into_iter()
                .filter(|b| {
                    b.inner_content.as_ref().is_some_and(|inner| {
                        !is_bill_ignored(inner)
                            && bill_matches_vehicle(
                                inner,
                                payload.vehicle_id.as_deref(),
                                target_norm.as_deref(),
                            )
                    })
                })
                .collect()
        } else {
            Vec::new()
        };

    let result = evaluate_linked_fee_rules(
        &settings.billing.linked_fee_rules,
        payload.fiscal_year.as_deref(),
        &requested,
        &vehicle_bills,
        &billing_rules,
    );
    HttpResponse::Ok().json(result)
}

pub async fn check_receipt_cancellation_handler(
    path: web::Path<i32>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let bill_id = path.into_inner();
    let settings = load_settings_from_server_data(&s_data)
        .await
        .unwrap_or_default();

    let bill_opt = {
        let cache = s_data.cache.read().await;
        cache.read(ResourceName::BillResource, bill_id)
    };
    let bill = match bill_opt {
        Some(b) => b,
        None => match s_data.db.read_resource(ResourceName::BillResource, bill_id).await {
            Ok(b) if b.id.is_some() => b,
            _ => {
                return HttpResponse::NotFound().json(serde_json::json!({
                    "success": false,
                    "message": format!("Bill {bill_id} not found")
                }));
            }
        },
    };

    let result = check_receipt_cancellation(&settings.receipts.cancellation, &bill, Utc::now());
    HttpResponse::Ok().json(result)
}

pub async fn evaluate_insurance_eligibility_handler(
    path: web::Path<String>,
    query: web::Query<InsuranceEligibilityQuery>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let vehicle = path.into_inner();
    let settings = load_settings_from_server_data(&s_data)
        .await
        .unwrap_or_default();

    let cache = s_data.cache.read().await;
    let bills = cache.read_all(ResourceName::BillResource).unwrap_or_default();

    let result = evaluate_insurance_eligibility(
        &settings.insurance.eligibility,
        Some(vehicle.trim()),
        Some(vehicle.trim()),
        query.fiscal_year.as_deref(),
        &bills,
    );
    HttpResponse::Ok().json(result)
}

pub async fn get_fiscal_year_selector_handler(
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let settings = load_settings_from_server_data(&s_data)
        .await
        .unwrap_or_default();
    let current_start = current_bs_fiscal_year_start_year();
    let result = generate_fiscal_year_selector(&settings.fiscal_year.selector, current_start);
    HttpResponse::Ok().json(result)
}

pub async fn check_fiscal_year_access_handler(
    path: web::Path<String>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let fy = path.into_inner();
    let settings = load_settings_from_server_data(&s_data)
        .await
        .unwrap_or_default();
    let current_start = current_bs_fiscal_year_start_year();
    let result = check_fiscal_year_billing_access(&settings.fiscal_year, &fy, current_start);
    HttpResponse::Ok().json(result)
}

pub async fn get_displaced_tagging_status_handler(
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let settings = load_settings_from_server_data(&s_data)
        .await
        .unwrap_or_default();
    HttpResponse::Ok().json(serde_json::json!({
        "enabled": settings.vehicle_management.displaced_tagging.enabled
    }))
}

pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.route("/resolved", web::get().to(get_resolved_settings_handler))
        .route("/billing/calculate-vat", web::post().to(calculate_vat_handler))
        .route(
            "/billing/arrears/{vehicle}",
            web::get().to(evaluate_vehicle_arrears_handler),
        )
        .route(
            "/billing/linked-fees/evaluate",
            web::post().to(evaluate_linked_fees_handler),
        )
        .route(
            "/receipts/cancellation-check/{bill_id}",
            web::get().to(check_receipt_cancellation_handler),
        )
        .route(
            "/insurance/eligibility/{vehicle}",
            web::get().to(evaluate_insurance_eligibility_handler),
        )
        .route(
            "/fiscal-year/selector",
            web::get().to(get_fiscal_year_selector_handler),
        )
        .route(
            "/fiscal-year/access/{fiscal_year:.*}",
            web::get().to(check_fiscal_year_access_handler),
        )
        .route(
            "/vehicle-management/displaced-tagging",
            web::get().to(get_displaced_tagging_status_handler),
        );
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{http::StatusCode, test as web_test, App, HttpMessage};

    fn full_user_setting_resource() -> DatabaseResource {
        DatabaseResource {
            id: Some(1),
            resource_name: Some(ResourceName::SettingResource),
            inner_content: Some(serde_json::json!({
                "billing": {
                    "vat": {
                        "rate": 13,
                        "enabled": true,
                        "billingEntityIds": ["1"]
                    },
                    "arrears": {
                        "monthlyPaid": {
                            "enabled": true,
                            "fiscalYear": "2083/84",
                            "billingRuleId": "45"
                        },
                        "paidExpired": {
                            "enabled": true,
                            "billingRuleId": "5"
                        }
                    },
                    "linkedFeeRules": {
                        "rules": [
                            {
                                "id": "f72c97c7-ddd4-4d35-bbdd-86e4746e57db",
                                "active": true,
                                "fiscalYear": "2082/83",
                                "linkedFeeHeadId": "21",
                                "sourceFeeHeadId": "3",
                                "oncePerFiscalYear": true
                            },
                            {
                                "id": "f604ebf4-8c7a-4e48-b9c5-40be292dcb7d",
                                "active": true,
                                "fiscalYear": "2082/83",
                                "linkedFeeHeadId": "36",
                                "sourceFeeHeadId": "25",
                                "oncePerFiscalYear": true
                            }
                        ],
                        "enabled": true
                    }
                },
                "receipts": {
                    "cancellation": {
                        "hours": 24,
                        "enabled": false
                    }
                },
                "insurance": {
                    "eligibility": {
                        "enabled": false,
                        "ruleIds": [],
                        "minPaidRules": 1
                    }
                },
                "fiscalYear": {
                    "selector": {
                        "yearsBack": 4,
                        "yearsForward": 1,
                        "archivedYears": []
                    },
                    "billingAccess": {
                        "nextYear": null,
                        "previousYear": {
                            "status": "CLOSED",
                            "updatedAt": "2026-11-22T12:08:22.422Z",
                            "updatedBy": "System Administrator",
                            "fiscalYear": "2082/83"
                        }
                    }
                },
                "integrations": {
                    "tqms": {
                        "SanghBranch": "Loadboard sthaniya"
                    }
                },
                "notifications": {
                    "sms": {
                        "apiKey": "36A1291E165814",
                        "baseUrl": "https://samayasms.com.np/smsapi/index.php",
                        "enabled": false,
                        "provider": "samayasms",
                        "senderId": "",
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
                },
                "schemaVersion": 1,
                "vehicleManagement": {
                    "displacedTagging": {
                        "enabled": true
                    }
                }
            })),
            external_content: None,
        }
    }

    #[test]
    fn test_parse_exact_nested_setting_resource_and_legacy_fallback() {
        let res = full_user_setting_resource();
        let parsed = AppSettings::from_inner_value(res.inner_content.as_ref().unwrap());

        assert_eq!(parsed.schema_version, 1);
        assert_eq!(parsed.billing.vat.rate, 13.0);
        assert!(parsed.billing.vat.enabled);
        assert_eq!(parsed.billing.vat.billing_entity_ids, vec!["1"]);

        assert!(parsed.billing.arrears.monthly_paid.enabled);
        assert_eq!(
            parsed.billing.arrears.monthly_paid.fiscal_year.as_deref(),
            Some("2083/84")
        );
        assert_eq!(
            parsed.billing.arrears.monthly_paid.billing_rule_id.as_deref(),
            Some("45")
        );
        assert!(parsed.billing.arrears.paid_expired.enabled);
        assert_eq!(
            parsed.billing.arrears.paid_expired.billing_rule_id.as_deref(),
            Some("5")
        );

        assert!(parsed.billing.linked_fee_rules.enabled);
        assert_eq!(parsed.billing.linked_fee_rules.rules.len(), 2);
        assert_eq!(parsed.billing.linked_fee_rules.rules[0].source_fee_head_id, "3");
        assert_eq!(parsed.billing.linked_fee_rules.rules[0].linked_fee_head_id, "21");

        assert_eq!(parsed.receipts.cancellation.hours, 24);
        assert!(!parsed.receipts.cancellation.enabled);

        assert!(!parsed.insurance.eligibility.enabled);
        assert_eq!(parsed.insurance.eligibility.min_paid_rules, 1);
        assert!(parsed.insurance.eligibility.rule_ids.is_empty());

        assert_eq!(parsed.fiscal_year.selector.years_back, 4);
        assert_eq!(parsed.fiscal_year.selector.years_forward, 1);
        assert!(parsed.fiscal_year.selector.archived_years.is_empty());
        assert!(parsed.fiscal_year.billing_access.next_year.is_none());
        let prev = parsed.fiscal_year.billing_access.previous_year.as_ref().unwrap();
        assert_eq!(prev.status, "CLOSED");
        assert_eq!(prev.fiscal_year, "2082/83");
        assert_eq!(prev.updated_by.as_deref(), Some("System Administrator"));

        assert_eq!(
            parsed.integrations.tqms.sangh_branch.as_deref(),
            Some("Loadboard sthaniya")
        );

        assert_eq!(parsed.notifications.sms.api_key, "36A1291E165814");
        assert!(!parsed.notifications.sms.enabled);
        assert_eq!(parsed.notifications.sms.provider, "samayasms");
        assert_eq!(parsed.notifications.sms.default_country_code, "+977");
        assert_eq!(parsed.notifications.sms.templates.len(), 3);
        assert_eq!(parsed.notifications.sms.templates[0].id, "PAID");
        assert!(parsed.vehicle_management.displaced_tagging.enabled);

        // Legacy flat schema parses identically
        let legacy = AppSettings::from_inner_value(&serde_json::json!({
            "vat": { "rate": 13, "enabled": true, "billingEntityIds": [1, 2] },
            "arrears": {
                "monthlyPaid": { "enabled": true, "fiscalYearId": "2082/83", "billingRuleId": 5 }
            },
            "receiptCancelWindow": { "hours": 48, "enabled": true },
            "insuranceEligibility": { "enabled": true, "ruleIds": [12], "minPaidRules": 1 },
            "SanghBranch": "Main Branch",
            "displacedTagging": { "enabled": false }
        }));
        assert_eq!(legacy.billing.vat.billing_entity_ids, vec!["1", "2"]);
        assert_eq!(legacy.receipts.cancellation.hours, 48);
        assert!(legacy.receipts.cancellation.enabled);
        assert!(legacy.insurance.eligibility.enabled);
        assert_eq!(
            legacy.integrations.tqms.sangh_branch.as_deref(),
            Some("Main Branch")
        );
        assert!(!legacy.vehicle_management.displaced_tagging.enabled);
    }

    #[test]
    fn test_vat_calculation_scenarios() {
        let vat = VatSettings {
            rate: 13.0,
            enabled: true,
            billing_entity_ids: vec!["1".to_string()],
        };

        // 1. All items match billingEntityId "1" -> 13% VAT
        let r1 = calculate_vat(
            &vat,
            &[serde_json::json!({ "amount": 500, "billingEntityId": "1" })],
            0.0,
        );
        assert_eq!(r1.subtotal, 500.0);
        assert_eq!(r1.taxable_subtotal, 500.0);
        assert_eq!(r1.vat_amount, 65.0);
        assert_eq!(r1.total, 565.0);

        // 2. Mixed taxable and exempt entities + discount
        let r2 = calculate_vat(
            &vat,
            &[
                serde_json::json!({ "amount": 1000, "billingEntityId": 1 }),
                serde_json::json!({ "amount": 400, "billingEntityId": "2" }),
            ],
            100.0,
        );
        assert_eq!(r2.subtotal, 1400.0);
        assert_eq!(r2.taxable_subtotal, 1000.0);
        assert_eq!(r2.vat_amount, 130.0);
        assert_eq!(r2.total, 1430.0);

        // 3. VAT disabled -> 0 VAT
        let mut disabled = vat.clone();
        disabled.enabled = false;
        let r3 = calculate_vat(
            &disabled,
            &[serde_json::json!({ "amount": 500, "billingEntityId": "1" })],
            0.0,
        );
        assert_eq!(r3.vat_rate_pct, 0.0);
        assert_eq!(r3.vat_amount, 0.0);
        assert_eq!(r3.total, 500.0);
        assert!(r3.vat_billing_entity_ids.is_empty());

        // 4. Empty billingEntityIds -> 0 taxable amount
        let mut no_entities = vat;
        no_entities.billing_entity_ids.clear();
        let r4 = calculate_vat(
            &no_entities,
            &[serde_json::json!({ "amount": 500, "billingEntityId": "1" })],
            0.0,
        );
        assert_eq!(r4.taxable_subtotal, 0.0);
        assert_eq!(r4.vat_amount, 0.0);
    }

    #[test]
    fn test_arrears_evaluation_monthly_paid_and_paid_expired_scenarios() {
        let settings = AppSettings::from_inner_value(
            full_user_setting_resource().inner_content.as_ref().unwrap(),
        );
        let rules = vec![DatabaseResource {
            id: Some(45),
            resource_name: Some(ResourceName::BillingRuleResource),
            inner_content: Some(serde_json::json!({
                "feeHead": { "id": 3, "name": "Monthly Fee" }
            })),
            external_content: None,
        }];
        let rule_refs: Vec<&DatabaseResource> = rules.iter().collect();

        let bills = vec![
            // Vehicle V1: paid 12 months on rule 45 for 2083/84 + unexpired rule 5
            DatabaseResource {
                id: Some(10),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(serde_json::json!({
                    "vehicle": { "id": "V1", "vehicleNumber": "BA-01-PA-1111" },
                    "fiscalYear": "2083/84",
                    "billingMonths": 12,
                    "status": "PAID",
                    "items": [{ "ruleId": "45", "amount": 7200 }]
                })),
                external_content: None,
            },
            DatabaseResource {
                id: Some(11),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(serde_json::json!({
                    "vehicle": { "id": "V1", "vehicleNumber": "BA-01-PA-1111" },
                    "periodEnd": "2026-12-30",
                    "status": "PAID",
                    "items": [{ "ruleId": "5", "amount": 600 }]
                })),
                external_content: None,
            },
            // Vehicle V2: partial 4 months on 2083/2084 + expired rule 5
            DatabaseResource {
                id: Some(20),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(serde_json::json!({
                    "vehicle": { "id": "V2", "vehicleNumber": "BA-01-PA-2222" },
                    "fiscalYear": "2083/2084",
                    "billingMonths": 4,
                    "status": "PAID",
                    "items": [{ "feeHeadId": "3", "amount": 2400 }]
                })),
                external_content: None,
            },
            DatabaseResource {
                id: Some(21),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(serde_json::json!({
                    "vehicle": { "id": "V2", "vehicleNumber": "BA-01-PA-2222" },
                    "periodEnd": "2026-11-01",
                    "status": "PAID",
                    "items": [{ "ruleId": "5", "amount": 600 }]
                })),
                external_content: None,
            },
            // Cancelled bill must be ignored
            DatabaseResource {
                id: Some(22),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(serde_json::json!({
                    "vehicle": { "id": "V2", "vehicleNumber": "BA-01-PA-2222" },
                    "fiscalYear": "2083/84",
                    "billingMonths": 12,
                    "status": "DELETED",
                    "items": [{ "ruleId": "45", "amount": 7200 }]
                })),
                external_content: None,
            },
        ];
        let bill_refs: Vec<&DatabaseResource> = bills.iter().collect();
        let today = NaiveDate::from_ymd_opt(2026, 11, 23).unwrap();

        let r_v1 = evaluate_vehicle_arrears(
            &settings.billing.arrears,
            Some("V1"),
            None,
            &bill_refs,
            &rule_refs,
            today,
        );
        assert!(!r_v1.has_arrears);
        assert_eq!(r_v1.monthly_paid.paid_months, 12);
        assert!(!r_v1.paid_expired.is_expired);

        let r_v2 = evaluate_vehicle_arrears(
            &settings.billing.arrears,
            None,
            Some("ba 01 pa 2222"),
            &bill_refs,
            &rule_refs,
            today,
        );
        assert!(r_v2.has_arrears);
        assert_eq!(r_v2.monthly_paid.paid_months, 4);
        assert_eq!(r_v2.monthly_paid.remaining_months, 8);
        assert!(r_v2.paid_expired.has_paid_history);
        assert!(r_v2.paid_expired.is_expired);

        // Disabled arrears -> has_arrears false
        let r_off = evaluate_vehicle_arrears(
            &ArrearsSettings::default(),
            Some("V2"),
            None,
            &bill_refs,
            &rule_refs,
            today,
        );
        assert!(!r_off.has_arrears);
    }

    #[test]
    fn test_linked_fee_rules_all_scenarios() {
        let settings = AppSettings::from_inner_value(
            full_user_setting_resource().inner_content.as_ref().unwrap(),
        );
        let rules = vec![DatabaseResource {
            id: Some(21),
            resource_name: Some(ResourceName::BillingRuleResource),
            inner_content: Some(serde_json::json!({
                "name": "सदस्यता शुल्क लिंक",
                "resourceType": "FEE_HEAD"
            })),
            external_content: None,
        }];
        let rule_refs: Vec<&DatabaseResource> = rules.iter().collect();

        // 1. Source fee "3" requested without linked fee "21" in "2082/83" -> missing
        let res_missing = evaluate_linked_fee_rules(
            &settings.billing.linked_fee_rules,
            Some("2082/83"),
            &["3".to_string()],
            &[],
            &rule_refs,
        );
        assert!(!res_missing.valid);
        assert_eq!(res_missing.missing_linked_fee_heads.len(), 1);
        assert_eq!(res_missing.missing_linked_fee_heads[0].linked_fee_head_id, "21");

        // 2. Source "3" + linked "21" together -> satisfied
        let res_ok = evaluate_linked_fee_rules(
            &settings.billing.linked_fee_rules,
            Some("2082/2083"),
            &["3".to_string(), "21".to_string()],
            &[],
            &rule_refs,
        );
        assert!(res_ok.valid);
        assert_eq!(res_ok.satisfied_linked_fee_heads, vec!["21"]);

        // 3. Linked "21" already paid in same FY & oncePerFiscalYear=true:
        //    3a. Requesting source "3" alone -> skipped cleanly (valid = true)
        let paid_bill = DatabaseResource {
            id: Some(1),
            resource_name: Some(ResourceName::BillResource),
            inner_content: Some(serde_json::json!({
                "fiscalYear": "2082/83",
                "status": "PAID",
                "items": [{ "feeHeadId": "21", "amount": 500 }]
            })),
            external_content: None,
        };
        let res_skip = evaluate_linked_fee_rules(
            &settings.billing.linked_fee_rules,
            Some("2082/83"),
            &["3".to_string()],
            &[&paid_bill],
            &rule_refs,
        );
        assert!(res_skip.valid);
        assert_eq!(res_skip.skipped_already_paid_fee_heads, vec!["21"]);

        //    3b. Requesting linked "21" again in same FY -> duplicate blocked!
        let res_dup = evaluate_linked_fee_rules(
            &settings.billing.linked_fee_rules,
            Some("2082/83"),
            &["3".to_string(), "21".to_string()],
            &[&paid_bill],
            &rule_refs,
        );
        assert!(!res_dup.valid);
        assert_eq!(res_dup.duplicate_once_per_fy_fee_heads, vec!["21"]);

        // 4. Different fiscal year ("2083/84") -> rule for "2082/83" does not apply
        let res_other_fy = evaluate_linked_fee_rules(
            &settings.billing.linked_fee_rules,
            Some("2083/84"),
            &["3".to_string()],
            &[],
            &rule_refs,
        );
        assert!(res_other_fy.valid);

        // 5. Rule for "2082/83" inactive or feature disabled -> valid = true
        let mut disabled_cfg = settings.billing.linked_fee_rules.clone();
        disabled_cfg.enabled = false;
        assert!(evaluate_linked_fee_rules(
            &disabled_cfg,
            Some("2082/83"),
            &["3".to_string()],
            &[],
            &rule_refs,
        )
        .valid);
    }

    #[test]
    fn test_receipt_cancellation_window_scenarios() {
        let now = DateTime::parse_from_rfc3339("2026-11-23T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let old_bill = DatabaseResource {
            id: Some(1),
            resource_name: Some(ResourceName::BillResource),
            inner_content: Some(serde_json::json!({ "paidAt": "2026-11-21T10:00:00Z" })),
            external_content: None,
        };
        let exact_24h_bill = DatabaseResource {
            id: Some(2),
            resource_name: Some(ResourceName::BillResource),
            inner_content: Some(serde_json::json!({ "paidAt": "2026-11-22T12:00:00Z" })),
            external_content: None,
        };
        let ext_ts_bill = DatabaseResource {
            id: Some(3),
            resource_name: Some(ResourceName::BillResource),
            inner_content: Some(serde_json::json!({})),
            external_content: Some(serde_json::json!({ "createdTime": "2026-11-23T06:00:00Z" })),
        };

        // Disabled -> old bill allowed
        let disabled = ReceiptCancellationSettings { hours: 24, enabled: false };
        assert!(check_receipt_cancellation(&disabled, &old_bill, now).allowed);

        // Enabled -> old bill blocked, exact 24h boundary & recent external_content bill allowed
        let enabled = ReceiptCancellationSettings { hours: 24, enabled: true };
        let old_res = check_receipt_cancellation(&enabled, &old_bill, now);
        assert!(!old_res.allowed);
        assert!(old_res.reason.contains("expired"));
        assert!(check_receipt_cancellation(&enabled, &exact_24h_bill, now).allowed);
        let ext_res = check_receipt_cancellation(&enabled, &ext_ts_bill, now);
        assert!(ext_res.allowed);
        assert!(ext_res.remaining_seconds.unwrap() > 0);
    }

    #[test]
    fn test_insurance_eligibility_scenarios() {
        let bills = vec![
            DatabaseResource {
                id: Some(1),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(serde_json::json!({
                    "vehicle": { "id": "V1", "vehicleNumber": "BA-01-PA-0001" },
                    "fiscalYear": "2083/84",
                    "status": "PAID",
                    "items": [{ "ruleId": "5" }, { "ruleId": "5" }]
                })),
                external_content: None,
            },
            DatabaseResource {
                id: Some(2),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(serde_json::json!({
                    "vehicle": { "id": "V1", "vehicleNumber": "BA-01-PA-0001" },
                    "fiscalYear": "2083/84",
                    "status": "PAID",
                    "items": [{ "ruleId": "12" }]
                })),
                external_content: None,
            },
            DatabaseResource {
                id: Some(3),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(serde_json::json!({
                    "vehicle": { "id": "V1", "vehicleNumber": "BA-01-PA-0001" },
                    "fiscalYear": "2083/84",
                    "status": "CANCELLED",
                    "items": [{ "ruleId": "99" }]
                })),
                external_content: None,
            },
        ];
        let bill_refs: Vec<&DatabaseResource> = bills.iter().collect();

        // Disabled -> not enabled, not eligible
        let disabled = InsuranceEligibilitySettings::default();
        let r0 = evaluate_insurance_eligibility(&disabled, Some("V1"), None, None, &bill_refs);
        assert!(!r0.enabled);
        assert!(!r0.eligible);

        // Enabled with empty ruleIds & minPaidRules = 2 -> eligible (5 and 12 paid; 99 ignored)
        let any_two = InsuranceEligibilitySettings {
            enabled: true,
            rule_ids: vec![],
            min_paid_rules: 2,
        };
        let r1 = evaluate_insurance_eligibility(&any_two, Some("V1"), None, Some("2083/84"), &bill_refs);
        assert!(r1.eligible);
        assert_eq!(r1.paid_rule_ids, vec!["12", "5"]);

        // Specific ruleIds ["5", "24"] with minPaidRules = 2 -> only "5" matched, "24" missing -> false
        let specific_two = InsuranceEligibilitySettings {
            enabled: true,
            rule_ids: vec!["5".into(), "24".into()],
            min_paid_rules: 2,
        };
        let r2 = evaluate_insurance_eligibility(&specific_two, Some("V1"), None, None, &bill_refs);
        assert!(!r2.eligible);
        assert_eq!(r2.matched_rule_ids, vec!["5"]);
        assert_eq!(r2.missing_rule_ids, vec!["24"]);
    }

    #[test]
    fn test_fiscal_year_selector_and_billing_access_scenarios() {
        let mut settings = AppSettings::from_inner_value(
            full_user_setting_resource().inner_content.as_ref().unwrap(),
        );
        settings.fiscal_year.selector.archived_years = vec!["2079/80".to_string()];

        // 1. Selector with current BS start year = 2083, yearsBack=4 (2079..=2084) excluding 2079/80
        let sel = generate_fiscal_year_selector(&settings.fiscal_year.selector, 2083);
        assert_eq!(sel.current_fiscal_year, "2083/84");
        assert_eq!(
            sel.all_window_years,
            vec!["2079/80", "2080/81", "2081/82", "2082/83", "2083/84", "2084/85"]
        );
        assert_eq!(
            sel.selectable_years,
            vec!["2080/81", "2081/82", "2082/83", "2083/84", "2084/85"]
        );

        // 2. Billing Access: current allowed, previous CLOSED blocked (canonical 2082/2083 == 2082/83), next null blocked, archived blocked
        let acc_cur = check_fiscal_year_billing_access(&settings.fiscal_year, "2083/84", 2083);
        assert!(acc_cur.allowed);

        let acc_prev_closed =
            check_fiscal_year_billing_access(&settings.fiscal_year, "2082/2083", 2083);
        assert!(!acc_prev_closed.allowed);
        assert_eq!(acc_prev_closed.code, "FISCAL_YEAR_BILLING_CLOSED");
        assert_eq!(
            acc_prev_closed.updated_by.as_deref(),
            Some("System Administrator")
        );

        let acc_next_null = check_fiscal_year_billing_access(&settings.fiscal_year, "2084/85", 2083);
        assert!(!acc_next_null.allowed);
        assert_eq!(acc_next_null.code, "FISCAL_YEAR_BILLING_CLOSED");

        let acc_archived = check_fiscal_year_billing_access(&settings.fiscal_year, "2079/80", 2083);
        assert!(!acc_archived.allowed);
        assert_eq!(acc_archived.code, "FISCAL_YEAR_ARCHIVED");

        // 3. Reopening previousYear or nextYear -> allowed!
        settings.fiscal_year.billing_access.previous_year.as_mut().unwrap().status = "OPEN".to_string();
        settings.fiscal_year.billing_access.next_year = Some(FiscalYearAccessEntry {
            status: "OPEN".to_string(),
            updated_at: None,
            updated_by: Some("Admin".to_string()),
            fiscal_year: "2084/85".to_string(),
        });
        assert!(check_fiscal_year_billing_access(&settings.fiscal_year, "2082/83", 2083).allowed);
        assert!(check_fiscal_year_billing_access(&settings.fiscal_year, "2084/85", 2083).allowed);
    }

    #[actix_web::test]
    async fn test_settings_http_endpoints_end_to_end() {
        let mut state = crate::access::test_helpers::MockDbState::default();
        state.seed(ResourceName::SettingResource, full_user_setting_resource());
        let (s_data, _) =
            crate::access::test_helpers::create_mock_server_data_with_state(state).await;

        {
            let mut cache = s_data.cache.write().await;
            cache.load_from_db(&s_data.db).await;
            cache.write(
                ResourceName::BillResource,
                501,
                DatabaseResource {
                    id: Some(501),
                    resource_name: Some(ResourceName::BillResource),
                    inner_content: Some(serde_json::json!({
                        "vehicle": { "id": "V50", "vehicleNumber": "BA-01-PA-5050" },
                        "fiscalYear": "2083/84",
                        "paidAt": Utc::now().to_rfc3339(),
                        "billingMonths": 12,
                        "status": "PAID",
                        "items": [{ "ruleId": "45", "amount": 5000 }]
                    })),
                    external_content: None,
                },
            );
        }

        let app = web_test::init_service(
            App::new()
                .app_data(s_data.clone())
                .service(web::scope("/api/v1/settings").configure(configure_routes)),
        )
        .await;

        // 1. Resolved endpoint masks secrets for non-admin, reveals for Admin
        let user_resp = web_test::call_service(
            &app,
            web_test::TestRequest::get()
                .uri("/api/v1/settings/resolved")
                .to_request(),
        )
        .await;
        assert_eq!(user_resp.status(), StatusCode::OK);
        let user_json: Value = web_test::read_body_json(user_resp).await;
        assert_eq!(user_json["notifications"]["sms"]["apiKey"], "[REDACTED]");
        assert_eq!(user_json["integrations"]["tqms"]["SanghBranch"], "Loadboard sthaniya");

        let admin_req = web_test::TestRequest::get()
            .uri("/api/v1/settings/resolved")
            .to_request();
        admin_req.extensions_mut().insert(crate::auth::Claims {
            sub: "admin@test.com".to_string(),
            exp: 9999999999,
            iat: 1000000000,
            role: crate::roles::Roles::Admin,
            user_id: Some(1),
            jti: None,
            role_name: None,
            tenant_id: None,
        });
        let admin_resp = web_test::call_service(&app, admin_req).await;
        let admin_json: Value = web_test::read_body_json(admin_resp).await;
        assert_eq!(admin_json["notifications"]["sms"]["apiKey"], "36A1291E165814");

        // 2. VAT endpoint
        let vat_resp = web_test::call_service(
            &app,
            web_test::TestRequest::post()
                .uri("/api/v1/settings/billing/calculate-vat")
                .set_json(serde_json::json!({
                    "items": [{ "amount": 1000, "billingEntityId": "1" }],
                    "discount": 0
                }))
                .to_request(),
        )
        .await;
        assert_eq!(vat_resp.status(), StatusCode::OK);
        let vat_json: VatCalculationResult = web_test::read_body_json(vat_resp).await;
        assert_eq!(vat_json.vat_amount, 130.0);
        assert_eq!(vat_json.total, 1130.0);

        // 3. Arrears endpoint
        let arr_resp = web_test::call_service(
            &app,
            web_test::TestRequest::get()
                .uri("/api/v1/settings/billing/arrears/V50")
                .to_request(),
        )
        .await;
        assert_eq!(arr_resp.status(), StatusCode::OK);
        let arr_json: ArrearsEvaluationResult = web_test::read_body_json(arr_resp).await;
        assert!(!arr_json.has_arrears);

        // 4. Linked fees endpoint
        let linked_resp = web_test::call_service(
            &app,
            web_test::TestRequest::post()
                .uri("/api/v1/settings/billing/linked-fees/evaluate")
                .set_json(serde_json::json!({
                    "fiscalYear": "2082/83",
                    "requestedFeeHeadIds": ["3"]
                }))
                .to_request(),
        )
        .await;
        assert_eq!(linked_resp.status(), StatusCode::OK);
        let linked_json: LinkedFeeEvaluationResult = web_test::read_body_json(linked_resp).await;
        assert!(!linked_json.valid);
        assert_eq!(linked_json.missing_linked_fee_heads[0].linked_fee_head_id, "21");

        // 5. Cancellation check endpoint
        let cancel_resp = web_test::call_service(
            &app,
            web_test::TestRequest::get()
                .uri("/api/v1/settings/receipts/cancellation-check/501")
                .to_request(),
        )
        .await;
        assert_eq!(cancel_resp.status(), StatusCode::OK);
        let cancel_json: ReceiptCancellationCheck = web_test::read_body_json(cancel_resp).await;
        assert!(!cancel_json.enabled);
        assert!(cancel_json.allowed);

        // 6. Insurance eligibility endpoint
        let ins_resp = web_test::call_service(
            &app,
            web_test::TestRequest::get()
                .uri("/api/v1/settings/insurance/eligibility/V50")
                .to_request(),
        )
        .await;
        assert_eq!(ins_resp.status(), StatusCode::OK);
        let ins_json: InsuranceEligibilityResult = web_test::read_body_json(ins_resp).await;
        assert!(!ins_json.enabled);

        // 7. Fiscal year selector & billing access (including slash in 2082/83)
        let sel_resp = web_test::call_service(
            &app,
            web_test::TestRequest::get()
                .uri("/api/v1/settings/fiscal-year/selector")
                .to_request(),
        )
        .await;
        assert_eq!(sel_resp.status(), StatusCode::OK);

        let fy_resp = web_test::call_service(
            &app,
            web_test::TestRequest::get()
                .uri("/api/v1/settings/fiscal-year/access/2082/83")
                .to_request(),
        )
        .await;
        assert_eq!(fy_resp.status(), StatusCode::OK);
        let fy_json: FiscalYearBillingAccessResult = web_test::read_body_json(fy_resp).await;
        assert!(!fy_json.allowed);
        assert_eq!(fy_json.code, "FISCAL_YEAR_BILLING_CLOSED");

        // 8. Displaced tagging endpoint
        let disp_resp = web_test::call_service(
            &app,
            web_test::TestRequest::get()
                .uri("/api/v1/settings/vehicle-management/displaced-tagging")
                .to_request(),
        )
        .await;
        assert_eq!(disp_resp.status(), StatusCode::OK);
        let disp_json: Value = web_test::read_body_json(disp_resp).await;
        assert_eq!(disp_json["enabled"], true);
    }
}
