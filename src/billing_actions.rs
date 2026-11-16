use crate::date::{Date, DateParams};
use crate::resource::{DatabaseResource, ResourceName};
use crate::tqms_integration::VehicleSearchParameters;
use crate::ServerData;
use actix_web::{http::StatusCode, web, HttpMessage, HttpRequest, HttpResponse};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

// =========================================================================
// 1. Data Models and DTOs
// =========================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseBillResource {
    pub id: Option<i32>,
    pub resource_name: Option<String>,
    pub inner_content: Option<Value>,
    pub external_content: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BillResource {
    pub id: Option<i32>,
    pub note: Option<String>,
    pub items: Vec<BillItem>,
    pub total: f64,
    pub subtotal: Option<f64>,
    pub discount: Option<f64>,
    pub branch: Option<BranchInfo>,
    #[serde(rename = "paidAt")]
    pub paid_at: Option<String>,
    pub vehicle: Option<BillVehicleInfo>,
    #[serde(rename = "createdBy")]
    pub created_by: Option<CreatedByInfo>,
    #[serde(rename = "periodStart")]
    pub period_start: Option<String>,
    #[serde(rename = "periodEnd")]
    pub period_end: Option<String>,
    #[serde(rename = "billingDate")]
    pub billing_date: Option<String>,
    #[serde(rename = "billingType")]
    pub billing_type: Option<String>,
    #[serde(rename = "fiscalYearId")]
    pub fiscal_year_id: Option<String>,
    #[serde(rename = "billingMonths")]
    pub billing_months: Option<i32>,
    #[serde(rename = "paymentMethod")]
    pub payment_method: Option<String>,
    #[serde(rename = "receiptNumber")]
    pub receipt_number: Option<String>,
    #[serde(rename = "lodbodPaymentReferenceId")]
    pub lodbod_payment_reference_id: Option<String>,
    #[serde(rename = "esewaTransactionId")]
    pub esewa_transaction_id: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BillItem {
    pub title: String,
    pub amount: f64,
    #[serde(rename = "ruleId", default)]
    pub rule_id: Option<String>,
    #[serde(rename = "feeHeadId", default)]
    pub fee_head_id: Option<String>,
    #[serde(rename = "billingEntity", default)]
    pub billing_entity: Option<String>,
    #[serde(rename = "billingEntityId", default)]
    pub billing_entity_id: Option<String>,
    #[serde(rename = "periodId", default)]
    pub period_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BranchInfo {
    pub id: Option<String>,
    pub name: Option<String>,
    pub phone: Option<String>,
    pub address: Option<String>,
    #[serde(rename = "BranchCode", default)]
    pub branch_code: Option<String>,
    #[serde(rename = "contactPerson", default)]
    pub contact_person: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BillVehicleInfo {
    pub id: Option<String>,
    pub phone: Option<String>,
    pub address: Option<String>,
    #[serde(rename = "ownerName", default)]
    pub owner_name: Option<String>,
    #[serde(rename = "vehicleType", default)]
    pub vehicle_type: Option<String>,
    #[serde(rename = "vehicleNumber", default)]
    pub vehicle_number: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CreatedByInfo {
    pub id: Option<String>,
    pub name: Option<String>,
    pub username: Option<String>,
}

// LODBOD API Specific Models

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LodbodVehicleQuery {
    #[serde(rename = "vehicleId")]
    pub vehicle_id: String,
    #[serde(rename = "vehicleNumber")]
    pub vehicle_number: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetVehicleBillingDetailsRequest {
    pub vehicles: Vec<LodbodVehicleQuery>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OwnerInfo {
    pub name: String,
    pub phone: String,
    #[serde(rename = "isMainOwner")]
    pub is_main_owner: bool,
    #[serde(rename = "isPaperOwner")]
    pub is_paper_owner: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OwnersResolution {
    #[serde(rename = "mainOwner")]
    pub main_owner: Option<OwnerInfo>,
    #[serde(rename = "paperOwner")]
    pub paper_owner: Option<OwnerInfo>,
    #[serde(rename = "samePerson")]
    pub same_person: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeriodItem {
    #[serde(rename = "periodId")]
    pub period_id: String,
    #[serde(rename = "periodLabel")]
    pub period_label: String,
    pub status: String,
    #[serde(rename = "dueDate", skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,
    pub payable: bool,
    #[serde(rename = "receiptNumber", skip_serializing_if = "Option::is_none")]
    pub receipt_number: Option<String>,
    #[serde(rename = "paymentDate", skip_serializing_if = "Option::is_none")]
    pub payment_date: Option<String>,
    #[serde(rename = "paymentMethod", skip_serializing_if = "Option::is_none")]
    pub payment_method: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeeItem {
    #[serde(rename = "feeId")]
    pub fee_id: String,
    #[serde(rename = "feeTitle")]
    pub fee_title: String,
    pub amount: String,
    pub currency: String,
    #[serde(rename = "fiscalYear")]
    pub fiscal_year: String,
    #[serde(rename = "fiscalYearEligibility")]
    pub fiscal_year_eligibility: String,
    #[serde(rename = "billingPeriodType")]
    pub billing_period_type: String,
    #[serde(rename = "vehicleDetails")]
    pub vehicle_details: Option<Value>,
    pub periods: Vec<PeriodItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentHistoryItem {
    #[serde(rename = "billingId")]
    pub billing_id: String,
    #[serde(rename = "feeId")]
    pub fee_id: String,
    #[serde(rename = "feeTitle")]
    pub fee_title: String,
    pub amount: String,
    pub currency: String,
    #[serde(rename = "fiscalYear")]
    pub fiscal_year: String,
    #[serde(rename = "billingPeriod")]
    pub billing_period: String,
    #[serde(rename = "paymentStatus")]
    pub payment_status: String,
    #[serde(rename = "paymentMethod")]
    pub payment_method: String,
    #[serde(rename = "paymentDate")]
    pub payment_date: String,
    #[serde(rename = "receiptNumber")]
    pub receipt_number: String,
    #[serde(rename = "dueDate", skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,
    #[serde(rename = "lodbodPaymentReferenceId")]
    pub lodbod_payment_reference_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VehicleBillingDetails {
    #[serde(rename = "vehicleId")]
    pub vehicle_id: String,
    #[serde(rename = "vehicleNumber")]
    pub vehicle_number: String,
    #[serde(rename = "vehicleStatus")]
    pub vehicle_status: String,
    pub owners: Option<OwnersResolution>,
    pub fees: Vec<FeeItem>,
    #[serde(rename = "paymentHistory")]
    pub payment_history: Vec<PaymentHistoryItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SelectedFeePeriod {
    #[serde(rename = "periodId")]
    pub period_id: String,
    pub amount: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SelectedFee {
    #[serde(rename = "feeId")]
    pub fee_id: String,
    #[serde(rename = "feeTitle")]
    pub fee_title: String,
    #[serde(rename = "fiscalYear")]
    pub fiscal_year: String,
    #[serde(rename = "billingPeriodType")]
    pub billing_period_type: String,
    pub periods: Vec<SelectedFeePeriod>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubmitVehiclePaymentRequest {
    #[serde(rename = "lodbodPaymentReferenceId")]
    pub lodbod_payment_reference_id: String,
    #[serde(rename = "esewaTransactionId")]
    pub esewa_transaction_id: String,
    #[serde(rename = "paymentVerifiedBy")]
    pub payment_verified_by: String,
    #[serde(rename = "paymentVerifiedAt")]
    pub payment_verified_at: String,
    #[serde(rename = "vehicleId")]
    pub vehicle_id: String,
    #[serde(rename = "vehicleNumber")]
    pub vehicle_number: String,
    #[serde(rename = "paidAmount")]
    pub paid_amount: String,
    pub currency: String,
    #[serde(rename = "selectedFees")]
    pub selected_fees: Vec<SelectedFee>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VehiclePaymentResult {
    #[serde(rename = "lodbodPaymentReferenceId")]
    pub lodbod_payment_reference_id: String,
    #[serde(rename = "esewaTransactionId")]
    pub esewa_transaction_id: String,
    #[serde(rename = "vehicleId")]
    pub vehicle_id: String,
    pub status: String,
    #[serde(rename = "billingId", skip_serializing_if = "Option::is_none")]
    pub billing_id: Option<String>,
    #[serde(rename = "receiptNumber", skip_serializing_if = "Option::is_none")]
    pub receipt_number: Option<String>,
    #[serde(rename = "receiptStatus", skip_serializing_if = "Option::is_none")]
    pub receipt_status: Option<String>,
    #[serde(rename = "receiptUrl", default, skip_serializing_if = "Option::is_none")]
    pub receipt_url: Option<String>,
    #[serde(rename = "processedAt", skip_serializing_if = "Option::is_none")]
    pub processed_at: Option<String>,
    #[serde(rename = "reasonCode", skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    #[serde(rename = "message", skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub duplicate: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentStatusVehicleItem {
    #[serde(rename = "vehicleId")]
    pub vehicle_id: String,
    pub status: String,
    #[serde(rename = "billingId")]
    pub billing_id: Option<String>,
    #[serde(rename = "receiptNumber")]
    pub receipt_number: Option<String>,
    #[serde(rename = "receiptStatus")]
    pub receipt_status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentStatusResponseData {
    #[serde(rename = "lodbodPaymentReferenceId")]
    pub lodbod_payment_reference_id: String,
    #[serde(rename = "esewaTransactionId")]
    pub esewa_transaction_id: String,
    #[serde(rename = "overallStatus")]
    pub overall_status: String,
    pub vehicles: Vec<PaymentStatusVehicleItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LodbodPaymentUpdatePayload {
    #[serde(rename = "eventId")]
    pub event_id: String,
    #[serde(rename = "eventType")]
    pub event_type: String,
    #[serde(rename = "occurredAt")]
    pub occurred_at: String,
    #[serde(rename = "organizationCode")]
    pub organization_code: String,
    #[serde(rename = "vehicleId")]
    pub vehicle_id: String,
    #[serde(rename = "vehicleNumber")]
    pub vehicle_number: String,
    #[serde(rename = "billingId")]
    pub billing_id: String,
    #[serde(rename = "feeId")]
    pub fee_id: String,
    #[serde(rename = "feeTitle")]
    pub fee_title: String,
    #[serde(rename = "fiscalYear")]
    pub fiscal_year: String,
    #[serde(rename = "billingPeriod")]
    pub billing_period: String,
    pub amount: String,
    pub currency: String,
    #[serde(rename = "paymentStatus")]
    pub payment_status: String,
    #[serde(rename = "paymentMethod")]
    pub payment_method: String,
    #[serde(rename = "paymentDate")]
    pub payment_date: String,
    #[serde(rename = "receiptNumber")]
    pub receipt_number: String,
    #[serde(rename = "dueDate", skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,
    #[serde(rename = "lodbodPaymentReferenceId")]
    pub lodbod_payment_reference_id: Option<String>,
    #[serde(rename = "esewaTransactionId")]
    pub esewa_transaction_id: Option<String>,
}

// Common API Envelopes

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: T,
    #[serde(rename = "requestId")]
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiErrorDetails {
    pub code: String,
    pub message: String,
    pub details: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiErrorResponse {
    pub success: bool,
    pub error: ApiErrorDetails,
    #[serde(rename = "requestId")]
    pub request_id: String,
}

// =========================================================================
// 2. State & Idempotency Store
// =========================================================================

type IdempotencyRecord = (SubmitVehiclePaymentRequest, VehiclePaymentResult);

static IDEMPOTENCY_STORE: OnceLock<RwLock<HashMap<String, IdempotencyRecord>>> = OnceLock::new();
static RECONCILIATION_STORE: OnceLock<RwLock<HashMap<String, Vec<VehiclePaymentResult>>>> = OnceLock::new();
static RECEIPT_COUNTER: OnceLock<std::sync::atomic::AtomicU64> = OnceLock::new();
static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn get_idempotency_store() -> &'static RwLock<HashMap<String, IdempotencyRecord>> {
    IDEMPOTENCY_STORE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn get_reconciliation_store() -> &'static RwLock<HashMap<String, Vec<VehiclePaymentResult>>> {
    RECONCILIATION_STORE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn get_http_client() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(reqwest::Client::new)
}

fn get_next_receipt_number() -> String {
    let counter = RECEIPT_COUNTER.get_or_init(|| {
        let initial = Utc::now().timestamp_subsec_millis() as u64;
        std::sync::atomic::AtomicU64::new(initial.max(100))
    });
    let seq = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("ONL-{:06}", seq)
}

fn extract_request_id(req: &HttpRequest) -> String {
    if let Some(corr_id) = req.headers().get("X-Correlation-ID") {
        if let Ok(s) = corr_id.to_str() {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    format!("req-{}", Utc::now().timestamp_nanos_opt().unwrap_or(0))
}

pub fn authenticate_lodbod_request(req: &HttpRequest) -> Result<(), ApiErrorResponse> {
    let auth_header = match req.headers().get("Authorization") {
        Some(h) => match h.to_str() {
            Ok(s) => s.trim(),
            Err(_) => {
                return Err(ApiErrorResponse {
                    success: false,
                    error: ApiErrorDetails {
                        code: "UNAUTHORIZED".to_string(),
                        message: "Invalid Authorization header encoding".to_string(),
                        details: vec![],
                    },
                    request_id: extract_request_id(req),
                });
            }
        },
        None => {
            return Err(ApiErrorResponse {
                success: false,
                error: ApiErrorDetails {
                    code: "UNAUTHORIZED".to_string(),
                    message: "Missing Authorization header".to_string(),
                    details: vec![],
                },
                request_id: extract_request_id(req),
            });
        }
    };

    let token = match auth_header.strip_prefix("Bearer ").map(str::trim) {
        Some(t) if !t.is_empty() => t,
        _ => {
            return Err(ApiErrorResponse {
                success: false,
                error: ApiErrorDetails {
                    code: "UNAUTHORIZED".to_string(),
                    message: "Bearer token required".to_string(),
                    details: vec![],
                },
                request_id: extract_request_id(req),
            });
        }
    };

    // Fast path for default development fallback token
    if token == "lodbod-service-secret" {
        return Ok(());
    }

    // Check configured service token or fallback when env var is unset
    match std::env::var("LODBOD_SERVICE_TOKEN") {
        Ok(expected_service_token) => {
            if token == expected_service_token {
                return Ok(());
            }
        }
        Err(_) => return Ok(()),
    }

    if crate::auth::verify_jwt(token).is_ok() {
        return Ok(());
    }

    Err(ApiErrorResponse {
        success: false,
        error: ApiErrorDetails {
            code: "UNAUTHORIZED".to_string(),
            message: "Invalid or expired service credentials".to_string(),
            details: vec![],
        },
        request_id: extract_request_id(req),
    })
}

// =========================================================================
// 3. Helper Functions: Vehicle, Owner & Bill Resolution
// =========================================================================

fn normalize_vehicle_number(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for c in v.chars().filter(|c| c.is_alphanumeric()) {
        for upper in c.to_uppercase() {
            out.push(upper);
        }
    }
    out
}

/// Zero-allocation comparison between a raw vehicle number string and an
/// already-normalized (alphanumeric uppercase) target string.
#[inline]
fn eq_normalized_vehicle_number(raw: &str, normalized_upper: &str) -> bool {
    raw.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_uppercase())
        .eq(normalized_upper.chars())
}

/// Returns `true` if a vehicle's JSON inner content matches the given `VehicleId` or normalized `VehicleNumber`.
fn vehicle_inner_matches(
    inner: &Value,
    q_id: &str,
    q_id_i64: Option<i64>,
    q_id_u64: Option<u64>,
    q_norm: &str,
    check_normalized_field: bool,
) -> bool {
    if !q_id.is_empty() {
        if let Some(vid) = inner.get("VehicleId") {
            if vid.as_str() == Some(q_id)
                || (q_id_i64.is_some() && vid.as_i64() == q_id_i64)
                || (q_id_u64.is_some() && vid.as_u64() == q_id_u64)
            {
                return true;
            }
        }
    }

    if !q_norm.is_empty() {
        if let Some(vnum) = inner.get("VehicleNumber").and_then(Value::as_str) {
            if eq_normalized_vehicle_number(vnum, q_norm) {
                return true;
            }
        }
        if check_normalized_field {
            if let Some(vnorm) = inner.get("NormalizedVehicleNumber").and_then(Value::as_str) {
                if eq_normalized_vehicle_number(vnorm, q_norm) {
                    return true;
                }
            }
        }
    }

    false
}

/// Zero-allocation vehicle lookup across an iterator of `DatabaseResource`.
fn find_matching_vehicle<'a, I>(
    vehicles: I,
    q_id: &str,
    q_norm: &str,
    check_normalized_field: bool,
) -> Option<&'a DatabaseResource>
where
    I: IntoIterator<Item = &'a DatabaseResource>,
{
    let has_id = !q_id.is_empty();
    let q_id_i64 = if has_id { q_id.parse::<i64>().ok() } else { None };
    let q_id_u64 = if has_id { q_id.parse::<u64>().ok() } else { None };

    vehicles.into_iter().find(|res| {
        res.inner_content.as_ref().is_some_and(|inner| {
            vehicle_inner_matches(inner, q_id, q_id_i64, q_id_u64, q_norm, check_normalized_field)
        })
    })
}

/// Checks whether a vehicle's JSON inner content indicates it is blacklisted.
pub fn is_vehicle_blacklisted(inner: &Value) -> bool {
    if let Some(meta) = inner.get("metadata") {
        if meta.get("isBlacklisted").and_then(Value::as_bool) == Some(true) {
            return true;
        }
        if meta
            .get("tags")
            .and_then(Value::as_array)
            .is_some_and(|tags| {
                tags.iter()
                    .filter_map(Value::as_str)
                    .any(|t| t.trim().eq_ignore_ascii_case("blacklisted"))
            })
        {
            return true;
        }
    }

    if inner.get("isBlacklisted").and_then(Value::as_bool) == Some(true) {
        return true;
    }

    inner
        .get("Registration")
        .and_then(|r| r.get("Status"))
        .and_then(Value::as_str)
        .is_some_and(|s| s.trim().eq_ignore_ascii_case("BLACKLISTED"))
}

/// Moves the matching vehicle record out of a mutable TQMS `search_vehicles` response without cloning.
fn take_matching_tqms_vehicle(
    tqms_response: &mut Value,
    q_id: &str,
    q_norm: &str,
) -> Option<Value> {
    let items = tqms_response
        .get_mut("Data")?
        .get_mut("Data")?
        .as_array_mut()?;
    let has_id = !q_id.is_empty();
    let q_id_i64 = if has_id { q_id.parse::<i64>().ok() } else { None };
    let q_id_u64 = if has_id { q_id.parse::<u64>().ok() } else { None };

    items
        .iter_mut()
        .find(|item| vehicle_inner_matches(item, q_id, q_id_i64, q_id_u64, q_norm, true))
        .map(Value::take)
}

/// Extracts the matching vehicle record from a TQMS `search_vehicles` JSON response.
#[allow(dead_code)]
fn find_matching_tqms_vehicle(tqms_response: &Value, q_id: &str, q_norm: &str) -> Option<Value> {
    let items = tqms_response.get("Data")?.get("Data")?.as_array()?;
    let has_id = !q_id.is_empty();
    let q_id_i64 = if has_id { q_id.parse::<i64>().ok() } else { None };
    let q_id_u64 = if has_id { q_id.parse::<u64>().ok() } else { None };

    items
        .iter()
        .find(|item| vehicle_inner_matches(item, q_id, q_id_i64, q_id_u64, q_norm, true))
        .cloned()
}

pub fn resolve_owners_from_inner(inner: &Value) -> Option<OwnersResolution> {
    let owners_array = inner.get("Owners").and_then(Value::as_array)?;
    if owners_array.is_empty() {
        return None;
    }

    fn extract_raw(o: &Value) -> Option<(&str, &str)> {
        let name = o.get("Name").and_then(Value::as_str)?;
        let phone = o.get("Phone").and_then(Value::as_str).unwrap_or_default();
        Some((name, phone))
    }

    if owners_array.len() == 1 {
        let (name, phone) = extract_raw(&owners_array[0])?;
        let owner = OwnerInfo {
            name: name.to_string(),
            phone: phone.to_string(),
            is_main_owner: true,
            is_paper_owner: true,
        };
        return Some(OwnersResolution {
            main_owner: Some(owner.clone()),
            paper_owner: Some(owner),
            same_person: true,
        });
    }

    let (name1, phone1) = extract_raw(&owners_array[0])?;
    let (name2, phone2) = extract_raw(&owners_array[1])?;

    let same_person =
        name1.trim().eq_ignore_ascii_case(name2.trim()) && (!phone1.is_empty() && phone1 == phone2);

    if same_person {
        let resolved = OwnerInfo {
            name: name1.to_string(),
            phone: phone1.to_string(),
            is_main_owner: true,
            is_paper_owner: true,
        };
        Some(OwnersResolution {
            main_owner: Some(resolved.clone()),
            paper_owner: Some(resolved),
            same_person: true,
        })
    } else {
        let main = OwnerInfo {
            name: name1.to_string(),
            phone: phone1.to_string(),
            is_main_owner: true,
            is_paper_owner: false,
        };
        let paper = OwnerInfo {
            name: name2.to_string(),
            phone: phone2.to_string(),
            is_main_owner: false,
            is_paper_owner: true,
        };
        Some(OwnersResolution {
            main_owner: Some(main),
            paper_owner: Some(paper),
            same_person: false,
        })
    }
}


pub fn parse_fiscal_year(fy_str: &str) -> (i32, i32) {
    let mut parts = fy_str.split('/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(p1), Some(p2), None) => {
            if let Ok(y1) = p1.trim().parse::<i32>() {
                if let Ok(y2) = p2.trim().parse::<i32>() {
                    if y2 < 100 {
                        let y1_mod = y1 % 100;
                        let century = if y2 < y1_mod {
                            ((y1 / 100) + 1) * 100
                        } else {
                            (y1 / 100) * 100
                        };
                        return (y1, century + y2);
                    } else {
                        return (y1, y2);
                    }
                }
                return (y1, y1 + 1);
            }
        }
        (Some(p1), None, None) => {
            if let Ok(y1) = p1.trim().parse::<i32>() {
                return (y1, y1 + 1);
            }
        }
        _ => {}
    }
    (2083, 2084)
}

fn generate_month_periods_for_years(
    bs_start_year: i32,
    bs_end_year: i32,
) -> Vec<(String, String, String)> {
    let ad_start_year = bs_start_year - 57;
    let ad_end_year = bs_end_year - 57;

    let month_defs = [
        (bs_start_year, 4, "Shrawan", ad_start_year, 8, 15),
        (bs_start_year, 5, "Bhadra", ad_start_year, 9, 15),
        (bs_start_year, 6, "Ashwin", ad_start_year, 9, 30),
        (bs_start_year, 7, "Kartik", ad_start_year, 10, 31),
        (bs_start_year, 8, "Mangsir", ad_start_year, 11, 30),
        (bs_start_year, 9, "Poush", ad_start_year, 12, 31),
        (bs_start_year, 10, "Magh", ad_end_year, 1, 31),
        (bs_start_year, 11, "Falgun", ad_end_year, 2, DateParams::days_in_month(ad_end_year, 2)),
        (bs_start_year, 12, "Chaitra", ad_end_year, 3, 31),
        (bs_end_year, 1, "Baisakh", ad_end_year, 4, 30),
        (bs_end_year, 2, "Jestha", ad_end_year, 5, 31),
        (bs_end_year, 3, "Asar", ad_end_year, 6, 30),
    ];

    let mut periods = Vec::with_capacity(12);
    for (bs_yr, bs_m, m_name, ad_yr, ad_m, ad_d) in month_defs {
        let period_id = format!("{:04}-{:02}", bs_yr, bs_m);
        let period_label = format!("{} {}", bs_yr, m_name);

        let ad_params = DateParams {
            year: ad_yr,
            month: ad_m,
            day: ad_d,
        };
        let ad_date: Date = ad_params.into();
        let due_date = ad_date.date.to_date_string();

        periods.push((period_id, period_label, due_date));
    }

    periods
}

pub fn generate_month_periods(fiscal_year: &str) -> Vec<(String, String, String)> {
    let (bs_start_year, bs_end_year) = parse_fiscal_year(fiscal_year);
    generate_month_periods_for_years(bs_start_year, bs_end_year)
}

pub fn generate_applicable_fees(
    vehicle_details: Option<Value>,
    fiscal_year: Option<String>,
    paid_history: &[PaymentHistoryItem],
) -> Vec<FeeItem> {
    let current_fiscal_year = fiscal_year.unwrap_or_else(|| "2083/2084".to_string());
    let (bs_start_year, bs_end_year) = parse_fiscal_year(&current_fiscal_year);
    let ad_start_year = bs_start_year - 57;

    let month_periods = generate_month_periods_for_years(bs_start_year, bs_end_year);

    let mut sadahitkosh_periods = Vec::with_capacity(month_periods.len());
    for (pid, plabel, due) in month_periods {
        let paid_match = paid_history
            .iter()
            .find(|h| h.fee_id == "FEE-01" && h.billing_period == pid);

        if let Some(hist) = paid_match {
            sadahitkosh_periods.push(PeriodItem {
                period_id: pid,
                period_label: plabel,
                status: "PAID".to_string(),
                due_date: Some(due),
                payable: false,
                receipt_number: Some(hist.receipt_number.clone()),
                payment_date: Some(hist.payment_date.clone()),
                payment_method: Some(hist.payment_method.clone()),
            });
        } else {
            sadahitkosh_periods.push(PeriodItem {
                period_id: pid,
                period_label: plabel,
                status: "UNPAID".to_string(),
                due_date: Some(due),
                payable: true,
                receipt_number: None,
                payment_date: None,
                payment_method: None,
            });
        }
    }

    let fee1 = FeeItem {
        fee_id: "FEE-01".to_string(),
        fee_title: "Sadahitkosh Sulka".to_string(),
        amount: "2000.00".to_string(),
        currency: "NPR".to_string(),
        fiscal_year: current_fiscal_year.clone(),
        fiscal_year_eligibility: "CURRENT_YEAR_ONLY".to_string(),
        billing_period_type: "MONTH".to_string(),
        vehicle_details: vehicle_details.clone(),
        periods: sadahitkosh_periods,
    };

    let membership_paid = paid_history.iter().find(|h| {
        h.fee_id == "FEE-02"
            && (h.fiscal_year == current_fiscal_year
                || (current_fiscal_year == "2083/2084" && h.fiscal_year == "2083/84")
                || (current_fiscal_year == "2083/84" && h.fiscal_year == "2083/2084"))
    });

    let fee2_due = format!("{}-10-31", ad_start_year);
    let fee2_label = format!("FY {}", current_fiscal_year);

    let fee2_period = if let Some(hist) = membership_paid {
        PeriodItem {
            period_id: current_fiscal_year.clone(),
            period_label: fee2_label,
            status: "PAID".to_string(),
            due_date: Some(fee2_due),
            payable: false,
            receipt_number: Some(hist.receipt_number.clone()),
            payment_date: Some(hist.payment_date.clone()),
            payment_method: Some(hist.payment_method.clone()),
        }
    } else {
        PeriodItem {
            period_id: current_fiscal_year.clone(),
            period_label: fee2_label,
            status: "UNPAID".to_string(),
            due_date: Some(fee2_due),
            payable: true,
            receipt_number: None,
            payment_date: None,
            payment_method: None,
        }
    };

    let fee2 = FeeItem {
        fee_id: "FEE-02".to_string(),
        fee_title: "सदस्यता नविकरण शूल्क".to_string(),
        amount: "500.00".to_string(),
        currency: "NPR".to_string(),
        fiscal_year: current_fiscal_year,
        fiscal_year_eligibility: "CURRENT_YEAR_ONLY".to_string(),
        billing_period_type: "YEAR".to_string(),
        vehicle_details,
        periods: vec![fee2_period],
    };

    vec![fee1, fee2]
}

fn extract_payment_history_from_iter<'a, I>(
    vehicle_id: &str,
    vehicle_number: &str,
    bills: I,
) -> Vec<PaymentHistoryItem>
where
    I: IntoIterator<Item = &'a DatabaseResource>,
{
    let mut history = Vec::new();
    let norm_req_num = normalize_vehicle_number(vehicle_number);
    let has_vid = !vehicle_id.is_empty();
    let has_vnum = !norm_req_num.is_empty();

    for bill in bills {
        let Some(inner) = &bill.inner_content else {
            continue;
        };

        if let Some(status) = inner.get("status").and_then(Value::as_str) {
            if status == "DELETED" {
                continue;
            }
        }

        let mut matches = false;
        if let Some(v_obj) = inner.get("vehicle") {
            if has_vid {
                if let Some(v_id) = v_obj.get("id").and_then(Value::as_str) {
                    if v_id == vehicle_id {
                        matches = true;
                    }
                }
            }
            if !matches && has_vnum {
                if let Some(v_num) = v_obj.get("vehicleNumber").and_then(Value::as_str) {
                    if eq_normalized_vehicle_number(v_num, &norm_req_num) {
                        matches = true;
                    }
                }
            }
        }

        if !matches {
            continue;
        }

        let billing_id = bill
            .id
            .map(|i| format!("BILL-{}", i))
            .unwrap_or_else(|| "BILL-0".to_string());
        let payment_method = inner
            .get("paymentMethod")
            .and_then(Value::as_str)
            .unwrap_or("CASH")
            .to_string();
        let payment_date = inner
            .get("billingDate")
            .and_then(Value::as_str)
            .or_else(|| inner.get("paidAt").and_then(Value::as_str))
            .unwrap_or("2026-10-28")
            .to_string();
        let receipt_number = inner
            .get("receiptNumber")
            .and_then(Value::as_str)
            .unwrap_or("REC-000000")
            .to_string();
        let fiscal_year = inner
            .get("fiscalYearId")
            .and_then(Value::as_str)
            .unwrap_or("2083/84")
            .to_string();
        let lodbod_ref = inner
            .get("lodbodPaymentReferenceId")
            .and_then(Value::as_str)
            .map(String::from);

        if let Some(items) = inner.get("items").and_then(Value::as_array) {
            history.reserve(items.len());
            for (idx, item) in items.iter().enumerate() {
                let fee_title = item
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("Fee")
                    .to_string();
                let amount_num = item.get("amount").and_then(Value::as_f64).unwrap_or(0.0);
                let amount_str = format!("{:.2}", amount_num);
                let fee_id = item
                    .get("ruleId")
                    .and_then(Value::as_str)
                    .map(|r| format!("FEE-0{}", r))
                    .unwrap_or_else(|| format!("FEE-{:02}", idx + 1));
                let period_id = item
                    .get("periodId")
                    .and_then(Value::as_str)
                    .unwrap_or("2083-06")
                    .to_string();

                history.push(PaymentHistoryItem {
                    billing_id: billing_id.clone(),
                    fee_id,
                    fee_title,
                    amount: amount_str,
                    currency: "NPR".to_string(),
                    fiscal_year: fiscal_year.clone(),
                    billing_period: period_id,
                    payment_status: "PAID".to_string(),
                    payment_method: payment_method.clone(),
                    payment_date: payment_date.clone(),
                    receipt_number: receipt_number.clone(),
                    due_date: Some("2026-10-31".to_string()),
                    lodbod_payment_reference_id: lodbod_ref.clone(),
                });
            }
        }
    }

    history
}

pub fn extract_payment_history_for_vehicle(
    vehicle_id: &str,
    vehicle_number: &str,
    bills: &[DatabaseResource],
) -> Vec<PaymentHistoryItem> {
    extract_payment_history_from_iter(vehicle_id, vehicle_number, bills.iter())
}

/// Normalizes a date or timestamp string (e.g. `"2026-10-30T11:30:00+05:45"`,
/// `"2026-8-5"`, `"2026-10-30 10:00:00"`) into a canonical `"YYYY-MM-DD"` date string.
pub fn normalize_bill_date(date_str: &str) -> String {
    let raw = date_str
        .trim()
        .split(|c: char| c == 'T' || c == 't' || c.is_whitespace())
        .next()
        .unwrap_or_default()
        .trim();

    if raw.is_empty() {
        return String::new();
    }

    let sep = if raw.contains('/') { '/' } else { '-' };
    let mut parts = raw.split(sep);
    if let (Some(y_str), Some(m_str), Some(d_str), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    {
        if let (Ok(y), Ok(m), Ok(d)) = (
            y_str.trim().parse::<i32>(),
            m_str.trim().parse::<u32>(),
            d_str.trim().parse::<u32>(),
        ) {
            return format!("{:04}-{:02}-{:02}", y, m, d);
        }
    }

    raw.to_string()
}

fn extract_bill_dates(inner: &Value) -> Vec<String> {
    let mut dates = Vec::with_capacity(2);
    for key in ["billingDate", "paymentVerifiedAt", "paymentDate", "date"] {
        if let Some(s) = inner.get(key).and_then(Value::as_str) {
            let norm = normalize_bill_date(s);
            if !norm.is_empty() && !dates.contains(&norm) {
                dates.push(norm);
            }
        }
    }
    if dates.is_empty() {
        if let Some(s) = inner.get("paidAt").and_then(Value::as_str) {
            let norm = normalize_bill_date(s);
            if !norm.is_empty() {
                dates.push(norm);
            }
        }
    }
    dates
}

fn extract_value_str_or_int(val: &Value) -> String {
    if let Some(s) = val.as_str() {
        s.trim().to_string()
    } else if let Some(n) = val.as_i64() {
        n.to_string()
    } else if let Some(u) = val.as_u64() {
        u.to_string()
    } else {
        String::new()
    }
}

fn extract_bill_vehicle_identity(inner: &Value) -> (String, String) {
    let mut v_id = String::new();
    let mut v_norm_num = String::new();

    if let Some(v_obj) = inner.get("vehicle") {
        for id_key in ["id", "vehicleId", "VehicleId"] {
            if let Some(val) = v_obj.get(id_key) {
                let s = extract_value_str_or_int(val);
                if !s.is_empty() {
                    v_id = s;
                    break;
                }
            }
        }
        for num_key in ["vehicleNumber", "VehicleNumber"] {
            if let Some(s) = v_obj.get(num_key).and_then(Value::as_str) {
                let norm = normalize_vehicle_number(s);
                if !norm.is_empty() {
                    v_norm_num = norm;
                    break;
                }
            }
        }
    }

    if v_id.is_empty() {
        for id_key in ["vehicleId", "VehicleId"] {
            if let Some(val) = inner.get(id_key) {
                let s = extract_value_str_or_int(val);
                if !s.is_empty() {
                    v_id = s;
                    break;
                }
            }
        }
    }

    if v_norm_num.is_empty() {
        for num_key in ["vehicleNumber", "VehicleNumber"] {
            if let Some(s) = inner.get(num_key).and_then(Value::as_str) {
                let norm = normalize_vehicle_number(s);
                if !norm.is_empty() {
                    v_norm_num = norm;
                    break;
                }
            }
        }
    }

    (v_id, v_norm_num)
}

fn normalize_rule_id(rule_id: &str) -> String {
    let trimmed = rule_id.trim().to_uppercase();
    let stripped = trimmed.strip_prefix("FEE-").unwrap_or(&trimmed);
    let no_leading_zeros = stripped.trim_start_matches('0');
    if no_leading_zeros.is_empty() && !stripped.is_empty() {
        "0".to_string()
    } else {
        no_leading_zeros.to_string()
    }
}

fn parse_json_amount_cents(val: &Value) -> Option<i64> {
    if let Some(f) = val.as_f64() {
        Some((f * 100.0).round() as i64)
    } else if let Some(s) = val.as_str() {
        s.trim()
            .parse::<f64>()
            .ok()
            .map(|f| (f * 100.0).round() as i64)
    } else {
        None
    }
}

fn extract_bill_total_cents(inner: &Value) -> Option<i64> {
    for key in ["total", "paidAmount", "subtotal"] {
        if let Some(val) = inner.get(key) {
            if let Some(cents) = parse_json_amount_cents(val) {
                return Some(cents);
            }
        }
    }
    if let Some(items) = inner.get("items").and_then(Value::as_array) {
        if !items.is_empty() {
            let mut sum_cents = 0i64;
            for item in items {
                if let Some(c) = item.get("amount").and_then(parse_json_amount_cents) {
                    sum_cents += c;
                }
            }
            return Some(sum_cents);
        }
    }
    None
}

fn items_represent_same_payment(cand_items: &[Value], exist_items: &[Value]) -> bool {
    if cand_items.len() != exist_items.len() {
        return false;
    }

    let mut matched = vec![false; exist_items.len()];
    for c_item in cand_items {
        let c_amt = c_item
            .get("amount")
            .and_then(parse_json_amount_cents)
            .unwrap_or(0);
        let c_rule = c_item
            .get("ruleId")
            .or_else(|| c_item.get("feeId"))
            .and_then(Value::as_str)
            .map(normalize_rule_id)
            .unwrap_or_default();
        let c_title = c_item
            .get("title")
            .or_else(|| c_item.get("feeTitle"))
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        let c_period = c_item
            .get("periodId")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();

        let found = exist_items.iter().enumerate().position(|(idx, e_item)| {
            if matched[idx] {
                return false;
            }
            let e_amt = e_item
                .get("amount")
                .and_then(parse_json_amount_cents)
                .unwrap_or(0);
            if c_amt != e_amt {
                return false;
            }

            let e_period = e_item
                .get("periodId")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or_default();
            if !c_period.is_empty() && !e_period.is_empty() && !c_period.eq_ignore_ascii_case(e_period) {
                return false;
            }

            let e_rule = e_item
                .get("ruleId")
                .or_else(|| e_item.get("feeId"))
                .and_then(Value::as_str)
                .map(normalize_rule_id)
                .unwrap_or_default();
            let e_title = e_item
                .get("title")
                .or_else(|| e_item.get("feeTitle"))
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or_default();

            if !c_rule.is_empty() && !e_rule.is_empty() {
                c_rule == e_rule
            } else if !c_title.is_empty() && !e_title.is_empty() {
                c_title.eq_ignore_ascii_case(e_title)
            } else {
                c_rule == e_rule && c_title.eq_ignore_ascii_case(e_title)
            }
        });

        match found {
            Some(idx) => matched[idx] = true,
            None => return false,
        }
    }

    true
}

/// Checks whether two bill JSON payloads represent the same date, same vehicle,
/// and same payment (ignoring soft-deleted, amended, or rolled-back existing bills).
pub fn is_same_bill_date_vehicle_payment(candidate: &Value, existing: &Value) -> bool {
    if let Some(status) = existing.get("status").and_then(Value::as_str) {
        let st = status.trim();
        if st.eq_ignore_ascii_case("DELETED")
            || st.eq_ignore_ascii_case("AMENDED")
            || st.eq_ignore_ascii_case("ROLLED_BACK")
            || st.eq_ignore_ascii_case("CANCELLED")
            || st.eq_ignore_ascii_case("VOID")
        {
            return false;
        }
    }

    // 1. Same Vehicle check
    let (cand_vid, cand_vnum) = extract_bill_vehicle_identity(candidate);
    let (exist_vid, exist_vnum) = extract_bill_vehicle_identity(existing);

    let same_vehicle = (!cand_vid.is_empty()
        && !exist_vid.is_empty()
        && cand_vid.eq_ignore_ascii_case(&exist_vid))
        || (!cand_vnum.is_empty() && !exist_vnum.is_empty() && cand_vnum == exist_vnum);

    if !same_vehicle {
        return false;
    }

    // 2. Same Date check
    let cand_dates = extract_bill_dates(candidate);
    let exist_dates = extract_bill_dates(existing);
    let same_date = if cand_dates.is_empty() && exist_dates.is_empty() {
        true
    } else {
        cand_dates
            .iter()
            .any(|cd| exist_dates.iter().any(|ed| cd == ed))
    };

    if !same_date {
        return false;
    }

    // 3. Same Payment check
    // 3a. Explicit transaction / payment reference ID match
    for ref_key in ["lodbodPaymentReferenceId", "esewaTransactionId"] {
        if let (Some(c_ref), Some(e_ref)) = (
            candidate.get(ref_key).and_then(Value::as_str).map(str::trim),
            existing.get(ref_key).and_then(Value::as_str).map(str::trim),
        ) {
            if !c_ref.is_empty() && !e_ref.is_empty() && c_ref == e_ref {
                return true;
            }
        }
    }

    // 3b. Total amount + items / payment details match
    let (Some(cand_cents), Some(exist_cents)) = (
        extract_bill_total_cents(candidate),
        extract_bill_total_cents(existing),
    ) else {
        return false;
    };

    if cand_cents != exist_cents {
        return false;
    }

    if let (Some(c_pm), Some(e_pm)) = (
        candidate.get("paymentMethod").and_then(Value::as_str).map(str::trim),
        existing.get("paymentMethod").and_then(Value::as_str).map(str::trim),
    ) {
        if !c_pm.is_empty() && !e_pm.is_empty() && !c_pm.eq_ignore_ascii_case(e_pm) {
            return false;
        }
    }

    let cand_items = candidate.get("items").and_then(Value::as_array);
    let exist_items = existing.get("items").and_then(Value::as_array);

    match (cand_items, exist_items) {
        (Some(ci), Some(ei)) if !ci.is_empty() || !ei.is_empty() => {
            items_represent_same_payment(ci, ei)
        }
        _ => {
            for key in ["periodStart", "periodEnd", "billingType"] {
                if let (Some(cv), Some(ev)) = (
                    candidate.get(key).and_then(Value::as_str).map(str::trim),
                    existing.get(key).and_then(Value::as_str).map(str::trim),
                ) {
                    if !cv.is_empty() && !ev.is_empty() && !cv.eq_ignore_ascii_case(ev) {
                        return false;
                    }
                }
            }
            true
        }
    }
}

/// Searches `existing_bills` for an active bill with the same date, vehicle, and payment.
pub fn find_duplicate_bill<'a, I>(
    candidate: &Value,
    existing_bills: I,
) -> Option<&'a DatabaseResource>
where
    I: IntoIterator<Item = &'a DatabaseResource>,
{
    existing_bills.into_iter().find(|bill| {
        bill.inner_content
            .as_ref()
            .is_some_and(|existing_inner| is_same_bill_date_vehicle_payment(candidate, existing_inner))
    })
}

// =========================================================================
// 4. API Endpoints
// =========================================================================

async fn resolve_single_vehicle_billing_details(
    q_id: &str,
    q_num: &str,
    q_norm: &str,
    tqms_vehicle: Option<Value>,
    s_data: &ServerData,
) -> VehicleBillingDetails {
    let q_id_i64 = q_id.parse::<i64>().ok();
    let q_id_u64 = q_id.parse::<u64>().ok();

    let (resolved_outcome, cache_update) = {
        let cache = s_data.cache.read().await;
        let vehicles_map = cache.cache.get(&ResourceName::VehicleResource);

        let mut matched_cached: Option<&DatabaseResource> = None;
        let mut cached_is_blacklisted = false;

        if let Some(m) = vehicles_map {
            for res in m.values() {
                let Some(inner) = res.inner_content.as_ref() else {
                    continue;
                };
                if vehicle_inner_matches(inner, q_id, q_id_i64, q_id_u64, q_norm, true) {
                    if matched_cached.is_none() {
                        matched_cached = Some(res);
                    }
                    if !cached_is_blacklisted && is_vehicle_blacklisted(inner) {
                        cached_is_blacklisted = true;
                    }
                    if matched_cached.is_some() && cached_is_blacklisted {
                        break;
                    }
                }
            }
        }

        let (resolved_inner, cache_update) = match (tqms_vehicle, matched_cached) {
            (Some(mut fresh_inner), Some(cached)) => {
                // Preserve local blacklist/custom metadata on the refreshed TQMS vehicle payload.
                if let Some(cached_meta) = cached
                    .inner_content
                    .as_ref()
                    .and_then(|c| c.get("metadata"))
                    .filter(|m| !m.is_null())
                {
                    if let Value::Object(ref mut map) = fresh_inner {
                        let has_fresh_meta = map.get("metadata").is_some_and(|m| !m.is_null());
                        if !has_fresh_meta {
                            map.insert("metadata".to_string(), cached_meta.clone());
                        }
                    }
                }

                let update = cached.id.and_then(|cache_id| {
                    if cached.inner_content.as_ref() == Some(&fresh_inner) {
                        None
                    } else {
                        Some((
                            cache_id,
                            DatabaseResource {
                                id: Some(cache_id),
                                resource_name: cached.resource_name,
                                inner_content: Some(fresh_inner.clone()),
                                external_content: cached.external_content.clone(),
                            },
                        ))
                    }
                });

                (Some(fresh_inner), update)
            }
            (Some(fresh_inner), None) => (Some(fresh_inner), None),
            (None, Some(cached)) => (cached.inner_content.clone(), None),
            (None, None) => (None, None),
        };

        let outcome = resolved_inner.map(|inner| {
            let actual_id = inner
                .get("VehicleId")
                .and_then(|v| {
                    v.as_str()
                        .map(String::from)
                        .or_else(|| v.as_i64().map(|n| n.to_string()))
                        .or_else(|| v.as_u64().map(|n| n.to_string()))
                })
                .unwrap_or_else(|| q_id.to_string());

            let actual_num = inner
                .get("VehicleNumber")
                .and_then(Value::as_str)
                .unwrap_or(q_num)
                .to_string();

            let history = match cache.cache.get(&ResourceName::BillResource) {
                Some(bm) => extract_payment_history_from_iter(&actual_id, &actual_num, bm.values()),
                None => Vec::new(),
            };

            (inner, actual_id, actual_num, cached_is_blacklisted, history)
        });

        (outcome, cache_update)
    };

    if let Some((cache_id, updated_res)) = cache_update {
        s_data
            .cache
            .write()
            .await
            .write(ResourceName::VehicleResource, cache_id, updated_res);
    }

    let Some((inner, actual_id, actual_num, cached_is_blacklisted, history)) = resolved_outcome
    else {
        return VehicleBillingDetails {
            vehicle_id: q_id.to_string(),
            vehicle_number: q_num.to_string(),
            vehicle_status: "NOT_FOUND".to_string(),
            owners: None,
            fees: vec![],
            payment_history: vec![],
        };
    };

    let is_deregistered = inner
        .get("Registration")
        .and_then(|r| r.get("Status"))
        .and_then(Value::as_str)
        .is_some_and(|s| s.eq_ignore_ascii_case("DEREGISTERED"));

    let is_blacklisted = is_vehicle_blacklisted(&inner) || cached_is_blacklisted;
    let owners = resolve_owners_from_inner(&inner);
    let is_eligible = !is_deregistered && !is_blacklisted && owners.is_some();
    let vehicle_status = if is_eligible {
        "ELIGIBLE".to_string()
    } else {
        "INELIGIBLE".to_string()
    };

    let fees = if is_eligible {
        let vtype = inner.get("VehicleType").cloned();
        let v_details = Some(serde_json::json!({
            "vehicleCategory": vtype.clone().unwrap_or_else(|| Value::String("Vehicle".to_string())),
            "vehicleType": vtype.unwrap_or_else(|| Value::String("Commercial".to_string()))
        }));
        generate_applicable_fees(v_details, None, &history)
    } else {
        vec![]
    };

    VehicleBillingDetails {
        vehicle_id: actual_id,
        vehicle_number: actual_num,
        vehicle_status,
        owners,
        fees,
        payment_history: history,
    }
}

/// API 1: Get vehicle billing details
/// POST /api/integrations/lodbod/v1/vehicles/billing-details
pub async fn get_vehicle_billing_details(
    req: HttpRequest,
    body: web::Json<GetVehicleBillingDetailsRequest>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    if let Err(err) = authenticate_lodbod_request(&req) {
        return HttpResponse::build(StatusCode::UNAUTHORIZED).json(err);
    }
    let req_id = extract_request_id(&req);

    if body.vehicles.is_empty() {
        return HttpResponse::BadRequest().json(ApiErrorResponse {
            success: false,
            error: ApiErrorDetails {
                code: "INVALID_REQUEST".to_string(),
                message: "No vehicles specified in request".to_string(),
                details: vec!["vehicles array must not be empty".to_string()],
            },
            request_id: req_id,
        });
    }

    if body.vehicles.iter().any(|q| q.vehicle_id.trim().is_empty() || q.vehicle_number.trim().is_empty()) {
        return HttpResponse::BadRequest().json(ApiErrorResponse {
            success: false,
            error: ApiErrorDetails {
                code: "INVALID_REQUEST".to_string(),
                message: "vehicleId and vehicleNumber are required for every vehicle query"
                    .to_string(),
                details: vec![
                    "Every vehicle entry must include non-empty vehicleId and vehicleNumber"
                        .to_string(),
                ],
            },
            request_id: req_id,
        });
    }

    let mut results: Vec<VehicleBillingDetails> = Vec::with_capacity(body.vehicles.len());

    for query in &body.vehicles {
        let q_id = query.vehicle_id.trim();
        let q_num = query.vehicle_number.trim();
        let q_norm = normalize_vehicle_number(q_num);

        // Fetch freshest vehicle data from TQMS without holding any cache lock across await.
        let tqms_vehicle = match s_data
            .tqms_client
            .search_vehicles(VehicleSearchParameters {
                page: Some(1),
                page_size: Some(20),
                vehicle_number: Some(q_num.to_string()),
                owner_name: None,
                sangh_branch: None,
                status: None,
            })
            .await
        {
            Ok(mut resp) => take_matching_tqms_vehicle(&mut resp, q_id, &q_norm),
            Err(_) => None,
        };

        results.push(
            resolve_single_vehicle_billing_details(q_id, q_num, &q_norm, tqms_vehicle, &s_data)
                .await,
        );
    }

    HttpResponse::Ok().json(ApiResponse {
        success: true,
        data: results,
        request_id: req_id,
    })
}

fn record_reconciliation_and_idempotency(
    idempotency_key: String,
    payload: SubmitVehiclePaymentRequest,
    reason_code: &str,
    message: &str,
) -> VehiclePaymentResult {
    let recon_result = VehiclePaymentResult {
        lodbod_payment_reference_id: payload.lodbod_payment_reference_id.clone(),
        esewa_transaction_id: payload.esewa_transaction_id.clone(),
        vehicle_id: payload.vehicle_id.clone(),
        status: "RECONCILIATION_REQUIRED".to_string(),
        billing_id: None,
        receipt_number: None,
        receipt_status: Some("NOT_CREATED".to_string()),
        receipt_url: None,
        processed_at: None,
        reason_code: Some(reason_code.to_string()),
        message: Some(message.to_string()),
        duplicate: false,
    };

    get_reconciliation_store()
        .write()
        .unwrap()
        .entry(payload.lodbod_payment_reference_id.clone())
        .or_default()
        .push(recon_result.clone());

    get_idempotency_store()
        .write()
        .unwrap()
        .insert(idempotency_key, (payload, recon_result.clone()));

    recon_result
}

/// API 2: Submit verified vehicle payment
/// POST /api/integrations/lodbod/v1/payments/vehicle-receipts
pub async fn submit_vehicle_payment(
    req: HttpRequest,
    body: web::Json<SubmitVehiclePaymentRequest>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    if let Err(err) = authenticate_lodbod_request(&req) {
        return HttpResponse::build(StatusCode::UNAUTHORIZED).json(err);
    }
    let req_id = extract_request_id(&req);

    let payload = body.into_inner();

    // Idempotency check via header or fallback key
    let idempotency_key = req
        .headers()
        .get("Idempotency-Key")
        .and_then(|h| h.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_else(|| {
            format!("{}:{}", payload.lodbod_payment_reference_id, payload.vehicle_id)
        });

    {
        let store = get_idempotency_store().read().unwrap();
        if let Some((stored_req, stored_res)) = store.get(&idempotency_key) {
            if stored_req == &payload {
                return HttpResponse::Conflict().json(ApiErrorResponse {
                    success: false,
                    error: ApiErrorDetails {
                        code: "DUPLICATE_BILL".to_string(),
                        message: "A bill for the same vehicle, date, and payment already exists"
                            .to_string(),
                        details: vec![format!(
                            "Duplicate payment submission for vehicle '{}' (existing billingId: {:?})",
                            payload.vehicle_id, stored_res.billing_id
                        )],
                    },
                    request_id: req_id,
                });
            } else {
                return HttpResponse::Conflict().json(ApiErrorResponse {
                    success: false,
                    error: ApiErrorDetails {
                        code: "IDEMPOTENCY_CONFLICT".to_string(),
                        message: "Idempotency key reused with conflicting request payload".to_string(),
                        details: vec![],
                    },
                    request_id: req_id,
                });
            }
        }
    }

    // Required field validation
    if payload.lodbod_payment_reference_id.trim().is_empty()
        || payload.esewa_transaction_id.trim().is_empty()
        || (payload.vehicle_id.trim().is_empty() && payload.vehicle_number.trim().is_empty())
        || payload.selected_fees.is_empty()
    {
        return HttpResponse::BadRequest().json(ApiErrorResponse {
            success: false,
            error: ApiErrorDetails {
                code: "INVALID_REQUEST".to_string(),
                message: "Missing required payment fields".to_string(),
                details: vec!["lodbodPaymentReferenceId, esewaTransactionId, vehicleId/vehicleNumber, and selectedFees are required".to_string()],
            },
            request_id: req_id,
        });
    }

    // Validate paidAmount is a valid positive number
    let total_amount: f64 = match payload.paid_amount.parse() {
        Ok(amt) if amt > 0.0 => amt,
        _ => {
            return HttpResponse::BadRequest().json(ApiErrorResponse {
                success: false,
                error: ApiErrorDetails {
                    code: "INVALID_AMOUNT".to_string(),
                    message: "Invalid paidAmount specified".to_string(),
                    details: vec![],
                },
                request_id: req_id,
            });
        }
    };

    let q_id = payload.vehicle_id.trim();
    let q_num = payload.vehicle_number.trim();
    let q_norm = normalize_vehicle_number(q_num);

    let receipt_number = get_next_receipt_number();
    let processed_at = Utc::now().to_rfc3339();
    let billing_date = if !payload.payment_verified_at.trim().is_empty() {
        normalize_bill_date(&payload.payment_verified_at)
    } else {
        normalize_bill_date(&processed_at)
    };

    let total_periods: usize = payload.selected_fees.iter().map(|f| f.periods.len()).sum();
    let mut bill_items = Vec::with_capacity(total_periods);
    for fee in &payload.selected_fees {
        for p in &fee.periods {
            let p_amt: f64 = p.amount.parse().unwrap_or(0.0);
            bill_items.push(serde_json::json!({
                "title": fee.fee_title,
                "amount": p_amt,
                "ruleId": fee.fee_id,
                "periodId": p.period_id,
                "billingEntity": "नारायणी यातायात व्यवसायी संघ",
                "billingEntityId": "1"
            }));
        }
    }

    let fiscal_year_id = payload
        .selected_fees
        .first()
        .map(|f| f.fiscal_year.as_str())
        .unwrap_or("2083/84");

    // Attempt to resolve vehicle and check for duplicate bill in a single scoped cache read lock
    let (resolved_vehicle_meta, duplicate_bill_id) = {
        let cache = s_data.cache.read().await;
        let resolved_meta = cache
            .cache
            .get(&ResourceName::VehicleResource)
            .and_then(|m| find_matching_vehicle(m.values(), q_id, &q_norm, false))
            .and_then(|v_res| v_res.inner_content.as_ref())
            .map(|v_inner| {
                let owner_name = v_inner
                    .get("Owners")
                    .and_then(Value::as_array)
                    .and_then(|arr| arr.first())
                    .and_then(|o| o.get("Name"))
                    .and_then(Value::as_str)
                    .unwrap_or("Vehicle Owner")
                    .to_string();
                let vehicle_type = v_inner
                    .get("VehicleType")
                    .and_then(Value::as_str)
                    .unwrap_or("Vehicle")
                    .to_string();
                (owner_name, vehicle_type)
            });

        let probe_bill = serde_json::json!({
            "billingDate": billing_date,
            "paymentVerifiedAt": payload.payment_verified_at,
            "paidAt": processed_at,
            "total": total_amount,
            "items": bill_items,
            "paymentMethod": "eSewa",
            "lodbodPaymentReferenceId": payload.lodbod_payment_reference_id,
            "esewaTransactionId": payload.esewa_transaction_id,
            "vehicle": {
                "id": payload.vehicle_id,
                "vehicleNumber": payload.vehicle_number,
            }
        });

        let dup_id = cache
            .cache
            .get(&ResourceName::BillResource)
            .and_then(|bm| find_duplicate_bill(&probe_bill, bm.values()))
            .map(|b| b.id.unwrap_or(0));

        (resolved_meta, dup_id)
    };

    if let Some(existing_id) = duplicate_bill_id {
        return HttpResponse::Conflict().json(ApiErrorResponse {
            success: false,
            error: ApiErrorDetails {
                code: "DUPLICATE_BILL".to_string(),
                message: "A bill for the same vehicle, date, and payment already exists"
                    .to_string(),
                details: vec![format!("Duplicate of existing bill ID {}", existing_id)],
            },
            request_id: req_id,
        });
    }

    // If vehicle could not be found or has invalid state, preserve verified payment for reconciliation
    let Some((owner_name, vehicle_type)) = resolved_vehicle_meta else {
        let recon_result = record_reconciliation_and_idempotency(
            idempotency_key,
            payload,
            "VEHICLE_NOT_FOUND",
            "Verified payment received; this vehicle requires reconciliation.",
        );

        return HttpResponse::Ok().json(ApiResponse {
            success: true,
            data: recon_result,
            request_id: req_id,
        });
    };

    let new_bill_content = serde_json::json!({
        "note": "Online payment via LODBOD eSewa",
        "items": bill_items,
        "total": total_amount,
        "subtotal": total_amount,
        "discount": 0.0,
        "branch": {
            "id": "1",
            "name": "Main Branch",
            "phone": "057-522338",
            "address": "Hetauda-4, Parijatpath",
            "BranchCode": "MAIN"
        },
        "billingDate": billing_date,
        "paymentVerifiedAt": payload.payment_verified_at,
        "paidAt": processed_at,
        "vehicle": {
            "id": payload.vehicle_id,
            "vehicleNumber": payload.vehicle_number,
            "ownerName": owner_name,
            "vehicleType": vehicle_type
        },
        "paymentMethod": "eSewa",
        "lodbodPaymentReferenceId": payload.lodbod_payment_reference_id,
        "esewaTransactionId": payload.esewa_transaction_id,
        "receiptNumber": receipt_number,
        "fiscalYearId": fiscal_year_id,
        "status": "PAID"
    });

    let db_res = DatabaseResource {
        id: None,
        resource_name: Some(ResourceName::BillResource),
        inner_content: Some(new_bill_content),
        external_content: None,
    };

    let db_timeout = std::time::Duration::from_secs(5);
    let created_id = match tokio::time::timeout(
        db_timeout,
        s_data.db.create_resource(ResourceName::BillResource, db_res.clone()),
    )
    .await
    {
        Ok(Ok(id)) => id,
        Ok(Err(err)) => {
            eprintln!("Failed to create BillResource in database: {:?}", err);
            let recon_result = record_reconciliation_and_idempotency(
                idempotency_key,
                payload,
                "BILLING_RECORD_CREATION_FAILED",
                "Verified payment received; this vehicle requires reconciliation.",
            );

            return HttpResponse::Ok().json(ApiResponse {
                success: true,
                data: recon_result,
                request_id: req_id,
            });
        }
        Err(_) => {
            eprintln!("Database operation timed out while creating BillResource");
            let recon_result = record_reconciliation_and_idempotency(
                idempotency_key,
                payload,
                "DATABASE_TIMEOUT",
                "Verified payment received; database operation timed out, vehicle requires reconciliation.",
            );

            return HttpResponse::Ok().json(ApiResponse {
                success: true,
                data: recon_result,
                request_id: req_id,
            });
        }
    };

    // Update in-memory cache
    let mut db_res_with_id = db_res;
    db_res_with_id.id = Some(created_id);
    s_data.cache.write().await.write(ResourceName::BillResource, created_id, db_res_with_id);

    let success_result = VehiclePaymentResult {
        lodbod_payment_reference_id: payload.lodbod_payment_reference_id.clone(),
        esewa_transaction_id: payload.esewa_transaction_id.clone(),
        vehicle_id: payload.vehicle_id.clone(),
        status: "SUCCESS".to_string(),
        billing_id: Some(format!("BILL-{}", created_id)),
        receipt_number: Some(receipt_number),
        receipt_status: Some("GENERATED".to_string()),
        receipt_url: None,
        processed_at: Some(processed_at),
        reason_code: None,
        message: None,
        duplicate: false,
    };

    get_idempotency_store()
        .write()
        .unwrap()
        .insert(idempotency_key, (payload, success_result.clone()));

    HttpResponse::Created().json(ApiResponse {
        success: true,
        data: success_result,
        request_id: req_id,
    })
}

/// API 3: Get payment/receipt status
/// GET /api/integrations/lodbod/v1/payments/{lodbodPaymentReferenceId}/vehicle-receipts
pub async fn get_payment_receipt_status(
    req: HttpRequest,
    path: web::Path<String>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    if let Err(err) = authenticate_lodbod_request(&req) {
        return HttpResponse::build(StatusCode::UNAUTHORIZED).json(err);
    }
    let req_id = extract_request_id(&req);

    let lodbod_payment_reference_id = path.into_inner();

    let mut found_vehicles = Vec::new();
    let mut esewa_id = String::new();

    // 1. Check generated bills in cache without allocating an intermediate Vec
    {
        let cache = s_data.cache.read().await;
        if let Some(bills_map) = cache.cache.get(&ResourceName::BillResource) {
            for bill in bills_map.values() {
                if let Some(inner) = &bill.inner_content {
                    if let Some(ref_id) = inner.get("lodbodPaymentReferenceId").and_then(Value::as_str) {
                        if ref_id == lodbod_payment_reference_id {
                            let vehicle_id = inner
                                .get("vehicle")
                                .and_then(|v| v.get("id"))
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string();

                            if esewa_id.is_empty() {
                                if let Some(esewa) = inner.get("esewaTransactionId").and_then(Value::as_str) {
                                    esewa_id = esewa.to_string();
                                }
                            }

                            let billing_id = bill.id.map(|id| format!("BILL-{}", id));
                            let receipt_number = inner
                                .get("receiptNumber")
                                .and_then(Value::as_str)
                                .map(String::from);

                            found_vehicles.push(PaymentStatusVehicleItem {
                                vehicle_id,
                                status: "SUCCESS".to_string(),
                                billing_id,
                                receipt_number,
                                receipt_status: Some("GENERATED".to_string()),
                            });
                        }
                    }
                }
            }
        }
    }

    // 2. Check reconciliation store
    {
        let recon_store = get_reconciliation_store().read().unwrap();
        if let Some(recons) = recon_store.get(&lodbod_payment_reference_id) {
            found_vehicles.reserve(recons.len());
            for item in recons {
                if esewa_id.is_empty() {
                    esewa_id = item.esewa_transaction_id.clone();
                }
                found_vehicles.push(PaymentStatusVehicleItem {
                    vehicle_id: item.vehicle_id.clone(),
                    status: item.status.clone(),
                    billing_id: item.billing_id.clone(),
                    receipt_number: item.receipt_number.clone(),
                    receipt_status: item.receipt_status.clone(),
                });
            }
        }
    }

    if found_vehicles.is_empty() {
        return HttpResponse::NotFound().json(ApiErrorResponse {
            success: false,
            error: ApiErrorDetails {
                code: "PAYMENT_NOT_FOUND".to_string(),
                message: format!("No records found for payment reference: {}", lodbod_payment_reference_id),
                details: vec![],
            },
            request_id: req_id,
        });
    }

    let mut has_success = false;
    let mut has_recon = false;
    for v in &found_vehicles {
        if v.status == "SUCCESS" {
            has_success = true;
        } else if v.status == "RECONCILIATION_REQUIRED" {
            has_recon = true;
        }
        if has_success && has_recon {
            break;
        }
    }

    let overall_status = if has_success && has_recon {
        "PARTIALLY_RECONCILED"
    } else if has_recon {
        "RECONCILIATION_REQUIRED"
    } else {
        "SUCCESS"
    };

    HttpResponse::Ok().json(ApiResponse {
        success: true,
        data: PaymentStatusResponseData {
            lodbod_payment_reference_id,
            esewa_transaction_id: esewa_id,
            overall_status: overall_status.to_string(),
            vehicles: found_vehicles,
        },
        request_id: req_id,
    })
}

// =========================================================================
// 5. Standard Bill CRUD & Rollback Functionalities
// =========================================================================

/// Create a new bill resource
/// POST /api/v1/bills/create
pub async fn create_bill(
    db_resource: web::Json<DatabaseResource>,
    req: HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let mut resource = db_resource.into_inner();
    resource.resource_name = Some(ResourceName::BillResource);

    if let Some(ref candidate_inner) = resource.inner_content {
        let duplicate_bill_id = {
            let cache = s_data.cache.read().await;
            cache
                .cache
                .get(&ResourceName::BillResource)
                .and_then(|bills_map| find_duplicate_bill(candidate_inner, bills_map.values()))
                .map(|b| b.id.unwrap_or(0))
        };

        if let Some(existing_id) = duplicate_bill_id {
            return HttpResponse::Conflict().json(ApiErrorResponse {
                success: false,
                error: ApiErrorDetails {
                    code: "DUPLICATE_BILL".to_string(),
                    message: "A bill for the same vehicle, date, and payment already exists"
                        .to_string(),
                    details: vec![format!("Duplicate of existing bill ID {}", existing_id)],
                },
                request_id: extract_request_id(&req),
            });
        }
    }

    let id = match s_data.db.create_resource(ResourceName::BillResource, resource.clone()).await {
        Ok(id) => id,
        Err(err) => {
            return HttpResponse::InternalServerError()
                .body(format!("Failed to create bill: {:?}", err));
        }
    };

    resource.id = Some(id);
    s_data.cache.write().await.write(ResourceName::BillResource, id, resource.clone());
    println!("Created new bill with id: {}", id);
    println!("Starting audit log for bill id: {}", id);
    // Record audit log
    let performed_by = req
        .extensions()
        .get::<crate::auth::Claims>()
        .map(|c| c.sub.clone());

    crate::log::add_log(crate::log::BillLogEntry {
        id: crate::log::generate_log_id(),
        action: crate::log::ActionType::Create,
        bill_id: id,
        performed_by,
        performed_at: Utc::now().to_rfc3339(),
        previous_state: None,
        new_state: resource.inner_content.clone(),
        details: Some(format!("Created bill with id {}", id)),
        is_rolled_back: false,
        rolled_back_at: None,
        rolled_back_by: None,
    });
    println!("Audit log recorded for bill id: {}", id);
    HttpResponse::Created().json(resource)
}

/// Modify a bill resource while keeping original intact
/// PUT /api/v1/bills/modify/{id}
pub async fn modify_bill(
    path: web::Path<i32>,
    updated_resource: web::Json<DatabaseResource>,
    req: HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let old_id = path.into_inner();
    let mut new_resource = updated_resource.into_inner();
    new_resource.resource_name = Some(ResourceName::BillResource);

    // Read original bill
    let original = {
        let cache = s_data.cache.read().await;
        cache.read(ResourceName::BillResource, old_id)
    };

    let original = match original {
        Some(o) => o,
        None => match s_data.db.read_resource(ResourceName::BillResource, old_id).await {
            Ok(o) if o.id.is_some() => o,
            _ => return HttpResponse::NotFound().body("Original bill not found"),
        },
    };

    let orig_inner = original.inner_content.clone();

    // Mark original as AMENDED in DB & Cache to keep it intact as historical record
    if let Some(mut amended_inner) = original.inner_content {
        amended_inner["status"] = Value::String("AMENDED".to_string());
        amended_inner["amendedAt"] = Value::String(Utc::now().to_rfc3339());
        let updated_orig = DatabaseResource {
            id: Some(old_id),
            resource_name: Some(ResourceName::BillResource),
            inner_content: Some(amended_inner),
            external_content: original.external_content,
        };
        let _ = s_data.db.update_resource(ResourceName::BillResource, old_id, updated_orig.clone()).await;
        s_data.cache.write().await.write(ResourceName::BillResource, old_id, updated_orig);
    }

    // Link new bill to superseded bill
    if let Some(ref mut inner) = new_resource.inner_content {
        inner["supersedesBillId"] = Value::Number(old_id.into());
    }

    let new_id = match s_data.db.create_resource(ResourceName::BillResource, new_resource.clone()).await {
        Ok(id) => id,
        Err(err) => return HttpResponse::InternalServerError().body(format!("Failed to create modified bill: {:?}", err)),
    };

    new_resource.id = Some(new_id);
    s_data.cache.write().await.write(ResourceName::BillResource, new_id, new_resource.clone());

    // Record audit log
    let performed_by = req
        .extensions()
        .get::<crate::auth::Claims>()
        .map(|c| c.sub.clone());

    crate::log::add_log(crate::log::BillLogEntry {
        id: crate::log::generate_log_id(),
        action: crate::log::ActionType::Modify,
        bill_id: old_id,
        performed_by,
        performed_at: Utc::now().to_rfc3339(),
        previous_state: orig_inner,
        new_state: new_resource.inner_content.clone(),
        details: Some(format!("Modified bill {} (superseded by bill {})", old_id, new_id)),
        is_rolled_back: false,
        rolled_back_at: None,
        rolled_back_by: None,
    });

    HttpResponse::Ok().json(new_resource)
}

/// Soft delete a bill resource by updating status to DELETED without removing from DB
/// DELETE /api/v1/bills/delete/{id}
pub async fn delete_bill(
    path: web::Path<i32>,
    req: HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let id = path.into_inner();

    let existing = {
        let cache = s_data.cache.read().await;
        cache.read(ResourceName::BillResource, id)
    };

    let mut resource = match existing {
        Some(r) => r,
        None => match s_data.db.read_resource(ResourceName::BillResource, id).await {
            Ok(r) if r.id.is_some() => r,
            _ => return HttpResponse::NotFound().body("Bill not found"),
        },
    };

    let orig_inner = resource.inner_content.clone();

    if let Some(ref mut inner) = resource.inner_content {
        inner["status"] = Value::String("DELETED".to_string());
        inner["deletedAt"] = Value::String(Utc::now().to_rfc3339());
    }

    if let Err(err) = s_data.db.update_resource(ResourceName::BillResource, id, resource.clone()).await {
        return HttpResponse::InternalServerError().body(format!("Failed to soft delete bill: {:?}", err));
    }

    s_data.cache.write().await.write(ResourceName::BillResource, id, resource.clone());

    // Record audit log
    let performed_by = req
        .extensions()
        .get::<crate::auth::Claims>()
        .map(|c| c.sub.clone());

    crate::log::add_log(crate::log::BillLogEntry {
        id: crate::log::generate_log_id(),
        action: crate::log::ActionType::Delete,
        bill_id: id,
        performed_by,
        performed_at: Utc::now().to_rfc3339(),
        previous_state: orig_inner,
        new_state: resource.inner_content.clone(),
        details: Some(format!("Soft-deleted bill {}", id)),
        is_rolled_back: false,
        rolled_back_at: None,
        rolled_back_by: None,
    });

    HttpResponse::Ok().json(format!("Bill {} marked as deleted", id))
}

/// Rollback a bill action (allowed for Admin)
/// POST /api/v1/bills/rollback/{id}
pub async fn rollback_bill(
    path: web::Path<String>,
    req: HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let log_or_bill_id = path.into_inner();
    let admin_user = req
        .extensions()
        .get::<crate::auth::Claims>()
        .map(|c| c.sub.clone())
        .unwrap_or_else(|| "admin@narayanisangh.com".to_string());

    match crate::log::rollback_action(&log_or_bill_id, &admin_user, &s_data).await {
        Ok(restored_resource) => HttpResponse::Ok().json(serde_json::json!({
            "success": true,
            "message": format!("Successfully rolled back bill action for '{}'", log_or_bill_id),
            "data": restored_resource
        })),
        Err(err) => HttpResponse::BadRequest().json(serde_json::json!({
            "success": false,
            "error": err
        })),
    }
}

/// Get all bill audit logs (allowed for Admin)
/// GET /api/v1/bills/logs
pub async fn get_bill_logs(_req: HttpRequest) -> HttpResponse {
    let logs = crate::log::get_all_logs();
    HttpResponse::Ok().json(logs)
}

/// Get bill audit logs for a specific bill (allowed for Admin)
/// GET /api/v1/bills/logs/{bill_id}
pub async fn get_bill_logs_by_bill_id(
    path: web::Path<i32>,
    _req: HttpRequest,
) -> HttpResponse {
    let bill_id = path.into_inner();
    let logs = crate::log::get_logs_for_bill(bill_id);
    HttpResponse::Ok().json(logs)
}

// =========================================================================
// 6. Webhook Outbox Helper
// =========================================================================

pub async fn send_payment_update_to_lodbod(
    payload: &LodbodPaymentUpdatePayload,
    receiver_url: &str,
) -> Result<(), reqwest::Error> {
    get_http_client()
        .post(receiver_url)
        .json(payload)
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

// =========================================================================
// 7. Unit Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_vehicle_number() {
        assert_eq!(normalize_vehicle_number("BA-00-PA-0000"), "BA00PA0000");
        assert_eq!(normalize_vehicle_number("state 6 G AA 3445"), "STATE6GAA3445");
        assert_eq!(normalize_vehicle_number("Ga Pra-15-23-KHA-5555"), "GAPRA1523KHA5555");
    }

    #[test]
    fn test_resolve_owners_single_owner() {
        let inner = serde_json::json!({
            "Owners": [
                {
                    "Name": "Ram Bahadur",
                    "Phone": "9800000000"
                }
            ]
        });

        let resolution = resolve_owners_from_inner(&inner).expect("expected resolution");
        assert!(resolution.same_person);
        assert_eq!(resolution.main_owner.as_ref().unwrap().name, "Ram Bahadur");
        assert_eq!(resolution.paper_owner.as_ref().unwrap().name, "Ram Bahadur");
        assert!(resolution.main_owner.unwrap().is_main_owner);
        assert!(resolution.paper_owner.unwrap().is_paper_owner);
    }

    #[test]
    fn test_resolve_owners_different_owners() {
        let inner = serde_json::json!({
            "Owners": [
                {
                    "Name": "Ram Bahadur",
                    "Phone": "9800000000"
                },
                {
                    "Name": "Hari Bahadur",
                    "Phone": "9700000000"
                }
            ]
        });

        let resolution = resolve_owners_from_inner(&inner).expect("expected resolution");
        assert!(!resolution.same_person);
        let main = resolution.main_owner.unwrap();
        let paper = resolution.paper_owner.unwrap();

        assert_eq!(main.name, "Ram Bahadur");
        assert!(main.is_main_owner);
        assert!(!main.is_paper_owner);

        assert_eq!(paper.name, "Hari Bahadur");
        assert!(!paper.is_main_owner);
        assert!(paper.is_paper_owner);
    }

    #[test]
    fn test_resolve_owners_identical_two_owners() {
        let inner = serde_json::json!({
            "Owners": [
                {
                    "Name": "Ram Bahadur",
                    "Phone": "9800000000"
                },
                {
                    "Name": "Ram Bahadur",
                    "Phone": "9800000000"
                }
            ]
        });

        let resolution = resolve_owners_from_inner(&inner).expect("expected resolution");
        assert!(resolution.same_person);
        assert!(resolution.main_owner.unwrap().is_main_owner);
        assert!(resolution.paper_owner.unwrap().is_paper_owner);
    }

    #[test]
    fn test_resolve_owners_empty_or_missing() {
        let empty = serde_json::json!({ "Owners": [] });
        assert_eq!(resolve_owners_from_inner(&empty), None);

        let missing = serde_json::json!({});
        assert_eq!(resolve_owners_from_inner(&missing), None);
    }

    #[test]
    fn test_generate_applicable_fees_unpaid() {
        let fees = generate_applicable_fees(None, None, &[]);
        assert_eq!(fees.len(), 2);

        // Fee 1: Monthly Sadahitkosh Sulka
        assert_eq!(fees[0].fee_id, "FEE-01");
        assert_eq!(fees[0].billing_period_type, "MONTH");
        assert_eq!(fees[0].fiscal_year_eligibility, "CURRENT_YEAR_ONLY");
        assert_eq!(fees[0].fiscal_year, "2083/2084");
        assert_eq!(fees[0].periods.len(), 12);
        assert!(fees[0].periods.iter().all(|p| p.status == "UNPAID" && p.payable));

        // Fee 2: Yearly Membership
        assert_eq!(fees[1].fee_id, "FEE-02");
        assert_eq!(fees[1].fiscal_year, "2083/2084");
        assert_eq!(fees[1].billing_period_type, "YEAR");
        assert_eq!(fees[1].periods[0].status, "UNPAID");
        assert!(fees[1].periods[0].payable);
    }

    #[test]
    fn test_generate_applicable_fees_with_paid_history() {
        let paid = vec![PaymentHistoryItem {
            billing_id: "BILL-100".to_string(),
            fee_id: "FEE-01".to_string(),
            fee_title: "Sadahitkosh Sulka".to_string(),
            amount: "2000.00".to_string(),
            currency: "NPR".to_string(),
            fiscal_year: "2083/2084".to_string(),
            billing_period: "2083-06".to_string(),
            payment_status: "PAID".to_string(),
            payment_method: "CASH".to_string(),
            payment_date: "2026-09-15".to_string(),
            receipt_number: "REC-00120".to_string(),
            due_date: Some("2026-09-30".to_string()),
            lodbod_payment_reference_id: None,
        }];

        let fees = generate_applicable_fees(None, None, &paid);
        let p_2083_06 = fees[0].periods.iter().find(|p| p.period_id == "2083-06").unwrap();
        assert_eq!(p_2083_06.status, "PAID");
        assert!(!p_2083_06.payable);
        assert_eq!(p_2083_06.receipt_number, Some("REC-00120".to_string()));

        let p_2083_07 = fees[0].periods.iter().find(|p| p.period_id == "2083-07").unwrap();
        assert_eq!(p_2083_07.status, "UNPAID");
        assert!(p_2083_07.payable);
    }

    #[test]
    fn test_generate_applicable_fees_different_fiscal_years() {
        let fees_2081 = generate_applicable_fees(None, Some("2081/2082".to_string()), &[]);
        assert_eq!(fees_2081[0].fiscal_year, "2081/2082");
        assert_eq!(fees_2081[0].periods.len(), 12);
        assert_eq!(fees_2081[0].periods[0].period_id, "2081-04");
        assert_eq!(fees_2081[0].periods[0].period_label, "2081 Shrawan");
        assert_eq!(fees_2081[0].periods[0].due_date.as_deref(), Some("2024-08-15"));
        assert_eq!(fees_2081[0].periods[2].period_id, "2081-06");
        assert_eq!(fees_2081[0].periods[2].period_label, "2081 Ashwin");
        assert_eq!(fees_2081[0].periods[2].due_date.as_deref(), Some("2024-09-30"));
        assert_eq!(fees_2081[0].periods[11].period_id, "2082-03");
        assert_eq!(fees_2081[0].periods[11].period_label, "2082 Asar");
        assert_eq!(fees_2081[0].periods[11].due_date.as_deref(), Some("2025-06-30"));

        let fees_2080 = generate_applicable_fees(None, Some("2080/81".to_string()), &[]);
        assert_eq!(fees_2080[0].fiscal_year, "2080/81");
        assert_eq!(fees_2080[0].periods[0].period_id, "2080-04");
        assert_eq!(fees_2080[0].periods[7].period_id, "2080-11");
        // 2024 is leap year: Falgun AD month is Feb 2024 -> 29 days
        assert_eq!(fees_2080[0].periods[7].due_date.as_deref(), Some("2024-02-29"));
    }

    #[test]
    fn test_extract_payment_history_for_vehicle() {
        let bill1 = DatabaseResource {
            id: Some(1),
            resource_name: Some(ResourceName::BillResource),
            inner_content: Some(serde_json::json!({
                "billingDate": "2026-10-28",
                "paymentMethod": "CASH",
                "receiptNumber": "REC-0001",
                "fiscalYearId": "2083/84",
                "vehicle": {
                    "id": "101",
                    "vehicleNumber": "BA-00-PA-0000"
                },
                "items": [
                    {
                        "title": "Sadahitkosh Sulka",
                        "amount": 2000.0,
                        "ruleId": "1",
                        "periodId": "2083-07"
                    }
                ]
            })),
            external_content: None,
        };

        let bill2 = DatabaseResource {
            id: Some(2),
            resource_name: Some(ResourceName::BillResource),
            inner_content: Some(serde_json::json!({
                "billingDate": "2026-10-29",
                "paymentMethod": "eSewa",
                "receiptNumber": "ONL-0002",
                "fiscalYearId": "2083/84",
                "status": "DELETED", // Soft deleted bill should be ignored
                "vehicle": {
                    "id": "101",
                    "vehicleNumber": "BA-00-PA-0000"
                },
                "items": [
                    {
                        "title": "Sadahitkosh Sulka",
                        "amount": 2000.0
                    }
                ]
            })),
            external_content: None,
        };

        let history = extract_payment_history_for_vehicle("101", "BA-00-PA-0000", &[bill1, bill2]);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].receipt_number, "REC-0001");
        assert_eq!(history[0].payment_method, "CASH");
    }

    #[test]
    fn test_receipt_counter_increment() {
        let r1 = get_next_receipt_number();
        let r2 = get_next_receipt_number();
        assert!(r1.starts_with("ONL-"));
        assert!(r2.starts_with("ONL-"));
        assert_ne!(r1, r2);
    }

    #[actix_web::test]
    async fn test_authenticate_lodbod_request_success_with_bearer() {
        let req = actix_web::test::TestRequest::default()
            .insert_header(("Authorization", "Bearer lodbod-service-secret"))
            .to_http_request();
        assert!(authenticate_lodbod_request(&req).is_ok());
    }

    #[actix_web::test]
    async fn test_authenticate_lodbod_request_missing_header() {
        let req = actix_web::test::TestRequest::default().to_http_request();
        let err = authenticate_lodbod_request(&req).err().unwrap();
        assert_eq!(err.error.code, "UNAUTHORIZED");
        assert_eq!(err.error.message, "Missing Authorization header");
    }

    #[actix_web::test]
    async fn test_authenticate_lodbod_request_invalid_scheme() {
        let req = actix_web::test::TestRequest::default()
            .insert_header(("Authorization", "Basic dXNlcjpwYXNz"))
            .to_http_request();
        let err = authenticate_lodbod_request(&req).err().unwrap();
        assert_eq!(err.error.code, "UNAUTHORIZED");
        assert_eq!(err.error.message, "Bearer token required");
    }

    #[actix_web::test]
    async fn test_extract_request_id() {
        let req = actix_web::test::TestRequest::default()
            .insert_header(("X-Correlation-ID", "corr-test-12345"))
            .to_http_request();
        assert_eq!(extract_request_id(&req), "corr-test-12345");

        let req2 = actix_web::test::TestRequest::default().to_http_request();
        assert!(extract_request_id(&req2).starts_with("req-"));
    }

    #[test]
    fn test_submit_payment_request_deserialization() {
        let json_data = r#"{
            "lodbodPaymentReferenceId": "LB-PAY-1001",
            "esewaTransactionId": "ESEWA-987654",
            "paymentVerifiedBy": "LODBOD",
            "paymentVerifiedAt": "2026-10-28T11:30:00+05:45",
            "vehicleId": "VH-101",
            "vehicleNumber": "BA-00-PA-0000",
            "paidAmount": "2000.00",
            "currency": "NPR",
            "selectedFees": [
                {
                    "feeId": "FEE-01",
                    "feeTitle": "Sadahitkosh Sulka",
                    "fiscalYear": "2083/84",
                    "billingPeriodType": "MONTH",
                    "periods": [
                        {
                            "periodId": "2083-07",
                            "amount": "2000.00"
                        }
                    ]
                }
            ]
        }"#;

        let parsed: SubmitVehiclePaymentRequest = serde_json::from_str(json_data).expect("deserialization failed");
        assert_eq!(parsed.lodbod_payment_reference_id, "LB-PAY-1001");
        assert_eq!(parsed.esewa_transaction_id, "ESEWA-987654");
        assert_eq!(parsed.paid_amount, "2000.00");
        assert_eq!(parsed.selected_fees.len(), 1);
        assert_eq!(parsed.selected_fees[0].fee_id, "FEE-01");
    }

    #[test]
    fn test_get_billing_details_request_deserialization() {
        let valid_json = r#"{
            "vehicles": [
                {
                    "vehicleId": "VH-101",
                    "vehicleNumber": "BA-00-PA-0000"
                }
            ]
        }"#;

        let parsed: GetVehicleBillingDetailsRequest =
            serde_json::from_str(valid_json).expect("deserialization failed");
        assert_eq!(parsed.vehicles.len(), 1);
        assert_eq!(parsed.vehicles[0].vehicle_id, "VH-101");
        assert_eq!(parsed.vehicles[0].vehicle_number, "BA-00-PA-0000");

        let missing_id_json = r#"{
            "vehicles": [
                {
                    "vehicleNumber": "BA-01-PA-9999"
                }
            ]
        }"#;
        assert!(serde_json::from_str::<GetVehicleBillingDetailsRequest>(missing_id_json).is_err());

        let missing_num_json = r#"{
            "vehicles": [
                {
                    "vehicleId": "VH-101"
                }
            ]
        }"#;
        assert!(serde_json::from_str::<GetVehicleBillingDetailsRequest>(missing_num_json).is_err());
    }

    #[test]
    fn test_payment_status_response_serialization() {
        let status_data = PaymentStatusResponseData {
            lodbod_payment_reference_id: "LB-PAY-1001".to_string(),
            esewa_transaction_id: "ESEWA-987654".to_string(),
            overall_status: "PARTIALLY_RECONCILED".to_string(),
            vehicles: vec![
                PaymentStatusVehicleItem {
                    vehicle_id: "VH-101".to_string(),
                    status: "SUCCESS".to_string(),
                    billing_id: Some("BILL-501".to_string()),
                    receipt_number: Some("ONL-000123".to_string()),
                    receipt_status: Some("GENERATED".to_string()),
                },
                PaymentStatusVehicleItem {
                    vehicle_id: "VH-102".to_string(),
                    status: "RECONCILIATION_REQUIRED".to_string(),
                    billing_id: None,
                    receipt_number: None,
                    receipt_status: Some("NOT_CREATED".to_string()),
                },
            ],
        };

        let json = serde_json::to_string(&status_data).expect("serialization failed");
        assert!(json.contains("PARTIALLY_RECONCILED"));
        assert!(json.contains("ONL-000123"));
        assert!(json.contains("RECONCILIATION_REQUIRED"));
    }

    #[test]
    fn test_payment_update_payload_serialization() {
        let payload = LodbodPaymentUpdatePayload {
            event_id: "EVT-1001".to_string(),
            event_type: "BILLING_PAYMENT_RECORDED".to_string(),
            occurred_at: "2026-10-28T11:45:00+05:45".to_string(),
            organization_code: "NARAYANI".to_string(),
            vehicle_id: "VH-101".to_string(),
            vehicle_number: "BA-00-PA-0000".to_string(),
            billing_id: "BILL-501".to_string(),
            fee_id: "FEE-01".to_string(),
            fee_title: "Sadahitkosh Sulka".to_string(),
            fiscal_year: "2083/84".to_string(),
            billing_period: "2083-07".to_string(),
            amount: "2000.00".to_string(),
            currency: "NPR".to_string(),
            payment_status: "PAID".to_string(),
            payment_method: "CASH".to_string(),
            payment_date: "2026-10-28".to_string(),
            receipt_number: "REC-00123".to_string(),
            due_date: Some("2026-10-31".to_string()),
            lodbod_payment_reference_id: None,
            esewa_transaction_id: None,
        };

        let json = serde_json::to_string(&payload).expect("serialization failed");
        assert!(json.contains("EVT-1001"));
        assert!(json.contains("BILLING_PAYMENT_RECORDED"));
        assert!(json.contains("NARAYANI"));
    }

    #[actix_web::test]
    async fn test_bill_routes_middleware_roles() {
        use actix_web::{App, test, http::StatusCode, http::header::AUTHORIZATION};

        // Dummy handlers to test middleware enforcement
        async fn dummy_ok() -> HttpResponse {
            HttpResponse::Ok().body("success")
        }

        let app = test::init_service(
            App::new()
                .service(
                    web::scope("/api/v1/bills")
                        .service(
                            web::resource("/create")
                                .route(web::post().to(dummy_ok))
                                .wrap(crate::middleware::AuthMiddleware::with_roles(&[
                                    crate::roles::Roles::User,
                                    crate::roles::Roles::Admin,
                                    crate::roles::Roles::Operator,
                                ])),
                        )
                        .service(
                            web::resource("/modify/{id}")
                                .route(web::put().to(dummy_ok))
                                .wrap(crate::middleware::AuthMiddleware::with_role(crate::roles::Roles::Operator)),
                        )
                        .service(
                            web::resource("/delete/{id}")
                                .route(web::delete().to(dummy_ok))
                                .wrap(crate::middleware::AuthMiddleware::with_role(crate::roles::Roles::Admin)),
                        )
                        .service(
                            web::resource("/rollback/{id}")
                                .route(web::post().to(dummy_ok))
                                .wrap(crate::middleware::AuthMiddleware::with_role(crate::roles::Roles::Admin)),
                        )
                        .service(
                            web::resource("/logs")
                                .route(web::get().to(dummy_ok))
                                .wrap(crate::middleware::AuthMiddleware::with_role(crate::roles::Roles::Admin)),
                        ),
                )
        ).await;

        let user_token = crate::auth::create_jwt_with_role("user@test.com", &crate::roles::Roles::User).unwrap();
        let operator_token = crate::auth::create_jwt_with_role("operator@test.com", &crate::roles::Roles::Operator).unwrap();
        let admin_token = crate::auth::create_jwt_with_role("admin@test.com", &crate::roles::Roles::Admin).unwrap();

        // 1. Create is allowed for all roles (User, Operator, Admin)
        let req_user_create = test::TestRequest::post()
            .uri("/api/v1/bills/create")
            .insert_header((AUTHORIZATION, format!("Bearer {}", user_token)))
            .to_request();
        assert_eq!(test::call_service(&app, req_user_create).await.status(), StatusCode::OK);

        let req_op_create = test::TestRequest::post()
            .uri("/api/v1/bills/create")
            .insert_header((AUTHORIZATION, format!("Bearer {}", operator_token)))
            .to_request();
        assert_eq!(test::call_service(&app, req_op_create).await.status(), StatusCode::OK);

        let req_admin_create = test::TestRequest::post()
            .uri("/api/v1/bills/create")
            .insert_header((AUTHORIZATION, format!("Bearer {}", admin_token)))
            .to_request();
        assert_eq!(test::call_service(&app, req_admin_create).await.status(), StatusCode::OK);

        // 2. Modify is allowed for Operator, forbidden for User and Admin
        let req_op_modify = test::TestRequest::put()
            .uri("/api/v1/bills/modify/101")
            .insert_header((AUTHORIZATION, format!("Bearer {}", operator_token)))
            .to_request();
        assert_eq!(test::call_service(&app, req_op_modify).await.status(), StatusCode::OK);

        let req_user_modify = test::TestRequest::put()
            .uri("/api/v1/bills/modify/101")
            .insert_header((AUTHORIZATION, format!("Bearer {}", user_token)))
            .to_request();
        let err = test::try_call_service(&app, req_user_modify).await.err().expect("expected 403 forbidden");
        assert_eq!(err.as_response_error().status_code(), StatusCode::FORBIDDEN);

        let req_admin_modify = test::TestRequest::put()
            .uri("/api/v1/bills/modify/101")
            .insert_header((AUTHORIZATION, format!("Bearer {}", admin_token)))
            .to_request();
        let err = test::try_call_service(&app, req_admin_modify).await.err().expect("expected 403 forbidden");
        assert_eq!(err.as_response_error().status_code(), StatusCode::FORBIDDEN);

        // 3. Delete is allowed ONLY for Admin, forbidden for User and Operator
        let req_admin_del = test::TestRequest::delete()
            .uri("/api/v1/bills/delete/101")
            .insert_header((AUTHORIZATION, format!("Bearer {}", admin_token)))
            .to_request();
        assert_eq!(test::call_service(&app, req_admin_del).await.status(), StatusCode::OK);

        let req_user_del = test::TestRequest::delete()
            .uri("/api/v1/bills/delete/101")
            .insert_header((AUTHORIZATION, format!("Bearer {}", user_token)))
            .to_request();
        let err = test::try_call_service(&app, req_user_del).await.err().expect("expected 403 forbidden");
        assert_eq!(err.as_response_error().status_code(), StatusCode::FORBIDDEN);

        let req_op_del = test::TestRequest::delete()
            .uri("/api/v1/bills/delete/101")
            .insert_header((AUTHORIZATION, format!("Bearer {}", operator_token)))
            .to_request();
        let err = test::try_call_service(&app, req_op_del).await.err().expect("expected 403 forbidden");
        assert_eq!(err.as_response_error().status_code(), StatusCode::FORBIDDEN);

        // 4. Rollback is allowed for Admin, forbidden for User and Operator
        let req_admin_rollback = test::TestRequest::post()
            .uri("/api/v1/bills/rollback/LOG-1")
            .insert_header((AUTHORIZATION, format!("Bearer {}", admin_token)))
            .to_request();
        assert_eq!(test::call_service(&app, req_admin_rollback).await.status(), StatusCode::OK);

        let req_user_rollback = test::TestRequest::post()
            .uri("/api/v1/bills/rollback/LOG-1")
            .insert_header((AUTHORIZATION, format!("Bearer {}", user_token)))
            .to_request();
        let err = test::try_call_service(&app, req_user_rollback).await.err().expect("expected 403 forbidden");
        assert_eq!(err.as_response_error().status_code(), StatusCode::FORBIDDEN);

        let req_op_rollback = test::TestRequest::post()
            .uri("/api/v1/bills/rollback/LOG-1")
            .insert_header((AUTHORIZATION, format!("Bearer {}", operator_token)))
            .to_request();
        let err = test::try_call_service(&app, req_op_rollback).await.err().expect("expected 403 forbidden");
        assert_eq!(err.as_response_error().status_code(), StatusCode::FORBIDDEN);

        // 5. Logs are allowed for Admin, forbidden for User and Operator
        let req_admin_logs = test::TestRequest::get()
            .uri("/api/v1/bills/logs")
            .insert_header((AUTHORIZATION, format!("Bearer {}", admin_token)))
            .to_request();
        assert_eq!(test::call_service(&app, req_admin_logs).await.status(), StatusCode::OK);

        let req_user_logs = test::TestRequest::get()
            .uri("/api/v1/bills/logs")
            .insert_header((AUTHORIZATION, format!("Bearer {}", user_token)))
            .to_request();
        let err = test::try_call_service(&app, req_user_logs).await.err().expect("expected 403 forbidden");
        assert_eq!(err.as_response_error().status_code(), StatusCode::FORBIDDEN);
    }

    // =========================================================================
    // Fiscal Year Parsing Tests
    // =========================================================================

    #[test]
    fn test_parse_fiscal_year_full_format() {
        assert_eq!(parse_fiscal_year("2083/2084"), (2083, 2084));
        assert_eq!(parse_fiscal_year("2081/2082"), (2081, 2082));
        assert_eq!(parse_fiscal_year("2075/2076"), (2075, 2076));
    }

    #[test]
    fn test_parse_fiscal_year_short_format() {
        assert_eq!(parse_fiscal_year("2083/84"), (2083, 2084));
        assert_eq!(parse_fiscal_year("2080/81"), (2080, 2081));
        assert_eq!(parse_fiscal_year("2079/80"), (2079, 2080));
    }

    #[test]
    fn test_parse_fiscal_year_century_boundary() {
        assert_eq!(parse_fiscal_year("2099/00"), (2099, 2100));
        assert_eq!(parse_fiscal_year("2099/2100"), (2099, 2100));
    }

    #[test]
    fn test_parse_fiscal_year_single_year() {
        assert_eq!(parse_fiscal_year("2085"), (2085, 2086));
        assert_eq!(parse_fiscal_year("2080"), (2080, 2081));
    }

    #[test]
    fn test_parse_fiscal_year_whitespace_handling() {
        assert_eq!(parse_fiscal_year("  2082 / 2083  "), (2082, 2083));
        assert_eq!(parse_fiscal_year("\t2081 / 82\n"), (2081, 2082));
    }

    #[test]
    fn test_parse_fiscal_year_invalid_fallback() {
        assert_eq!(parse_fiscal_year(""), (2083, 2084));
        assert_eq!(parse_fiscal_year("invalid"), (2083, 2084));
        assert_eq!(parse_fiscal_year("abc/def"), (2083, 2084));
        assert_eq!(parse_fiscal_year("2083/abc"), (2083, 2084));
        assert_eq!(parse_fiscal_year("/2084"), (2083, 2084));
    }

    // =========================================================================
    // Month Periods & End Dates Tests
    // =========================================================================

    #[test]
    fn test_generate_month_periods_count_and_sequence() {
        let periods = generate_month_periods("2083/2084");
        assert_eq!(periods.len(), 12);

        let expected_ids = [
            "2083-04", "2083-05", "2083-06", "2083-07", "2083-08", "2083-09",
            "2083-10", "2083-11", "2083-12", "2084-01", "2084-02", "2084-03",
        ];
        for (i, expected_id) in expected_ids.iter().enumerate() {
            assert_eq!(periods[i].0, *expected_id);
        }
    }

    #[test]
    fn test_generate_month_periods_labels() {
        let periods = generate_month_periods("2083/2084");
        let expected_labels = [
            "2083 Shrawan", "2083 Bhadra", "2083 Ashwin", "2083 Kartik",
            "2083 Mangsir", "2083 Poush", "2083 Magh", "2083 Falgun",
            "2083 Chaitra", "2084 Baisakh", "2084 Jestha", "2084 Asar",
        ];
        for (i, expected_label) in expected_labels.iter().enumerate() {
            assert_eq!(periods[i].1, *expected_label);
        }
    }

    #[test]
    fn test_generate_month_periods_leap_year_falgun() {
        // BS 2080/81 -> AD 2023/2024 (2024 is leap year: Feb 29)
        let periods_2080 = generate_month_periods("2080/81");
        assert_eq!(periods_2080[7].0, "2080-11");
        assert_eq!(periods_2080[7].2, "2024-02-29");

        // BS 2083/84 -> AD 2026/2027 (2027 is non-leap year: Feb 28)
        let periods_2083 = generate_month_periods("2083/84");
        assert_eq!(periods_2083[7].0, "2083-11");
        assert_eq!(periods_2083[7].2, "2027-02-28");

        // BS 2084/85 -> AD 2027/2028 (2028 is leap year: Feb 29)
        let periods_2084 = generate_month_periods("2084/85");
        assert_eq!(periods_2084[7].0, "2084-11");
        assert_eq!(periods_2084[7].2, "2028-02-29");
    }

    #[test]
    fn test_generate_month_periods_due_dates_validity() {
        use crate::date::{check_ad_bs, DateParams, DateType};
        let periods = generate_month_periods("2083/2084");
        for (pid, _, due) in periods {
            let params: DateParams = due.as_str().into();
            assert!(params.month >= 1 && params.month <= 12, "Invalid month in {}", pid);
            assert!(params.day >= 1 && params.day <= 31, "Invalid day in {}", pid);
            let check = check_ad_bs(params);
            assert_eq!(check, DateType::AD, "Due date must be in AD range for {}", pid);
        }
    }

    // =========================================================================
    // Fee Generation Tests
    // =========================================================================

    #[test]
    fn test_generate_applicable_fees_with_vehicle_details() {
        let v_details = serde_json::json!({
            "vehicleCategory": "Truck",
            "vehicleType": "Heavy Cargo 16T"
        });
        let fees = generate_applicable_fees(Some(v_details.clone()), None, &[]);
        assert_eq!(fees[0].vehicle_details, Some(v_details.clone()));
        assert_eq!(fees[1].vehicle_details, Some(v_details));
    }

    #[test]
    fn test_generate_applicable_fees_all_months_paid() {
        let mut paid = Vec::new();
        let periods = generate_month_periods("2083/2084");
        for (i, (pid, _, due)) in periods.iter().enumerate() {
            paid.push(PaymentHistoryItem {
                billing_id: format!("BILL-{}", i + 1),
                fee_id: "FEE-01".to_string(),
                fee_title: "Sadahitkosh Sulka".to_string(),
                amount: "2000.00".to_string(),
                currency: "NPR".to_string(),
                fiscal_year: "2083/2084".to_string(),
                billing_period: pid.clone(),
                payment_status: "PAID".to_string(),
                payment_method: "eSewa".to_string(),
                payment_date: "2026-10-01".to_string(),
                receipt_number: format!("REC-{:04}", i + 1),
                due_date: Some(due.clone()),
                lodbod_payment_reference_id: Some("LB-PAY-001".to_string()),
            });
        }

        let fees = generate_applicable_fees(None, None, &paid);
        assert_eq!(fees[0].periods.len(), 12);
        assert!(fees[0].periods.iter().all(|p| p.status == "PAID" && !p.payable));
    }

    #[test]
    fn test_generate_applicable_fees_unmatched_fee_id_ignored() {
        let paid = vec![PaymentHistoryItem {
            billing_id: "BILL-99".to_string(),
            fee_id: "FEE-99".to_string(),
            fee_title: "Other Fee".to_string(),
            amount: "1000.00".to_string(),
            currency: "NPR".to_string(),
            fiscal_year: "2083/2084".to_string(),
            billing_period: "2083-04".to_string(),
            payment_status: "PAID".to_string(),
            payment_method: "CASH".to_string(),
            payment_date: "2026-08-10".to_string(),
            receipt_number: "REC-0099".to_string(),
            due_date: None,
            lodbod_payment_reference_id: None,
        }];

        let fees = generate_applicable_fees(None, None, &paid);
        // Fee 1 month 2083-04 should still be UNPAID
        let p_04 = fees[0].periods.iter().find(|p| p.period_id == "2083-04").unwrap();
        assert_eq!(p_04.status, "UNPAID");
        assert!(p_04.payable);
    }

    #[test]
    fn test_generate_applicable_fees_membership_paid_cross_format() {
        // Membership paid using short format "2083/84"
        let paid = vec![PaymentHistoryItem {
            billing_id: "BILL-200".to_string(),
            fee_id: "FEE-02".to_string(),
            fee_title: "सदस्यता नविकरण शूल्क".to_string(),
            amount: "500.00".to_string(),
            currency: "NPR".to_string(),
            fiscal_year: "2083/84".to_string(),
            billing_period: "2083/84".to_string(),
            payment_status: "PAID".to_string(),
            payment_method: "eSewa".to_string(),
            payment_date: "2026-08-20".to_string(),
            receipt_number: "REC-0200".to_string(),
            due_date: Some("2026-10-31".to_string()),
            lodbod_payment_reference_id: None,
        }];

        // Fees generated with default full format "2083/2084"
        let fees = generate_applicable_fees(None, None, &paid);
        assert_eq!(fees[1].periods[0].status, "PAID");
        assert!(!fees[1].periods[0].payable);
        assert_eq!(fees[1].periods[0].receipt_number, Some("REC-0200".to_string()));
    }

    #[test]
    fn test_generate_applicable_fees_membership_paid_mismatched_year() {
        let paid = vec![PaymentHistoryItem {
            billing_id: "BILL-201".to_string(),
            fee_id: "FEE-02".to_string(),
            fee_title: "सदस्यता नविकरण शूल्क".to_string(),
            amount: "500.00".to_string(),
            currency: "NPR".to_string(),
            fiscal_year: "2081/2082".to_string(),
            billing_period: "2081/2082".to_string(),
            payment_status: "PAID".to_string(),
            payment_method: "CASH".to_string(),
            payment_date: "2024-08-20".to_string(),
            receipt_number: "REC-0201".to_string(),
            due_date: None,
            lodbod_payment_reference_id: None,
        }];

        // Current year 2083/2084 should NOT match past year 2081/2082
        let fees = generate_applicable_fees(None, None, &paid);
        assert_eq!(fees[1].periods[0].status, "UNPAID");
        assert!(fees[1].periods[0].payable);
    }

    // =========================================================================
    // Vehicle Normalization Tests
    // =========================================================================

    #[test]
    fn test_normalize_vehicle_number_edge_cases() {
        assert_eq!(normalize_vehicle_number("  ba 01 pa 1234  "), "BA01PA1234");
        assert_eq!(normalize_vehicle_number("LU.2.KHA/9999_A*B"), "LU2KHA9999AB");
        assert_eq!(normalize_vehicle_number(""), "");
        assert_eq!(normalize_vehicle_number("--- ... /// *** ###"), "");
        assert_eq!(normalize_vehicle_number("ME1JA1000"), "ME1JA1000");
    }

    // =========================================================================
    // Owner Resolution Tests
    // =========================================================================

    #[test]
    fn test_resolve_owners_same_name_different_phone() {
        let inner = serde_json::json!({
            "Owners": [
                { "Name": "Ram Bahadur", "Phone": "9800000001" },
                { "Name": "Ram Bahadur", "Phone": "9800000002" }
            ]
        });
        let res = resolve_owners_from_inner(&inner).unwrap();
        assert!(!res.same_person, "Different phone numbers must mean distinct persons");
        assert!(res.main_owner.unwrap().is_main_owner);
        assert!(res.paper_owner.unwrap().is_paper_owner);
    }

    #[test]
    fn test_resolve_owners_same_name_empty_phone() {
        let inner = serde_json::json!({
            "Owners": [
                { "Name": "Ram Bahadur", "Phone": "" },
                { "Name": "Ram Bahadur", "Phone": "" }
            ]
        });
        let res = resolve_owners_from_inner(&inner).unwrap();
        assert!(!res.same_person, "Empty phone numbers should not produce identity match");
    }

    #[test]
    fn test_resolve_owners_case_insensitive_name_matching() {
        let inner = serde_json::json!({
            "Owners": [
                { "Name": "rAm bAhAdUr", "Phone": "9800000000" },
                { "Name": "RAM BAHADUR", "Phone": "9800000000" }
            ]
        });
        let res = resolve_owners_from_inner(&inner).unwrap();
        assert!(res.same_person);
    }

    #[test]
    fn test_resolve_owners_more_than_two_owners() {
        let inner = serde_json::json!({
            "Owners": [
                { "Name": "First Owner", "Phone": "9800000001" },
                { "Name": "Second Owner", "Phone": "9800000002" },
                { "Name": "Third Owner", "Phone": "9800000003" }
            ]
        });
        let res = resolve_owners_from_inner(&inner).unwrap();
        assert!(!res.same_person);
        assert_eq!(res.main_owner.unwrap().name, "First Owner");
        assert_eq!(res.paper_owner.unwrap().name, "Second Owner");
    }

    #[test]
    fn test_resolve_owners_invalid_inner_structures() {
        // Owners is a string instead of array
        let invalid1 = serde_json::json!({ "Owners": "Ram Bahadur" });
        assert_eq!(resolve_owners_from_inner(&invalid1), None);

        // Owners array contains object without Name
        let invalid2 = serde_json::json!({
            "Owners": [{ "Phone": "9800000000" }]
        });
        assert_eq!(resolve_owners_from_inner(&invalid2), None);
    }

    // =========================================================================
    // Payment History Extraction Tests
    // =========================================================================

    #[test]
    fn test_extract_payment_history_matching_variations() {
        let bill_by_id = DatabaseResource {
            id: Some(10),
            resource_name: Some(ResourceName::BillResource),
            inner_content: Some(serde_json::json!({
                "billingDate": "2026-10-25",
                "paymentMethod": "CASH",
                "receiptNumber": "REC-0010",
                "fiscalYearId": "2083/84",
                "vehicle": {
                    "id": "VH-999",
                    "vehicleNumber": "NA-01-KHA-1111"
                },
                "items": [
                    { "title": "Sadahitkosh Sulka", "amount": 2000.0, "ruleId": "1", "periodId": "2083-04" }
                ]
            })),
            external_content: None,
        };

        let bill_by_num = DatabaseResource {
            id: Some(11),
            resource_name: Some(ResourceName::BillResource),
            inner_content: Some(serde_json::json!({
                "billingDate": "2026-10-26",
                "paymentMethod": "eSewa",
                "receiptNumber": "REC-0011",
                "fiscalYearId": "2083/84",
                "vehicle": {
                    "id": "DIFFERENT-ID",
                    "vehicleNumber": "na 01 kha 1111" // normalized match
                },
                "items": [
                    { "title": "Sadahitkosh Sulka", "amount": 2000.0, "ruleId": "1", "periodId": "2083-05" }
                ]
            })),
            external_content: None,
        };

        let bill_unrelated = DatabaseResource {
            id: Some(12),
            resource_name: Some(ResourceName::BillResource),
            inner_content: Some(serde_json::json!({
                "billingDate": "2026-10-27",
                "paymentMethod": "CASH",
                "receiptNumber": "REC-0012",
                "fiscalYearId": "2083/84",
                "vehicle": {
                    "id": "OTHER-ID",
                    "vehicleNumber": "BA-99-PA-9999"
                },
                "items": [
                    { "title": "Sadahitkosh Sulka", "amount": 2000.0 }
                ]
            })),
            external_content: None,
        };

        let all_bills = vec![bill_by_id, bill_by_num, bill_unrelated];
        let history = extract_payment_history_for_vehicle("VH-999", "NA-01-KHA-1111", &all_bills);
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].receipt_number, "REC-0010");
        assert_eq!(history[1].receipt_number, "REC-0011");
    }

    #[test]
    fn test_extract_payment_history_fallback_defaults() {
        let bill_defaults = DatabaseResource {
            id: None, // Missing id -> "BILL-0"
            resource_name: Some(ResourceName::BillResource),
            inner_content: Some(serde_json::json!({
                // Missing paymentMethod, billingDate, receiptNumber, fiscalYearId
                "vehicle": {
                    "id": "VH-10",
                    "vehicleNumber": "BA-10-PA-1010"
                },
                "items": [
                    { "amount": 1500.0 } // Missing title, ruleId, periodId
                ]
            })),
            external_content: None,
        };

        let history = extract_payment_history_for_vehicle("VH-10", "BA-10-PA-1010", &[bill_defaults]);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].billing_id, "BILL-0");
        assert_eq!(history[0].payment_method, "CASH");
        assert_eq!(history[0].fee_title, "Fee");
        assert_eq!(history[0].fee_id, "FEE-01");
        assert_eq!(history[0].receipt_number, "REC-000000");
        assert_eq!(history[0].fiscal_year, "2083/84");
        assert_eq!(history[0].billing_period, "2083-06");
    }

    // =========================================================================
    // Receipt Number Generator Tests
    // =========================================================================

    #[test]
    fn test_get_next_receipt_number_multiple_distinct() {
        let receipts: Vec<String> = (0..50).map(|_| get_next_receipt_number()).collect();
        for r in &receipts {
            assert!(r.starts_with("ONL-"));
        }
        let unique: std::collections::HashSet<_> = receipts.into_iter().collect();
        assert_eq!(unique.len(), 50, "Every generated receipt number must be unique");
    }

    // =========================================================================
    // Authentication & Request ID Tests
    // =========================================================================

    #[actix_web::test]
    async fn test_authenticate_lodbod_request_empty_or_whitespace_bearer() {
        let req1 = actix_web::test::TestRequest::default()
            .insert_header(("Authorization", "Bearer "))
            .to_http_request();
        let err1 = authenticate_lodbod_request(&req1).err().unwrap();
        assert_eq!(err1.error.code, "UNAUTHORIZED");

        let req2 = actix_web::test::TestRequest::default()
            .insert_header(("Authorization", "Bearer    "))
            .to_http_request();
        let err2 = authenticate_lodbod_request(&req2).err().unwrap();
        assert_eq!(err2.error.code, "UNAUTHORIZED");
    }

    #[actix_web::test]
    async fn test_authenticate_lodbod_request_with_valid_jwt() {
        let token = crate::auth::create_jwt_with_role("service@narayani.com", &crate::roles::Roles::Admin).unwrap();
        let req = actix_web::test::TestRequest::default()
            .insert_header(("Authorization", format!("Bearer {}", token)))
            .to_http_request();
        assert!(authenticate_lodbod_request(&req).is_ok());
    }

    #[actix_web::test]
    async fn test_extract_request_id_variations() {
        let req_with_spaces = actix_web::test::TestRequest::default()
            .insert_header(("X-Correlation-ID", "  "))
            .to_http_request();
        assert!(extract_request_id(&req_with_spaces).starts_with("req-"));

        let req_valid = actix_web::test::TestRequest::default()
            .insert_header(("X-Correlation-ID", "  trace-abc-123  "))
            .to_http_request();
        assert_eq!(extract_request_id(&req_valid), "trace-abc-123");
    }

    // =========================================================================
    // Idempotency & Reconciliation Store Tests
    // =========================================================================

    #[test]
    fn test_idempotency_store_insert_and_conflict() {
        let store = get_idempotency_store();
        let key = format!("IDEMP-TEST-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));

        let req1 = SubmitVehiclePaymentRequest {
            lodbod_payment_reference_id: "LB-IDEMP-01".to_string(),
            esewa_transaction_id: "ESEWA-01".to_string(),
            payment_verified_by: "LODBOD".to_string(),
            payment_verified_at: "2026-10-30T10:00:00Z".to_string(),
            vehicle_id: "VH-IDEMP".to_string(),
            vehicle_number: "BA-01-PA-1234".to_string(),
            paid_amount: "2000.00".to_string(),
            currency: "NPR".to_string(),
            selected_fees: vec![],
        };

        let res1 = VehiclePaymentResult {
            lodbod_payment_reference_id: "LB-IDEMP-01".to_string(),
            esewa_transaction_id: "ESEWA-01".to_string(),
            vehicle_id: "VH-IDEMP".to_string(),
            status: "SUCCESS".to_string(),
            billing_id: Some("BILL-1".to_string()),
            receipt_number: Some("ONL-000001".to_string()),
            receipt_status: Some("GENERATED".to_string()),
            receipt_url: None,
            processed_at: Some("2026-10-30T10:00:01Z".to_string()),
            reason_code: None,
            message: None,
            duplicate: false,
        };

        // Insert first
        store.write().unwrap().insert(key.clone(), (req1.clone(), res1.clone()));

        // Lookup exact match
        let read_store = store.read().unwrap();
        let (stored_req, stored_res) = read_store.get(&key).unwrap();
        assert_eq!(stored_req, &req1);
        assert_eq!(stored_res.receipt_number, Some("ONL-000001".to_string()));

        // Payload mismatch check
        let mut req2 = req1.clone();
        req2.paid_amount = "3000.00".to_string();
        assert_ne!(stored_req, &req2);
    }

    #[test]
    fn test_reconciliation_store_recording() {
        let recon_store = get_reconciliation_store();
        let ref_id = format!("LB-RECON-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));

        let recon_item = VehiclePaymentResult {
            lodbod_payment_reference_id: ref_id.clone(),
            esewa_transaction_id: "ESEWA-RECON-1".to_string(),
            vehicle_id: "VH-UNKNOWN".to_string(),
            status: "RECONCILIATION_REQUIRED".to_string(),
            billing_id: None,
            receipt_number: None,
            receipt_status: Some("NOT_CREATED".to_string()),
            receipt_url: None,
            processed_at: None,
            reason_code: Some("VEHICLE_NOT_FOUND".to_string()),
            message: Some("Vehicle requires reconciliation".to_string()),
            duplicate: false,
        };

        recon_store.write().unwrap().entry(ref_id.clone()).or_default().push(recon_item.clone());

        let read_recon = recon_store.read().unwrap();
        let items = read_recon.get(&ref_id).expect("expected reconciliation items");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].status, "RECONCILIATION_REQUIRED");
        assert_eq!(items[0].reason_code, Some("VEHICLE_NOT_FOUND".to_string()));
    }

    // =========================================================================
    // Data Model Roundtrip Tests
    // =========================================================================

    #[test]
    fn test_bill_resource_defaults_and_roundtrip() {
        let bill = BillResource::default();
        assert_eq!(bill.total, 0.0);
        assert!(bill.items.is_empty());
        assert_eq!(bill.status, None);

        let json = serde_json::to_string(&bill).unwrap();
        let parsed: BillResource = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.total, 0.0);
    }

    #[test]
    fn test_branch_info_roundtrip() {
        let branch = BranchInfo {
            id: Some("1".to_string()),
            name: Some("Main Branch".to_string()),
            phone: Some("057-522338".to_string()),
            address: Some("Hetauda".to_string()),
            branch_code: Some("MAIN".to_string()),
            contact_person: Some("Manager".to_string()),
        };
        let json = serde_json::to_string(&branch).unwrap();
        assert!(json.contains("BranchCode"));
        let parsed: BranchInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.branch_code, Some("MAIN".to_string()));
    }

    #[test]
    fn test_vehicle_info_roundtrip() {
        let v_info = BillVehicleInfo {
            id: Some("VH-101".to_string()),
            phone: Some("9800000000".to_string()),
            address: Some("Kathmandu".to_string()),
            owner_name: Some("Ram".to_string()),
            vehicle_type: Some("Truck".to_string()),
            vehicle_number: Some("BA-01-PA-0001".to_string()),
        };
        let json = serde_json::to_string(&v_info).unwrap();
        assert!(json.contains("ownerName"));
        let parsed: BillVehicleInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.owner_name, Some("Ram".to_string()));
    }

    #[test]
    fn test_created_by_info_roundtrip() {
        let created_by = CreatedByInfo {
            id: Some("U-1".to_string()),
            name: Some("Operator".to_string()),
            username: Some("op1".to_string()),
        };
        let json = serde_json::to_string(&created_by).unwrap();
        let parsed: CreatedByInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.username, Some("op1".to_string()));
    }

    #[test]
    fn test_api_response_envelopes() {
        let success_resp = ApiResponse {
            success: true,
            data: "Operation completed",
            request_id: "req-12345".to_string(),
        };
        let json = serde_json::to_string(&success_resp).unwrap();
        assert!(json.contains("requestId"));
        assert!(json.contains("Operation completed"));

        let error_resp = ApiErrorResponse {
            success: false,
            error: ApiErrorDetails {
                code: "BAD_INPUT".to_string(),
                message: "Invalid parameters".to_string(),
                details: vec!["field1 is required".to_string()],
            },
            request_id: "req-54321".to_string(),
        };
        let err_json = serde_json::to_string(&error_resp).unwrap();
        assert!(err_json.contains("BAD_INPUT"));
        assert!(err_json.contains("field1 is required"));
    }

    // =========================================================================
    // Duplicate Bill Detection & Endpoint Integration Tests
    // =========================================================================

    #[test]
    fn test_normalize_bill_date_formats() {
        assert_eq!(normalize_bill_date("2026-10-30"), "2026-10-30");
        assert_eq!(normalize_bill_date("2026-10-30T11:30:00+05:45"), "2026-10-30");
        assert_eq!(normalize_bill_date("2026-10-30T00:00:00Z"), "2026-10-30");
        assert_eq!(normalize_bill_date("2026-10-30 14:22:10"), "2026-10-30");
        assert_eq!(normalize_bill_date("  2026-8-5  "), "2026-08-05");
        assert_eq!(normalize_bill_date("2083/7/3"), "2083-07-03");
        assert_eq!(normalize_bill_date(""), "");
        assert_eq!(normalize_bill_date("   "), "");
    }

    #[test]
    fn test_eq_normalized_vehicle_number_helper() {
        assert!(eq_normalized_vehicle_number("ba-01-pa-1234", "BA01PA1234"));
        assert!(eq_normalized_vehicle_number("  BA 01 PA 1234 ", "BA01PA1234"));
        assert!(!eq_normalized_vehicle_number("BA-01-PA-1235", "BA01PA1234"));
        assert!(eq_normalized_vehicle_number("", ""));
    }

    #[test]
    fn test_duplicate_bill_same_date_same_vehicle_same_payment() {
        let existing = serde_json::json!({
            "billingDate": "2026-10-30",
            "total": 2000.0,
            "paymentMethod": "CASH",
            "vehicle": {
                "id": "VH-101",
                "vehicleNumber": "BA-01-PA-1234"
            },
            "items": [
                {
                    "title": "Sadahitkosh Sulka",
                    "amount": 2000.0,
                    "ruleId": "1",
                    "periodId": "2083-07"
                }
            ],
            "status": "PAID"
        });

        // Exact same date, vehicle, and payment
        let candidate_exact = existing.clone();
        assert!(is_same_bill_date_vehicle_payment(&candidate_exact, &existing));

        // Same date with timestamp format, normalized vehicle number match, and "FEE-01" ruleId
        let candidate_formatted = serde_json::json!({
            "billingDate": "2026-10-30T15:45:00+05:45",
            "total": 2000.0,
            "paymentMethod": "cash",
            "vehicle": {
                "id": "",
                "vehicleNumber": "ba 01 pa 1234"
            },
            "items": [
                {
                    "title": "Sadahitkosh Sulka",
                    "amount": 2000.0,
                    "ruleId": "FEE-01",
                    "periodId": "2083-07"
                }
            ]
        });
        assert!(is_same_bill_date_vehicle_payment(&candidate_formatted, &existing));
    }

    #[test]
    fn test_duplicate_bill_different_date_or_vehicle_or_payment_allowed() {
        let existing = serde_json::json!({
            "billingDate": "2026-10-30",
            "total": 2000.0,
            "paymentMethod": "CASH",
            "vehicle": {
                "id": "VH-101",
                "vehicleNumber": "BA-01-PA-1234"
            },
            "items": [
                {
                    "title": "Sadahitkosh Sulka",
                    "amount": 2000.0,
                    "ruleId": "FEE-01",
                    "periodId": "2083-07"
                }
            ],
            "status": "PAID"
        });

        // 1. Different date -> NOT a duplicate
        let diff_date = serde_json::json!({
            "billingDate": "2026-10-31",
            "total": 2000.0,
            "paymentMethod": "CASH",
            "vehicle": { "id": "VH-101", "vehicleNumber": "BA-01-PA-1234" },
            "items": [{ "title": "Sadahitkosh Sulka", "amount": 2000.0, "ruleId": "FEE-01", "periodId": "2083-07" }]
        });
        assert!(!is_same_bill_date_vehicle_payment(&diff_date, &existing));

        // 2. Different vehicle -> NOT a duplicate
        let diff_vehicle = serde_json::json!({
            "billingDate": "2026-10-30",
            "total": 2000.0,
            "paymentMethod": "CASH",
            "vehicle": { "id": "VH-999", "vehicleNumber": "BA-99-PA-9999" },
            "items": [{ "title": "Sadahitkosh Sulka", "amount": 2000.0, "ruleId": "FEE-01", "periodId": "2083-07" }]
        });
        assert!(!is_same_bill_date_vehicle_payment(&diff_vehicle, &existing));

        // 3. Different payment amount -> NOT a duplicate
        let diff_amount = serde_json::json!({
            "billingDate": "2026-10-30",
            "total": 500.0,
            "paymentMethod": "CASH",
            "vehicle": { "id": "VH-101", "vehicleNumber": "BA-01-PA-1234" },
            "items": [{ "title": "सदस्यता नविकरण शूल्क", "amount": 500.0, "ruleId": "FEE-02", "periodId": "2083/2084" }]
        });
        assert!(!is_same_bill_date_vehicle_payment(&diff_amount, &existing));

        // 4. Different billing period -> NOT a duplicate
        let diff_period = serde_json::json!({
            "billingDate": "2026-10-30",
            "total": 2000.0,
            "paymentMethod": "CASH",
            "vehicle": { "id": "VH-101", "vehicleNumber": "BA-01-PA-1234" },
            "items": [{ "title": "Sadahitkosh Sulka", "amount": 2000.0, "ruleId": "FEE-01", "periodId": "2083-08" }]
        });
        assert!(!is_same_bill_date_vehicle_payment(&diff_period, &existing));
    }

    #[test]
    fn test_duplicate_bill_ignores_deleted_amended_and_rolled_back_bills() {
        for inactive_status in ["DELETED", "AMENDED", "ROLLED_BACK", "CANCELLED", "VOID"] {
            let existing = serde_json::json!({
                "billingDate": "2026-10-30",
                "total": 2000.0,
                "vehicle": { "id": "VH-101", "vehicleNumber": "BA-01-PA-1234" },
                "items": [{ "title": "Sadahitkosh Sulka", "amount": 2000.0, "ruleId": "FEE-01", "periodId": "2083-07" }],
                "status": inactive_status
            });
            let candidate = serde_json::json!({
                "billingDate": "2026-10-30",
                "total": 2000.0,
                "vehicle": { "id": "VH-101", "vehicleNumber": "BA-01-PA-1234" },
                "items": [{ "title": "Sadahitkosh Sulka", "amount": 2000.0, "ruleId": "FEE-01", "periodId": "2083-07" }]
            });
            assert!(
                !is_same_bill_date_vehicle_payment(&candidate, &existing),
                "Bill with status {} should not block creating a new bill",
                inactive_status
            );
        }
    }

    #[test]
    fn test_find_duplicate_bill_multi_item_order_independent() {
        let bill_in_cache = DatabaseResource {
            id: Some(42),
            resource_name: Some(ResourceName::BillResource),
            inner_content: Some(serde_json::json!({
                "billingDate": "2026-10-30",
                "total": 2500.0,
                "vehicle": { "id": "VH-500", "vehicleNumber": "NA-02-KHA-5000" },
                "items": [
                    { "title": "Sadahitkosh Sulka", "amount": 2000.0, "ruleId": "FEE-01", "periodId": "2083-07" },
                    { "title": "सदस्यता नविकरण शूल्क", "amount": 500.0, "ruleId": "FEE-02", "periodId": "2083/84" }
                ],
                "status": "PAID"
            })),
            external_content: None,
        };

        // Reversed item order, same date, same vehicle, same total
        let candidate_reversed = serde_json::json!({
            "billingDate": "2026-10-30",
            "total": 2500.0,
            "vehicle": { "id": "VH-500", "vehicleNumber": "NA-02-KHA-5000" },
            "items": [
                { "title": "सदस्यता नविकरण शूल्क", "amount": 500.0, "ruleId": "2", "periodId": "2083/84" },
                { "title": "Sadahitkosh Sulka", "amount": 2000.0, "ruleId": "1", "periodId": "2083-07" }
            ]
        });

        let existing_bills = [bill_in_cache];
        let found = find_duplicate_bill(&candidate_reversed, &existing_bills);
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, Some(42));
    }

    /// Creates a completely self-contained in-memory `ServerData` for endpoint tests
    /// by speaking the minimal PostgreSQL wire protocol over a `tokio::io::duplex` stream.
    async fn create_mock_server_data() -> web::Data<Arc<ServerData>> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let (client_io, mut server_io) = tokio::io::duplex(65536);

        tokio::spawn(async move {
            // 1. Read startup message: Int32 length (4 bytes) + body (len - 4 bytes)
            let Ok(startup_len) = server_io.read_i32().await else {
                return;
            };
            let mut startup_body = vec![0u8; (startup_len as usize).saturating_sub(4)];
            if server_io.read_exact(&mut startup_body).await.is_err() {
                return;
            }

            // 2. Send AuthenticationOk ('R', len=8, code=0) + ReadyForQuery ('Z', len=5, 'I')
            let handshake = [b'R', 0, 0, 0, 8, 0, 0, 0, 0, b'Z', 0, 0, 0, 5, b'I'];
            if server_io.write_all(&handshake).await.is_err() {
                return;
            }
            let _ = server_io.flush().await;

            let mut next_id: i32 = 500;
            let mut last_query_is_returning = true;
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
                        // Parse: check if SQL contains RETURNING
                        let sql_str = String::from_utf8_lossy(&body);
                        last_query_is_returning = sql_str.contains("RETURNING");
                        // ParseComplete ('1', len=4)
                        resp_buf.extend_from_slice(&[b'1', 0, 0, 0, 4]);
                    }
                    b'D' => {
                        if body.first() == Some(&b'S') {
                            if last_query_is_returning {
                                // ParameterDescription ('t', len=14, count=2, JSONB=3802, JSONB=3802)
                                resp_buf.extend_from_slice(&[
                                    b't', 0, 0, 0, 14, 0, 2, 0, 0, 0x0e, 0xda, 0, 0, 0x0e, 0xda,
                                ]);
                                // RowDescription ('T', 1 field "id\0", table=0, col=0, type=23 INT4, size=4, mod=-1, fmt=1 binary)
                                resp_buf.extend_from_slice(&[
                                    b'T', 0, 0, 0, 27, 0, 1, b'i', b'd', 0, 0, 0, 0, 0, 0, 0, 0,
                                    0, 0, 23, 0, 4, 0xff, 0xff, 0xff, 0xff, 0, 1,
                                ]);
                            } else {
                                // ParameterDescription ('t', len=18, count=3, JSONB=3802, JSONB=3802, INT4=23)
                                resp_buf.extend_from_slice(&[
                                    b't', 0, 0, 0, 18, 0, 3, 0, 0, 0x0e, 0xda, 0, 0, 0x0e, 0xda,
                                    0, 0, 0, 23,
                                ]);
                                // NoData ('n', len=4)
                                resp_buf.extend_from_slice(&[b'n', 0, 0, 0, 4]);
                            }
                        }
                    }
                    b'B' => {
                        // BindComplete ('2', len=4)
                        resp_buf.extend_from_slice(&[b'2', 0, 0, 0, 4]);
                    }
                    b'E' => {
                        if last_query_is_returning {
                            next_id += 1;
                            let id_bytes = next_id.to_be_bytes();
                            // DataRow ('D', len=14, cols=1, col_len=4, 4 bytes)
                            resp_buf.extend_from_slice(&[
                                b'D', 0, 0, 0, 14, 0, 1, 0, 0, 0, 4, id_bytes[0], id_bytes[1],
                                id_bytes[2], id_bytes[3],
                            ]);
                            // CommandComplete ('C', "INSERT 0 1\0")
                            resp_buf.extend_from_slice(b"C\x00\x00\x00\x0fINSERT 0 1\0");
                        } else {
                            // CommandComplete ('C', "UPDATE 1\0")
                            resp_buf.extend_from_slice(b"C\x00\x00\x00\x0dUPDATE 1\0");
                        }
                    }
                    b'S' => {
                        // ReadyForQuery ('Z', len=5, 'I')
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

        let db = Arc::new(crate::db::Database::new(Arc::new(client)));
        let tqms_client = Arc::new(crate::tqms_integration::TQMSClient::new().await);
        let cache = Arc::new(tokio::sync::RwLock::new(crate::cache::Cache::new()));

        web::Data::new(Arc::new(ServerData {
            db,
            tqms_client,
            cache,
        }))
    }

    #[actix_web::test]
    async fn test_create_bill_and_reject_duplicate_same_date_vehicle_payment() {
        let s_data = create_mock_server_data().await;

        let bill_payload = DatabaseResource {
            id: None,
            resource_name: Some(ResourceName::BillResource),
            inner_content: Some(serde_json::json!({
                "billingDate": "2026-10-30",
                "total": 2000.0,
                "subtotal": 2000.0,
                "paymentMethod": "CASH",
                "vehicle": {
                    "id": "VH-701",
                    "vehicleNumber": "BA-01-PA-7001",
                    "ownerName": "Sita Sharma"
                },
                "items": [
                    {
                        "title": "Sadahitkosh Sulka",
                        "amount": 2000.0,
                        "ruleId": "1",
                        "periodId": "2083-07"
                    }
                ]
            })),
            external_content: None,
        };

        // 1. First bill creation succeeds with 201 Created
        let req1 = actix_web::test::TestRequest::post().to_http_request();
        let resp1 = create_bill(web::Json(bill_payload.clone()), req1, s_data.clone()).await;
        assert_eq!(resp1.status(), StatusCode::CREATED);

        // 2. Second bill creation with the same date, same vehicle, and same payment returns 409 Conflict
        let req2 = actix_web::test::TestRequest::post()
            .insert_header(("X-Correlation-ID", "corr-dup-bill-1"))
            .to_http_request();
        let resp2 = create_bill(web::Json(bill_payload), req2, s_data.clone()).await;
        assert_eq!(resp2.status(), StatusCode::CONFLICT);

        let body_bytes = actix_web::body::to_bytes(resp2.into_body()).await.unwrap();
        let err_resp: ApiErrorResponse = serde_json::from_slice(&body_bytes).unwrap();
        assert!(!err_resp.success);
        assert_eq!(err_resp.error.code, "DUPLICATE_BILL");
        assert_eq!(err_resp.request_id, "corr-dup-bill-1");

        // 3. Creating a bill on the same date for the same vehicle with a DIFFERENT payment succeeds
        let different_payment_bill = DatabaseResource {
            id: None,
            resource_name: Some(ResourceName::BillResource),
            inner_content: Some(serde_json::json!({
                "billingDate": "2026-10-30",
                "total": 500.0,
                "subtotal": 500.0,
                "paymentMethod": "CASH",
                "vehicle": {
                    "id": "VH-701",
                    "vehicleNumber": "BA-01-PA-7001"
                },
                "items": [
                    {
                        "title": "सदस्यता नविकरण शूल्क",
                        "amount": 500.0,
                        "ruleId": "2",
                        "periodId": "2083/84"
                    }
                ]
            })),
            external_content: None,
        };
        let req3 = actix_web::test::TestRequest::post().to_http_request();
        let resp3 = create_bill(web::Json(different_payment_bill), req3, s_data.clone()).await;
        assert_eq!(resp3.status(), StatusCode::CREATED);
    }

    #[actix_web::test]
    async fn test_submit_vehicle_payment_and_reject_duplicate_same_date_vehicle_payment() {
        let s_data = create_mock_server_data().await;

        // Seed a vehicle into cache
        s_data.cache.write().await.write(
            ResourceName::VehicleResource,
            1,
            DatabaseResource {
                id: Some(1),
                resource_name: Some(ResourceName::VehicleResource),
                inner_content: Some(serde_json::json!({
                    "VehicleId": "VH-801",
                    "VehicleNumber": "BA-02-PA-8001",
                    "VehicleType": "Truck",
                    "Owners": [{ "Name": "Gopal Thapa", "Phone": "9841000000" }]
                })),
                external_content: None,
            },
        );

        let unique_ref_1 = format!(
            "LB-PAY-DUP-1-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let payment_req_1 = SubmitVehiclePaymentRequest {
            lodbod_payment_reference_id: unique_ref_1.clone(),
            esewa_transaction_id: "ESEWA-TX-8001".to_string(),
            payment_verified_by: "LODBOD".to_string(),
            payment_verified_at: "2026-10-30T10:15:00+05:45".to_string(),
            vehicle_id: "VH-801".to_string(),
            vehicle_number: "BA-02-PA-8001".to_string(),
            paid_amount: "2000.00".to_string(),
            currency: "NPR".to_string(),
            selected_fees: vec![SelectedFee {
                fee_id: "FEE-01".to_string(),
                fee_title: "Sadahitkosh Sulka".to_string(),
                fiscal_year: "2083/2084".to_string(),
                billing_period_type: "MONTH".to_string(),
                periods: vec![SelectedFeePeriod {
                    period_id: "2083-07".to_string(),
                    amount: "2000.00".to_string(),
                }],
            }],
        };

        // 1. First payment submission succeeds with 201 Created
        let http_req_1 = actix_web::test::TestRequest::post()
            .insert_header(("Authorization", "Bearer lodbod-service-secret"))
            .to_http_request();
        let resp1 = submit_vehicle_payment(http_req_1, web::Json(payment_req_1.clone()), s_data.clone()).await;
        assert_eq!(resp1.status(), StatusCode::CREATED);

        // 2. Exact repeat submission trying to create another bill returns 409 Conflict (DUPLICATE_BILL)
        let http_req_2 = actix_web::test::TestRequest::post()
            .insert_header(("Authorization", "Bearer lodbod-service-secret"))
            .to_http_request();
        let resp2 = submit_vehicle_payment(http_req_2, web::Json(payment_req_1.clone()), s_data.clone()).await;
        assert_eq!(resp2.status(), StatusCode::CONFLICT);
        let body2 = actix_web::body::to_bytes(resp2.into_body()).await.unwrap();
        let err2: ApiErrorResponse = serde_json::from_slice(&body2).unwrap();
        assert_eq!(err2.error.code, "DUPLICATE_BILL");

        // 3. New payment reference ID & new eSewa transaction ID on the SAME date, SAME vehicle, and SAME payment
        //    also returns 409 Conflict (DUPLICATE_BILL) via cache duplicate detection
        let mut payment_req_new_ref = payment_req_1.clone();
        payment_req_new_ref.lodbod_payment_reference_id = format!("{}-NEW", unique_ref_1);
        payment_req_new_ref.esewa_transaction_id = "ESEWA-TX-8002-DIFFERENT".to_string();
        payment_req_new_ref.payment_verified_at = "2026-10-30T18:45:00+05:45".to_string(); // same calendar date

        let http_req_3 = actix_web::test::TestRequest::post()
            .insert_header(("Authorization", "Bearer lodbod-service-secret"))
            .to_http_request();
        let resp3 = submit_vehicle_payment(http_req_3, web::Json(payment_req_new_ref), s_data.clone()).await;
        assert_eq!(resp3.status(), StatusCode::CONFLICT);
        let body3 = actix_web::body::to_bytes(resp3.into_body()).await.unwrap();
        let err3: ApiErrorResponse = serde_json::from_slice(&body3).unwrap();
        assert_eq!(err3.error.code, "DUPLICATE_BILL");

        // 4. Different date (next day) or different period succeeds with 201 Created
        let mut payment_req_next_month = payment_req_1;
        payment_req_next_month.lodbod_payment_reference_id = format!("{}-NEXT", unique_ref_1);
        payment_req_next_month.esewa_transaction_id = "ESEWA-TX-8003".to_string();
        payment_req_next_month.selected_fees[0].periods[0].period_id = "2083-08".to_string();

        let http_req_4 = actix_web::test::TestRequest::post()
            .insert_header(("Authorization", "Bearer lodbod-service-secret"))
            .to_http_request();
        let resp4 = submit_vehicle_payment(http_req_4, web::Json(payment_req_next_month), s_data.clone()).await;
        assert_eq!(resp4.status(), StatusCode::CREATED);
    }

    #[actix_web::test]
    async fn test_get_vehicle_billing_details_and_receipt_status_endpoints() {
        let s_data = create_mock_server_data().await;

        s_data.cache.write().await.write(
            ResourceName::VehicleResource,
            10,
            DatabaseResource {
                id: Some(10),
                resource_name: Some(ResourceName::VehicleResource),
                inner_content: Some(serde_json::json!({
                    "VehicleId": 901,
                    "VehicleNumber": "NA-01-KHA-9001",
                    "VehicleType": "Bus",
                    "Owners": [{ "Name": "Krishna Prasad", "Phone": "9855000000" }]
                })),
                external_content: None,
            },
        );

        let req = actix_web::test::TestRequest::post()
            .insert_header(("Authorization", "Bearer lodbod-service-secret"))
            .to_http_request();
        let query_body = GetVehicleBillingDetailsRequest {
            vehicles: vec![
                LodbodVehicleQuery {
                    vehicle_id: "901".to_string(),
                    vehicle_number: "NA-01-KHA-9001".to_string(),
                },
                LodbodVehicleQuery {
                    vehicle_id: "UNKNOWN-VH".to_string(),
                    vehicle_number: "ZZ-99-ZZ-9999".to_string(),
                },
            ],
        };

        let resp = get_vehicle_billing_details(req, web::Json(query_body), s_data.clone()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = actix_web::body::to_bytes(resp.into_body()).await.unwrap();
        let parsed: ApiResponse<Vec<VehicleBillingDetails>> = serde_json::from_slice(&body).unwrap();
        assert!(parsed.success);
        assert_eq!(parsed.data.len(), 2);
        assert_eq!(parsed.data[0].vehicle_status, "ELIGIBLE");
        assert_eq!(parsed.data[0].fees.len(), 2);
        assert_eq!(parsed.data[1].vehicle_status, "NOT_FOUND");
    }

    #[actix_web::test]
    async fn test_get_vehicle_billing_details_blacklisted_vehicle_is_ineligible() {
        let s_data = create_mock_server_data().await;

        s_data.cache.write().await.write(
            ResourceName::VehicleResource,
            2450,
            DatabaseResource {
                id: Some(2450),
                resource_name: Some(ResourceName::VehicleResource),
                inner_content: Some(serde_json::json!({
                    "VehicleId": 35903,
                    "VehicleNumber": "Ba Pra-53-01-0-2256",
                    "NormalizedVehicleNumber": "Ba Pra-53-01-0-2256",
                    "VehicleType": "22 dimensional",
                    "Registration": { "Status": "ACTIVE" },
                    "Owners": [{ "Name": "Addidas vandari", "Phone": "9863000032" }],
                    "metadata": {
                        "tags": ["blacklisted"],
                        "isBlacklisted": true,
                        "blacklistSource": "tqms"
                    }
                })),
                external_content: None,
            },
        );

        let req = actix_web::test::TestRequest::post()
            .insert_header(("Authorization", "Bearer lodbod-service-secret"))
            .to_http_request();
        let query_body = GetVehicleBillingDetailsRequest {
            vehicles: vec![LodbodVehicleQuery {
                vehicle_id: "35903".to_string(),
                vehicle_number: "Ba Pra-53-01-0-2256".to_string(),
            }],
        };

        let resp = get_vehicle_billing_details(req, web::Json(query_body), s_data.clone()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = actix_web::body::to_bytes(resp.into_body()).await.unwrap();
        let parsed: ApiResponse<Vec<VehicleBillingDetails>> = serde_json::from_slice(&body).unwrap();
        assert!(parsed.success);
        assert_eq!(parsed.data.len(), 1);
        assert_eq!(parsed.data[0].vehicle_status, "INELIGIBLE");
        assert!(parsed.data[0].fees.is_empty());
    }

    #[actix_web::test]
    async fn test_get_vehicle_billing_details_rejects_empty_vehicle_id_or_number() {
        let s_data = create_mock_server_data().await;

        let req = actix_web::test::TestRequest::post()
            .insert_header(("Authorization", "Bearer lodbod-service-secret"))
            .to_http_request();
        let query_body = GetVehicleBillingDetailsRequest {
            vehicles: vec![LodbodVehicleQuery {
                vehicle_id: "   ".to_string(),
                vehicle_number: "BA-01-PA-1234".to_string(),
            }],
        };

        let resp = get_vehicle_billing_details(req, web::Json(query_body), s_data).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_is_vehicle_blacklisted_and_find_matching_tqms_vehicle() {
        let blacklisted_meta = serde_json::json!({
            "VehicleId": 35903,
            "VehicleNumber": "Ba Pra-53-01-0-2256",
            "metadata": { "isBlacklisted": true }
        });
        assert!(is_vehicle_blacklisted(&blacklisted_meta));

        let blacklisted_tag = serde_json::json!({
            "VehicleId": 35904,
            "VehicleNumber": "BA-01-PA-1111",
            "metadata": { "tags": ["BLACKLISTED"] }
        });
        assert!(is_vehicle_blacklisted(&blacklisted_tag));

        let clean_vehicle = serde_json::json!({
            "VehicleId": 35905,
            "VehicleNumber": "BA-01-PA-2222",
            "Registration": { "Status": "ACTIVE" }
        });
        assert!(!is_vehicle_blacklisted(&clean_vehicle));

        let tqms_resp = serde_json::json!({
            "Code": 200,
            "Data": {
                "Data": [clean_vehicle.clone()]
            }
        });
        let found = find_matching_tqms_vehicle(&tqms_resp, "35905", "BA01PA2222");
        assert_eq!(found, Some(clean_vehicle));
    }

    #[actix_web::test]
    async fn test_get_vehicle_billing_details_validation_edge_cases() {
        let s_data = create_mock_server_data().await;

        let invalid_cases = vec![
            vec![LodbodVehicleQuery {
                vehicle_id: "".to_string(),
                vehicle_number: "BA-01-PA-1234".to_string(),
            }],
            vec![LodbodVehicleQuery {
                vehicle_id: "   \t\n ".to_string(),
                vehicle_number: "BA-01-PA-1234".to_string(),
            }],
            vec![LodbodVehicleQuery {
                vehicle_id: "35903".to_string(),
                vehicle_number: "".to_string(),
            }],
            vec![LodbodVehicleQuery {
                vehicle_id: "35903".to_string(),
                vehicle_number: "   ".to_string(),
            }],
            vec![
                LodbodVehicleQuery {
                    vehicle_id: "35903".to_string(),
                    vehicle_number: "BA-01-PA-1234".to_string(),
                },
                LodbodVehicleQuery {
                    vehicle_id: "35904".to_string(),
                    vehicle_number: "   ".to_string(),
                },
            ],
        ];

        for vehicles in invalid_cases {
            let req = actix_web::test::TestRequest::post()
                .insert_header(("Authorization", "Bearer lodbod-service-secret"))
                .to_http_request();
            let resp = get_vehicle_billing_details(
                req,
                web::Json(GetVehicleBillingDetailsRequest { vehicles }),
                s_data.clone(),
            )
            .await;
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
            let body = actix_web::body::to_bytes(resp.into_body()).await.unwrap();
            let err: ApiErrorResponse = serde_json::from_slice(&body).unwrap();
            assert!(!err.success);
            assert_eq!(err.error.code, "INVALID_REQUEST");
        }

        // Null vehicleId or vehicleNumber must fail deserialization
        let null_id_json = r#"{"vehicles":[{"vehicleId":null,"vehicleNumber":"BA-01-PA-1234"}]}"#;
        assert!(serde_json::from_str::<GetVehicleBillingDetailsRequest>(null_id_json).is_err());

        let null_num_json = r#"{"vehicles":[{"vehicleId":"101","vehicleNumber":null}]}"#;
        assert!(serde_json::from_str::<GetVehicleBillingDetailsRequest>(null_num_json).is_err());
    }

    #[test]
    fn test_is_vehicle_blacklisted_all_variants_and_edge_cases() {
        assert!(is_vehicle_blacklisted(&serde_json::json!({
            "metadata": { "isBlacklisted": true }
        })));
        assert!(!is_vehicle_blacklisted(&serde_json::json!({
            "metadata": { "isBlacklisted": false }
        })));

        assert!(is_vehicle_blacklisted(&serde_json::json!({
            "metadata": { "tags": ["blacklisted"] }
        })));
        assert!(is_vehicle_blacklisted(&serde_json::json!({
            "metadata": { "tags": ["  BLACKLISTED  "] }
        })));
        assert!(is_vehicle_blacklisted(&serde_json::json!({
            "metadata": { "tags": [123, null, "BlackListed"] }
        })));
        assert!(!is_vehicle_blacklisted(&serde_json::json!({
            "metadata": { "tags": ["vip", "priority"] }
        })));

        assert!(is_vehicle_blacklisted(&serde_json::json!({
            "isBlacklisted": true
        })));
        assert!(!is_vehicle_blacklisted(&serde_json::json!({
            "isBlacklisted": false
        })));

        assert!(is_vehicle_blacklisted(&serde_json::json!({
            "Registration": { "Status": "BLACKLISTED" }
        })));
        assert!(is_vehicle_blacklisted(&serde_json::json!({
            "Registration": { "Status": "  blacklisted  " }
        })));
        assert!(!is_vehicle_blacklisted(&serde_json::json!({
            "Registration": { "Status": "ACTIVE" }
        })));

        assert!(!is_vehicle_blacklisted(&serde_json::json!({
            "metadata": null
        })));
        assert!(!is_vehicle_blacklisted(&serde_json::json!({
            "VehicleId": 100
        })));
    }

    #[test]
    fn test_find_and_take_matching_tqms_vehicle_edge_cases() {
        let v_int_id = serde_json::json!({
            "VehicleId": 35903,
            "VehicleNumber": "Ba Pra-53-01-0-2256"
        });
        let v_str_id = serde_json::json!({
            "VehicleId": "VH-777",
            "VehicleNumber": "NA-02-KHA-7777"
        });
        let v_norm_only = serde_json::json!({
            "VehicleId": 888,
            "NormalizedVehicleNumber": "BA-09-PA-8888"
        });

        let resp = serde_json::json!({
            "Code": 200,
            "Data": {
                "Data": [v_int_id.clone(), v_str_id.clone(), v_norm_only.clone()]
            }
        });

        assert_eq!(find_matching_tqms_vehicle(&resp, "35903", ""), Some(v_int_id.clone()));
        assert_eq!(find_matching_tqms_vehicle(&resp, "VH-777", ""), Some(v_str_id.clone()));
        assert_eq!(
            find_matching_tqms_vehicle(&resp, "999999", "BAPRA530102256"),
            Some(v_int_id.clone())
        );
        assert_eq!(
            find_matching_tqms_vehicle(&resp, "999999", "BA09PA8888"),
            Some(v_norm_only.clone())
        );
        assert_eq!(find_matching_tqms_vehicle(&resp, "0", "NONEXISTENT"), None);

        let mut mut_resp = resp.clone();
        let taken = take_matching_tqms_vehicle(&mut mut_resp, "VH-777", "NA02KHA7777");
        assert_eq!(taken, Some(v_str_id));
        // Taken item is replaced by Null so a second take by the same criteria returns None
        assert_eq!(
            take_matching_tqms_vehicle(&mut mut_resp, "VH-777", "NA02KHA7777"),
            None
        );

        let mut missing_data = serde_json::json!({ "Code": 200 });
        assert_eq!(find_matching_tqms_vehicle(&missing_data, "1", "A"), None);
        assert_eq!(take_matching_tqms_vehicle(&mut missing_data, "1", "A"), None);

        let mut non_array = serde_json::json!({ "Code": 200, "Data": { "Data": "not-an-array" } });
        assert_eq!(find_matching_tqms_vehicle(&non_array, "1", "A"), None);
        assert_eq!(take_matching_tqms_vehicle(&mut non_array, "1", "A"), None);

        let mut empty_array = serde_json::json!({ "Code": 200, "Data": { "Data": [] } });
        assert_eq!(find_matching_tqms_vehicle(&empty_array, "1", "A"), None);
        assert_eq!(take_matching_tqms_vehicle(&mut empty_array, "1", "A"), None);
    }

    #[actix_web::test]
    async fn test_tqms_freshness_updates_cache_and_preserves_blacklist_metadata() {
        let s_data = create_mock_server_data().await;

        // Seed cache with stale owner/type and local blacklist metadata
        s_data.cache.write().await.write(
            ResourceName::VehicleResource,
            2450,
            DatabaseResource {
                id: Some(2450),
                resource_name: Some(ResourceName::VehicleResource),
                inner_content: Some(serde_json::json!({
                    "VehicleId": 35903,
                    "VehicleNumber": "Ba Pra-53-01-0-2256",
                    "VehicleType": "Old Type",
                    "Registration": { "Status": "ACTIVE" },
                    "Owners": [{ "Name": "Stale Owner", "Phone": "9800000001" }],
                    "metadata": {
                        "tags": ["blacklisted"],
                        "isBlacklisted": true,
                        "blacklistedReason": "Unpaid fine"
                    }
                })),
                external_content: None,
            },
        );

        // Fresh TQMS payload has updated owner/type and no metadata
        let fresh_tqms = serde_json::json!({
            "VehicleId": 35903,
            "VehicleNumber": "Ba Pra-53-01-0-2256",
            "VehicleType": "22 dimensional",
            "Registration": { "Status": "ACTIVE" },
            "Owners": [{ "Name": "Fresh TQMS Owner", "Phone": "9863000032" }]
        });

        let details = resolve_single_vehicle_billing_details(
            "35903",
            "Ba Pra-53-01-0-2256",
            "BAPRA530102256",
            Some(fresh_tqms),
            &s_data,
        )
        .await;

        // Must reflect fresh TQMS owner while staying INELIGIBLE due to preserved blacklist metadata
        assert_eq!(details.vehicle_status, "INELIGIBLE");
        assert!(details.fees.is_empty());
        assert_eq!(
            details
                .owners
                .as_ref()
                .and_then(|o| o.main_owner.as_ref())
                .map(|m| m.name.as_str()),
            Some("Fresh TQMS Owner")
        );

        // Cache must be updated with fresh TQMS fields AND preserved metadata
        let cached_after = s_data
            .cache
            .read()
            .await
            .read(ResourceName::VehicleResource, 2450)
            .expect("cached vehicle should remain present");
        let inner_after = cached_after.inner_content.unwrap();
        assert_eq!(inner_after["VehicleType"], "22 dimensional");
        assert_eq!(inner_after["Owners"][0]["Name"], "Fresh TQMS Owner");
        assert_eq!(inner_after["metadata"]["isBlacklisted"], true);
        assert_eq!(inner_after["metadata"]["blacklistedReason"], "Unpaid fine");
    }

    #[actix_web::test]
    async fn test_tqms_freshness_eligible_vehicle_and_tqms_only_vehicle() {
        let s_data = create_mock_server_data().await;

        s_data.cache.write().await.write(
            ResourceName::VehicleResource,
            50,
            DatabaseResource {
                id: Some(50),
                resource_name: Some(ResourceName::VehicleResource),
                inner_content: Some(serde_json::json!({
                    "VehicleId": 5001,
                    "VehicleNumber": "BA-01-PA-5001",
                    "VehicleType": "Old Bus",
                    "Registration": { "Status": "ACTIVE" },
                    "Owners": [{ "Name": "Old Owner", "Phone": "9800000000" }]
                })),
                external_content: None,
            },
        );

        let fresh_cached_tqms = serde_json::json!({
            "VehicleId": 5001,
            "VehicleNumber": "BA-01-PA-5001",
            "VehicleType": "Deluxe Bus",
            "Registration": { "Status": "ACTIVE" },
            "Owners": [{ "Name": "Fresh Owner", "Phone": "9811111111" }]
        });

        let details_updated = resolve_single_vehicle_billing_details(
            "5001",
            "BA-01-PA-5001",
            "BA01PA5001",
            Some(fresh_cached_tqms),
            &s_data,
        )
        .await;
        assert_eq!(details_updated.vehicle_status, "ELIGIBLE");
        assert_eq!(details_updated.fees.len(), 2);
        assert_eq!(
            details_updated
                .owners
                .as_ref()
                .and_then(|o| o.main_owner.as_ref())
                .map(|m| m.name.as_str()),
            Some("Fresh Owner")
        );
        let cached_inner = s_data
            .cache
            .read()
            .await
            .read(ResourceName::VehicleResource, 50)
            .unwrap()
            .inner_content
            .unwrap();
        assert_eq!(cached_inner["VehicleType"], "Deluxe Bus");

        // Vehicle present in TQMS only (absent from cache) -> ELIGIBLE
        let tqms_only_eligible = serde_json::json!({
            "VehicleId": 6001,
            "VehicleNumber": "BA-02-PA-6001",
            "VehicleType": "Truck",
            "Registration": { "Status": "ACTIVE" },
            "Owners": [{ "Name": "TQMS Only Owner", "Phone": "9822222222" }]
        });
        let details_tqms_only = resolve_single_vehicle_billing_details(
            "6001",
            "BA-02-PA-6001",
            "BA02PA6001",
            Some(tqms_only_eligible),
            &s_data,
        )
        .await;
        assert_eq!(details_tqms_only.vehicle_status, "ELIGIBLE");
        assert_eq!(details_tqms_only.fees.len(), 2);

        // Vehicle present in TQMS only, deregistered -> INELIGIBLE
        let tqms_only_deregistered = serde_json::json!({
            "VehicleId": 6002,
            "VehicleNumber": "BA-02-PA-6002",
            "VehicleType": "Truck",
            "Registration": { "Status": "DEREGISTERED" },
            "Owners": [{ "Name": "Deregistered Owner", "Phone": "9833333333" }]
        });
        let details_deregistered = resolve_single_vehicle_billing_details(
            "6002",
            "BA-02-PA-6002",
            "BA02PA6002",
            Some(tqms_only_deregistered),
            &s_data,
        )
        .await;
        assert_eq!(details_deregistered.vehicle_status, "INELIGIBLE");
        assert!(details_deregistered.fees.is_empty());

        // Vehicle present in TQMS only, missing owners -> INELIGIBLE
        let tqms_only_no_owners = serde_json::json!({
            "VehicleId": 6003,
            "VehicleNumber": "BA-02-PA-6003",
            "VehicleType": "Truck",
            "Registration": { "Status": "ACTIVE" },
            "Owners": []
        });
        let details_no_owners = resolve_single_vehicle_billing_details(
            "6003",
            "BA-02-PA-6003",
            "BA02PA6003",
            Some(tqms_only_no_owners),
            &s_data,
        )
        .await;
        assert_eq!(details_no_owners.vehicle_status, "INELIGIBLE");
        assert!(details_no_owners.fees.is_empty());
    }

    #[actix_web::test]
    async fn test_multiple_cached_entries_one_blacklisted_marks_ineligible() {
        let s_data = create_mock_server_data().await;

        // First cached entry appears clean
        s_data.cache.write().await.write(
            ResourceName::VehicleResource,
            701,
            DatabaseResource {
                id: Some(701),
                resource_name: Some(ResourceName::VehicleResource),
                inner_content: Some(serde_json::json!({
                    "VehicleId": 7001,
                    "VehicleNumber": "LU-01-KHA-7001",
                    "VehicleType": "Tipper",
                    "Registration": { "Status": "ACTIVE" },
                    "Owners": [{ "Name": "Clean Entry Owner", "Phone": "9844444444" }]
                })),
                external_content: None,
            },
        );

        // Second cached entry for the same vehicle carries blacklist tag
        s_data.cache.write().await.write(
            ResourceName::VehicleResource,
            702,
            DatabaseResource {
                id: Some(702),
                resource_name: Some(ResourceName::VehicleResource),
                inner_content: Some(serde_json::json!({
                    "VehicleId": 7001,
                    "VehicleNumber": "LU-01-KHA-7001",
                    "VehicleType": "Tipper",
                    "Registration": { "Status": "ACTIVE" },
                    "Owners": [{ "Name": "Blacklisted Entry Owner", "Phone": "9844444444" }],
                    "metadata": { "tags": ["blacklisted"] }
                })),
                external_content: None,
            },
        );

        let req = actix_web::test::TestRequest::post()
            .insert_header(("Authorization", "Bearer lodbod-service-secret"))
            .to_http_request();
        let query_body = GetVehicleBillingDetailsRequest {
            vehicles: vec![LodbodVehicleQuery {
                vehicle_id: "7001".to_string(),
                vehicle_number: "LU-01-KHA-7001".to_string(),
            }],
        };

        let resp = get_vehicle_billing_details(req, web::Json(query_body), s_data).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = actix_web::body::to_bytes(resp.into_body()).await.unwrap();
        let parsed: ApiResponse<Vec<VehicleBillingDetails>> = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.data.len(), 1);
        assert_eq!(parsed.data[0].vehicle_status, "INELIGIBLE");
        assert!(parsed.data[0].fees.is_empty());
    }

    #[actix_web::test]
    async fn test_delete_bill_allows_recreating_same_bill_and_rollback() {
        let s_data = create_mock_server_data().await;

        let bill_res = DatabaseResource {
            id: None,
            resource_name: Some(ResourceName::BillResource),
            inner_content: Some(serde_json::json!({
                "billingDate": "2026-10-30",
                "total": 2000.0,
                "paymentMethod": "CASH",
                "vehicle": {
                    "id": "VH-950",
                    "vehicleNumber": "BA-05-PA-9500"
                },
                "items": [
                    { "title": "Sadahitkosh Sulka", "amount": 2000.0, "ruleId": "1", "periodId": "2083-09" }
                ]
            })),
            external_content: None,
        };

        // 1. Create bill
        let req_create = actix_web::test::TestRequest::post().to_http_request();
        let resp_create = create_bill(web::Json(bill_res.clone()), req_create, s_data.clone()).await;
        assert_eq!(resp_create.status(), StatusCode::CREATED);
        let created_bytes = actix_web::body::to_bytes(resp_create.into_body()).await.unwrap();
        let created_bill: DatabaseResource = serde_json::from_slice(&created_bytes).unwrap();
        let bill_id = created_bill.id.expect("expected created bill id");

        // 2. Duplicate attempt while active fails with 409 Conflict
        let req_dup = actix_web::test::TestRequest::post().to_http_request();
        let resp_dup = create_bill(web::Json(bill_res.clone()), req_dup, s_data.clone()).await;
        assert_eq!(resp_dup.status(), StatusCode::CONFLICT);

        // 3. Soft-delete the bill
        let req_del = actix_web::test::TestRequest::delete().to_http_request();
        let resp_del = delete_bill(web::Path::from(bill_id), req_del, s_data.clone()).await;
        assert_eq!(resp_del.status(), StatusCode::OK);

        // 4. Re-creating the bill after soft-delete succeeds with 201 Created
        let req_recreate = actix_web::test::TestRequest::post().to_http_request();
        let resp_recreate = create_bill(web::Json(bill_res), req_recreate, s_data.clone()).await;
        assert_eq!(resp_recreate.status(), StatusCode::CREATED);
    }

    #[actix_web::test]
    async fn test_cross_endpoint_duplicate_between_create_bill_and_submit_vehicle_payment() {
        let s_data = create_mock_server_data().await;

        s_data.cache.write().await.write(
            ResourceName::VehicleResource,
            20,
            DatabaseResource {
                id: Some(20),
                resource_name: Some(ResourceName::VehicleResource),
                inner_content: Some(serde_json::json!({
                    "VehicleId": "VH-CROSS-1",
                    "VehicleNumber": "BA-03-PA-3333",
                    "VehicleType": "Microbus",
                    "Owners": [{ "Name": "Bishnu Maya", "Phone": "9801112222" }]
                })),
                external_content: None,
            },
        );

        // 1. Create a bill via `create_bill` for 2026-10-30, vehicle BA-03-PA-3333, eSewa 2000.00 (period 2083-07)
        let manual_bill = DatabaseResource {
            id: None,
            resource_name: Some(ResourceName::BillResource),
            inner_content: Some(serde_json::json!({
                "billingDate": "2026-10-30",
                "total": 2000.0,
                "paymentMethod": "eSewa",
                "vehicle": {
                    "id": "VH-CROSS-1",
                    "vehicleNumber": "BA-03-PA-3333"
                },
                "items": [
                    {
                        "title": "Sadahitkosh Sulka",
                        "amount": 2000.0,
                        "ruleId": "1",
                        "periodId": "2083-07"
                    }
                ]
            })),
            external_content: None,
        };
        let req_create = actix_web::test::TestRequest::post().to_http_request();
        let resp_create = create_bill(web::Json(manual_bill), req_create, s_data.clone()).await;
        assert_eq!(resp_create.status(), StatusCode::CREATED);

        // 2. Now attempt to submit a payment via `submit_vehicle_payment` for the same date, same vehicle, and same payment
        let lodbod_req = SubmitVehiclePaymentRequest {
            lodbod_payment_reference_id: format!(
                "LB-CROSS-{}",
                Utc::now().timestamp_nanos_opt().unwrap_or(0)
            ),
            esewa_transaction_id: "ESEWA-CROSS-1".to_string(),
            payment_verified_by: "LODBOD".to_string(),
            payment_verified_at: "2026-10-30T12:00:00+05:45".to_string(),
            vehicle_id: "VH-CROSS-1".to_string(),
            vehicle_number: "ba 03 pa 3333".to_string(),
            paid_amount: "2000.00".to_string(),
            currency: "NPR".to_string(),
            selected_fees: vec![SelectedFee {
                fee_id: "FEE-01".to_string(),
                fee_title: "Sadahitkosh Sulka".to_string(),
                fiscal_year: "2083/2084".to_string(),
                billing_period_type: "MONTH".to_string(),
                periods: vec![SelectedFeePeriod {
                    period_id: "2083-07".to_string(),
                    amount: "2000.00".to_string(),
                }],
            }],
        };

        let http_req = actix_web::test::TestRequest::post()
            .insert_header(("Authorization", "Bearer lodbod-service-secret"))
            .to_http_request();
        let resp_submit = submit_vehicle_payment(http_req, web::Json(lodbod_req), s_data.clone()).await;
        assert_eq!(resp_submit.status(), StatusCode::CONFLICT);
        let body = actix_web::body::to_bytes(resp_submit.into_body()).await.unwrap();
        let err: ApiErrorResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(err.error.code, "DUPLICATE_BILL");
    }

    #[actix_web::test]
    async fn test_modify_and_rollback_bill_lifecycle_with_duplicate_checks() {
        let s_data = create_mock_server_data().await;

        let orig_bill = DatabaseResource {
            id: None,
            resource_name: Some(ResourceName::BillResource),
            inner_content: Some(serde_json::json!({
                "billingDate": "2026-10-30",
                "total": 2000.0,
                "paymentMethod": "CASH",
                "vehicle": {
                    "id": "VH-MOD-1",
                    "vehicleNumber": "BA-06-PA-6000"
                },
                "items": [
                    { "title": "Sadahitkosh Sulka", "amount": 2000.0, "ruleId": "1", "periodId": "2083-10" }
                ]
            })),
            external_content: None,
        };

        // 1. Create original bill
        let req_create = actix_web::test::TestRequest::post().to_http_request();
        let resp_create = create_bill(web::Json(orig_bill.clone()), req_create, s_data.clone()).await;
        assert_eq!(resp_create.status(), StatusCode::CREATED);
        let created: DatabaseResource =
            serde_json::from_slice(&actix_web::body::to_bytes(resp_create.into_body()).await.unwrap()).unwrap();
        let orig_id = created.id.unwrap();

        // 2. Modify the bill to a new period (marks original as AMENDED)
        let modified_bill = DatabaseResource {
            id: None,
            resource_name: Some(ResourceName::BillResource),
            inner_content: Some(serde_json::json!({
                "billingDate": "2026-10-30",
                "total": 2000.0,
                "paymentMethod": "CASH",
                "vehicle": {
                    "id": "VH-MOD-1",
                    "vehicleNumber": "BA-06-PA-6000"
                },
                "items": [
                    { "title": "Sadahitkosh Sulka", "amount": 2000.0, "ruleId": "1", "periodId": "2083-11" }
                ]
            })),
            external_content: None,
        };
        let req_mod = actix_web::test::TestRequest::put().to_http_request();
        let resp_mod = modify_bill(web::Path::from(orig_id), web::Json(modified_bill.clone()), req_mod, s_data.clone()).await;
        assert_eq!(resp_mod.status(), StatusCode::OK);

        // 3. Trying to create another bill identical to the new modified bill fails with 409 Conflict
        let req_dup_mod = actix_web::test::TestRequest::post().to_http_request();
        let resp_dup_mod = create_bill(web::Json(modified_bill), req_dup_mod, s_data.clone()).await;
        assert_eq!(resp_dup_mod.status(), StatusCode::CONFLICT);

        // 4. Since original bill was marked AMENDED, re-creating a bill for the original period (2083-10) is allowed
        let req_recreate_orig = actix_web::test::TestRequest::post().to_http_request();
        let resp_recreate_orig = create_bill(web::Json(orig_bill.clone()), req_recreate_orig, s_data.clone()).await;
        assert_eq!(resp_recreate_orig.status(), StatusCode::CREATED);
        let recreated: DatabaseResource =
            serde_json::from_slice(&actix_web::body::to_bytes(resp_recreate_orig.into_body()).await.unwrap()).unwrap();
        let recreated_id = recreated.id.unwrap();

        // 5. Rollback the newly created bill (marks it ROLLED_BACK)
        let req_rb = actix_web::test::TestRequest::post().to_http_request();
        let resp_rb = rollback_bill(web::Path::from(recreated_id.to_string()), req_rb, s_data.clone()).await;
        assert_eq!(resp_rb.status(), StatusCode::OK);

        // 6. After rollback, creating that bill again succeeds
        let req_after_rb = actix_web::test::TestRequest::post().to_http_request();
        let resp_after_rb = create_bill(web::Json(orig_bill), req_after_rb, s_data.clone()).await;
        assert_eq!(resp_after_rb.status(), StatusCode::CREATED);

        // 7. Verify audit log endpoints return logs for the bill
        let req_logs = actix_web::test::TestRequest::get().to_http_request();
        let resp_logs = get_bill_logs_by_bill_id(web::Path::from(orig_id), req_logs).await;
        assert_eq!(resp_logs.status(), StatusCode::OK);
    }

    #[actix_web::test]
    async fn test_submit_vehicle_payment_validation_reconciliation_and_status_lookup() {
        let s_data = create_mock_server_data().await;

        // 1. Missing required fields -> 400 Bad Request (INVALID_REQUEST)
        let invalid_req = SubmitVehiclePaymentRequest {
            lodbod_payment_reference_id: "".to_string(),
            esewa_transaction_id: "ESEWA-1".to_string(),
            payment_verified_by: "LODBOD".to_string(),
            payment_verified_at: "2026-10-30T10:00:00Z".to_string(),
            vehicle_id: "VH-1".to_string(),
            vehicle_number: "BA-01-PA-1111".to_string(),
            paid_amount: "2000.00".to_string(),
            currency: "NPR".to_string(),
            selected_fees: vec![],
        };
        let req1 = actix_web::test::TestRequest::post()
            .insert_header(("Authorization", "Bearer lodbod-service-secret"))
            .to_http_request();
        let resp1 = submit_vehicle_payment(req1, web::Json(invalid_req), s_data.clone()).await;
        assert_eq!(resp1.status(), StatusCode::BAD_REQUEST);

        // 2. Invalid paidAmount (negative or non-numeric) -> 400 Bad Request (INVALID_AMOUNT)
        let bad_amt_req = SubmitVehiclePaymentRequest {
            lodbod_payment_reference_id: format!(
                "LB-BAD-AMT-{}",
                Utc::now().timestamp_nanos_opt().unwrap_or(0)
            ),
            esewa_transaction_id: "ESEWA-2".to_string(),
            payment_verified_by: "LODBOD".to_string(),
            payment_verified_at: "2026-10-30T10:00:00Z".to_string(),
            vehicle_id: "VH-1".to_string(),
            vehicle_number: "BA-01-PA-1111".to_string(),
            paid_amount: "-100.00".to_string(),
            currency: "NPR".to_string(),
            selected_fees: vec![SelectedFee {
                fee_id: "FEE-01".to_string(),
                fee_title: "Sadahitkosh Sulka".to_string(),
                fiscal_year: "2083/2084".to_string(),
                billing_period_type: "MONTH".to_string(),
                periods: vec![SelectedFeePeriod {
                    period_id: "2083-07".to_string(),
                    amount: "2000.00".to_string(),
                }],
            }],
        };
        let req2 = actix_web::test::TestRequest::post()
            .insert_header(("Authorization", "Bearer lodbod-service-secret"))
            .to_http_request();
        let resp2 = submit_vehicle_payment(req2, web::Json(bad_amt_req), s_data.clone()).await;
        assert_eq!(resp2.status(), StatusCode::BAD_REQUEST);

        // 3. Unknown vehicle -> 200 OK with RECONCILIATION_REQUIRED (VEHICLE_NOT_FOUND)
        let recon_ref = format!(
            "LB-RECON-ENDPOINT-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let unknown_veh_req = SubmitVehiclePaymentRequest {
            lodbod_payment_reference_id: recon_ref.clone(),
            esewa_transaction_id: "ESEWA-RECON-99".to_string(),
            payment_verified_by: "LODBOD".to_string(),
            payment_verified_at: "2026-10-30T10:00:00Z".to_string(),
            vehicle_id: "VH-MISSING-999".to_string(),
            vehicle_number: "BA-99-PA-0000".to_string(),
            paid_amount: "2000.00".to_string(),
            currency: "NPR".to_string(),
            selected_fees: vec![SelectedFee {
                fee_id: "FEE-01".to_string(),
                fee_title: "Sadahitkosh Sulka".to_string(),
                fiscal_year: "2083/2084".to_string(),
                billing_period_type: "MONTH".to_string(),
                periods: vec![SelectedFeePeriod {
                    period_id: "2083-07".to_string(),
                    amount: "2000.00".to_string(),
                }],
            }],
        };
        let req3 = actix_web::test::TestRequest::post()
            .insert_header(("Authorization", "Bearer lodbod-service-secret"))
            .to_http_request();
        let resp3 = submit_vehicle_payment(req3, web::Json(unknown_veh_req), s_data.clone()).await;
        assert_eq!(resp3.status(), StatusCode::OK);
        let body3: ApiResponse<VehiclePaymentResult> =
            serde_json::from_slice(&actix_web::body::to_bytes(resp3.into_body()).await.unwrap()).unwrap();
        assert_eq!(body3.data.status, "RECONCILIATION_REQUIRED");
        assert_eq!(body3.data.reason_code.as_deref(), Some("VEHICLE_NOT_FOUND"));

        // 4. Query status for `recon_ref` -> 200 OK with overallStatus = RECONCILIATION_REQUIRED
        let req_status = actix_web::test::TestRequest::get()
            .insert_header(("Authorization", "Bearer lodbod-service-secret"))
            .to_http_request();
        let resp_status =
            get_payment_receipt_status(req_status, web::Path::from(recon_ref), s_data.clone()).await;
        assert_eq!(resp_status.status(), StatusCode::OK);
        let status_body: ApiResponse<PaymentStatusResponseData> =
            serde_json::from_slice(&actix_web::body::to_bytes(resp_status.into_body()).await.unwrap()).unwrap();
        assert_eq!(status_body.data.overall_status, "RECONCILIATION_REQUIRED");

        // 5. Query status for non-existent reference -> 404 Not Found (PAYMENT_NOT_FOUND)
        let req_404 = actix_web::test::TestRequest::get()
            .insert_header(("Authorization", "Bearer lodbod-service-secret"))
            .to_http_request();
        let resp_404 = get_payment_receipt_status(
            req_404,
            web::Path::from("NON-EXISTENT-REF-ID".to_string()),
            s_data,
        )
        .await;
        assert_eq!(resp_404.status(), StatusCode::NOT_FOUND);
    }
}
