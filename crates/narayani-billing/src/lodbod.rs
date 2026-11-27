use narayani_core::resource::{DatabaseResource, ResourceName};
use narayani_date::{Date, DateParams};
pub use narayani_integrations::lodbod::LodbodPaymentUpdatePayload;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    #[serde(rename = "sanghBranch", alias = "SanghBranch", default, skip_serializing_if = "Option::is_none")]
    pub sangh_branch: Option<String>,
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
    #[serde(rename = "sanghBranch", alias = "SanghBranch", default, skip_serializing_if = "Option::is_none")]
    pub sangh_branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CreatedByInfo {
    pub id: Option<String>,
    pub name: Option<String>,
    pub username: Option<String>,
}

// LODBOD API Specific Models

pub const REQUIRED_STHANIYA_BRANCH: &str = "Loadboard sthaniya";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LodbodVehicleQuery {
    #[serde(rename = "vehicleId")]
    pub vehicle_id: String,
    #[serde(rename = "sthaniyaBranch")]
    pub sthaniya_branch: String,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
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
    #[serde(rename = "sthaniyaBranch", default, skip_serializing_if = "Option::is_none")]
    pub sthaniya_branch: Option<String>,
    #[serde(rename = "vehicleStatus")]
    pub vehicle_status: String,
    pub owners: Option<OwnersResolution>,
    pub fees: Vec<FeeItem>,
    #[serde(rename = "paymentHistory", default, skip_serializing_if = "Vec::is_empty")]
    pub payment_history: Vec<PaymentHistoryItem>,
    #[serde(rename = "isSampleBillingData", default, skip_serializing_if = "Option::is_none")]
    pub is_sample_billing_data: Option<bool>,
    #[serde(rename = "sampleSourceBranch", default, skip_serializing_if = "Option::is_none")]
    pub sample_source_branch: Option<String>,
    #[serde(rename = "message", default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SelectedFeePeriod {
    #[serde(rename = "periodId")]
    pub period_id: String,
    pub amount: String,
}

pub fn deserialize_opt_amount_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let val = Option::<Value>::deserialize(deserializer)?;
    match val {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
        Some(Value::Number(n)) => Ok(Some(n.to_string())),
        Some(other) => Err(serde::de::Error::custom(format!(
            "expected string or number for SelectedFee.amount, got {}",
            other
        ))),
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SelectedFee {
    #[serde(rename = "feeId")]
    pub fee_id: String,
    #[serde(rename = "feeTitle")]
    pub fee_title: String,
    #[serde(rename = "fiscalYear")]
    pub fiscal_year: String,
    #[serde(rename = "billingPeriodType", default)]
    pub billing_period_type: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_amount_string"
    )]
    pub amount: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
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
    #[serde(rename = "sanghBranch", alias = "SanghBranch", default, skip_serializing_if = "Option::is_none")]
    pub sangh_branch: Option<String>,
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


pub fn normalize_vehicle_number(v: &str) -> String {
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
pub fn eq_normalized_vehicle_number(raw: &str, normalized_upper: &str) -> bool {
    raw.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_uppercase())
        .eq(normalized_upper.chars())
}

/// Returns `true` if a vehicle's JSON inner content matches the given `VehicleId`.
pub fn vehicle_inner_matches_id(
    inner: &Value,
    q_id: &str,
    q_id_i64: Option<i64>,
    q_id_u64: Option<u64>,
) -> bool {
    if q_id.is_empty() {
        return false;
    }

    for id_key in ["VehicleId", "vehicleId"] {
        if let Some(vid) = inner.get(id_key) {
            if vid.as_str().map(str::trim) == Some(q_id)
                || (q_id_i64.is_some() && vid.as_i64() == q_id_i64)
                || (q_id_u64.is_some() && vid.as_u64() == q_id_u64)
            {
                return true;
            }
        }
    }

    false
}

/// Returns `true` if a vehicle's JSON inner content matches the given normalized `VehicleNumber`.
pub fn vehicle_inner_matches_number(
    inner: &Value,
    q_norm: &str,
    check_normalized_field: bool,
) -> bool {
    if q_norm.is_empty() {
        return false;
    }

    for num_key in ["VehicleNumber", "vehicleNumber"] {
        if let Some(vnum) = inner.get(num_key).and_then(Value::as_str) {
            if eq_normalized_vehicle_number(vnum, q_norm) {
                return true;
            }
        }
    }

    if check_normalized_field {
        for norm_key in ["NormalizedVehicleNumber", "normalizedVehicleNumber"] {
            if let Some(vnorm) = inner.get(norm_key).and_then(Value::as_str) {
                if eq_normalized_vehicle_number(vnorm, q_norm) {
                    return true;
                }
            }
        }
    }

    false
}

/// Returns `true` if a vehicle's JSON inner content matches the given `VehicleId` or normalized `VehicleNumber`.
pub fn vehicle_inner_matches(
    inner: &Value,
    q_id: &str,
    q_id_i64: Option<i64>,
    q_id_u64: Option<u64>,
    q_norm: &str,
    check_normalized_field: bool,
) -> bool {
    vehicle_inner_matches_id(inner, q_id, q_id_i64, q_id_u64)
        || vehicle_inner_matches_number(inner, q_norm, check_normalized_field)
}

pub fn extract_vehicle_id_and_number(inner: &Value) -> (Option<String>, Option<String>) {
    let vid = ["VehicleId", "vehicleId"].into_iter().find_map(|k| {
        inner.get(k).and_then(|v| {
            let s = extract_value_str_or_int(v);
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        })
    });
    let vnum = ["VehicleNumber", "vehicleNumber"].into_iter().find_map(|k| {
        inner
            .get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    });
    (vid, vnum)
}

/// Zero-allocation vehicle lookup across an iterator of `DatabaseResource`.
pub fn find_matching_vehicle<'a, I>(
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
/// When both `q_id` and `q_norm` are provided, prefers an exact match on both fields before falling back
/// to a partial match so the caller can surface a vehicle mismatch error.
pub fn take_matching_tqms_vehicle(
    tqms_response: &mut Value,
    q_id: &str,
    q_norm: &str,
) -> Option<Value> {
    let items = tqms_response
        .get_mut("Data")?
        .get_mut("Data")?
        .as_array_mut()?;
    let has_id = !q_id.is_empty();
    let has_norm = !q_norm.is_empty();
    let q_id_i64 = if has_id { q_id.parse::<i64>().ok() } else { None };
    let q_id_u64 = if has_id { q_id.parse::<u64>().ok() } else { None };

    if has_id && has_norm {
        if let Some(exact) = items.iter_mut().find(|item| {
            vehicle_inner_matches_id(item, q_id, q_id_i64, q_id_u64)
                && vehicle_inner_matches_number(item, q_norm, true)
        }) {
            return Some(Value::take(exact));
        }
    }

    items
        .iter_mut()
        .find(|item| vehicle_inner_matches(item, q_id, q_id_i64, q_id_u64, q_norm, true))
        .map(Value::take)
}

/// Extracts the matching vehicle record from a TQMS `search_vehicles` JSON response.
#[allow(dead_code)]
pub fn find_matching_tqms_vehicle(tqms_response: &Value, q_id: &str, q_norm: &str) -> Option<Value> {
    let items = tqms_response.get("Data")?.get("Data")?.as_array()?;
    let has_id = !q_id.is_empty();
    let has_norm = !q_norm.is_empty();
    let q_id_i64 = if has_id { q_id.parse::<i64>().ok() } else { None };
    let q_id_u64 = if has_id { q_id.parse::<u64>().ok() } else { None };

    if has_id && has_norm {
        if let Some(exact) = items.iter().find(|item| {
            vehicle_inner_matches_id(item, q_id, q_id_i64, q_id_u64)
                && vehicle_inner_matches_number(item, q_norm, true)
        }) {
            return Some(exact.clone());
        }
    }

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

pub fn generate_month_periods_for_years(
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

pub fn extract_payment_history_from_iter<'a, I>(
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

pub fn extract_bill_dates(inner: &Value) -> Vec<String> {
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

pub fn extract_value_str_or_int(val: &Value) -> String {
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

pub fn extract_bill_vehicle_identity(inner: &Value) -> (String, String) {
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

pub fn normalize_rule_id(rule_id: &str) -> String {
    let trimmed = rule_id.trim().to_uppercase();
    let stripped = trimmed.strip_prefix("FEE-").unwrap_or(&trimmed);
    let no_leading_zeros = stripped.trim_start_matches('0');
    if no_leading_zeros.is_empty() && !stripped.is_empty() {
        "0".to_string()
    } else {
        no_leading_zeros.to_string()
    }
}

pub fn parse_json_amount_cents(val: &Value) -> Option<i64> {
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

pub fn extract_bill_total_cents(inner: &Value) -> Option<i64> {
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

#[inline]
pub fn branch_matches(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
        || (!(a.is_ascii() && b.is_ascii()) && a.to_lowercase() == b.to_lowercase())
}

pub fn extract_trimmed_branch_str(val: Option<&Value>) -> Option<String> {
    val.and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

pub fn push_unique_branch(branches: &mut Vec<String>, candidate: &str) {
    let trimmed = candidate.trim();
    if trimmed.is_empty() {
        return;
    }
    if !branches.iter().any(|existing| branch_matches(existing, trimmed)) {
        branches.push(trimmed.to_string());
    }
}

pub fn collect_branch_values(val: Option<&Value>, out: &mut Vec<String>) {
    let Some(v) = val else {
        return;
    };
    match v {
        Value::String(s) => {
            for part in s.split(',') {
                push_unique_branch(out, part);
            }
        }
        Value::Array(arr) => {
            for entry in arr {
                if let Some(s) = entry.as_str() {
                    for part in s.split(',') {
                        push_unique_branch(out, part);
                    }
                }
            }
        }
        _ => {}
    }
}

/// Extracts `sanghBranch` / `SanghBranch` from a bill's top-level object, `vehicle`, `branch`, or `items`.
pub fn extract_bill_sangh_branch(inner: &Value) -> Option<String> {
    if let Some(b) = extract_trimmed_branch_str(
        inner
            .get("sanghBranch")
            .or_else(|| inner.get("SanghBranch")),
    ) {
        return Some(b);
    }

    for nested_key in ["vehicle", "branch"] {
        if let Some(nested) = inner.get(nested_key) {
            if let Some(b) = extract_trimmed_branch_str(
                nested
                    .get("sanghBranch")
                    .or_else(|| nested.get("SanghBranch")),
            ) {
                return Some(b);
            }
        }
    }

    if let Some(items) = inner.get("items").and_then(Value::as_array) {
        for item in items {
            if let Some(b) = extract_trimmed_branch_str(
                item.get("sanghBranch")
                    .or_else(|| item.get("SanghBranch")),
            ) {
                return Some(b);
            }
        }
    }

    None
}

/// Collects all unique non-empty `SanghBranch` / `sanghBranch` strings from top-level and `Owners`.
pub fn extract_vehicle_sangh_branches(vehicle_inner: &Value) -> Vec<String> {
    let mut branches = Vec::new();
    collect_branch_values(vehicle_inner.get("SanghBranch"), &mut branches);
    collect_branch_values(vehicle_inner.get("sanghBranch"), &mut branches);

    if let Some(owners) = vehicle_inner
        .get("Owners")
        .or_else(|| vehicle_inner.get("owners"))
        .and_then(Value::as_array)
    {
        for owner in owners {
            collect_branch_values(owner.get("SanghBranch"), &mut branches);
            collect_branch_values(owner.get("sanghBranch"), &mut branches);
        }
    }

    branches
}

/// Returns true if `expected_branch` matches at least one extracted `SanghBranch` entry
/// (including comma-separated values like `"Loadboard sthaniya, Lumbini sangh, ..."`).
pub fn vehicle_has_sangh_branch(vehicle_inner: &Value, expected_branch: &str) -> bool {
    let target = expected_branch.trim();
    if target.is_empty() {
        return false;
    }
    extract_vehicle_sangh_branches(vehicle_inner)
        .iter()
        .any(|b| branch_matches(b, target))
}

pub fn vehicle_has_explicit_sangh_branch_field(vehicle_inner: &Value) -> bool {
    if vehicle_inner.get("SanghBranch").is_some() || vehicle_inner.get("sanghBranch").is_some() {
        return true;
    }

    vehicle_inner
        .get("Owners")
        .or_else(|| vehicle_inner.get("owners"))
        .and_then(Value::as_array)
        .is_some_and(|owners| {
            owners
                .iter()
                .any(|o| o.get("SanghBranch").is_some() || o.get("sanghBranch").is_some())
        })
}

/// Validates that `requested_branch` exists on the vehicle's `SanghBranch`/`sanghBranch`.
pub fn validate_vehicle_sangh_branch(
    vehicle_inner: &Value,
    requested_branch: Option<&str>,
) -> Result<Option<String>, String> {
    let available = extract_vehicle_sangh_branches(vehicle_inner);

    if let Some(req_trimmed) = requested_branch.map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(matched) = available
            .into_iter()
            .find(|b| branch_matches(b, req_trimmed))
        {
            return Ok(Some(matched));
        }
        return Err("branch unavailable".to_string());
    }

    if let Some(first) = available.into_iter().next() {
        return Ok(Some(first));
    }

    if vehicle_has_explicit_sangh_branch_field(vehicle_inner) {
        return Err("branch unavailable".to_string());
    }

    Ok(None)
}

pub fn find_matching_vehicle_sangh_branch(
    vehicle_inner: &Value,
    requested_branch: &str,
) -> Option<String> {
    let target = requested_branch.trim();
    if target.is_empty() {
        return None;
    }
    extract_vehicle_sangh_branches(vehicle_inner)
        .into_iter()
        .find(|b| branch_matches(b, target))
}

pub fn has_billing_rule_for_sangh(
    cache: &narayani_storage::Cache,
    sangh_branch: &str,
    default_sangh: &str,
) -> bool {
    if branch_matches(sangh_branch, default_sangh) {
        return true;
    }

    let Some(rules_map) = cache.cache.get(&ResourceName::BillingRuleResource) else {
        return false;
    };

    rules_map.values().any(|rule| {
        let Some(inner) = rule.inner_content.as_ref() else {
            return false;
        };
        if let Some(status) = inner.get("status").and_then(Value::as_str) {
            let st = status.trim();
            if st.eq_ignore_ascii_case("INACTIVE")
                || st.eq_ignore_ascii_case("DELETED")
                || st.eq_ignore_ascii_case("DISABLED")
                || st.eq_ignore_ascii_case("ARCHIVED")
            {
                return false;
            }
        }
        if inner
            .get("resourceType")
            .and_then(Value::as_str)
            .is_some_and(|rt| rt.trim().eq_ignore_ascii_case("FEE_HEAD"))
        {
            return false;
        }

        let mut branches = Vec::new();
        collect_branch_values(inner.get("sthaniyaBranch"), &mut branches);
        collect_branch_values(inner.get("SanghBranch"), &mut branches);
        collect_branch_values(inner.get("sanghBranch"), &mut branches);
        branches.iter().any(|b| branch_matches(b, sangh_branch))
    })
}


pub fn extract_item_fee_head(item: &Value) -> String {
    item.get("feeHeadId")
        .or_else(|| item.get("fee_head_id"))
        .map(extract_value_str_or_int)
        .filter(|s| !s.is_empty())
        .map(|s| normalize_rule_id(&s))
        .unwrap_or_default()
}

pub fn extract_item_rule_id(item: &Value) -> String {
    item.get("ruleId")
        .or_else(|| item.get("feeId"))
        .map(extract_value_str_or_int)
        .filter(|s| !s.is_empty())
        .map(|s| normalize_rule_id(&s))
        .unwrap_or_default()
}

pub fn items_represent_same_fee_head(c_item: &Value, e_item: &Value) -> bool {
    let c_period = c_item
        .get("periodId")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    let e_period = e_item
        .get("periodId")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if !c_period.is_empty() && !e_period.is_empty() && !c_period.eq_ignore_ascii_case(e_period) {
        return false;
    }

    let c_fee_head = extract_item_fee_head(c_item);
    let e_fee_head = extract_item_fee_head(e_item);
    if !c_fee_head.is_empty() && !e_fee_head.is_empty() {
        return c_fee_head == e_fee_head;
    }

    let c_rule = extract_item_rule_id(c_item);
    let e_rule = extract_item_rule_id(e_item);
    if !c_rule.is_empty() && !e_rule.is_empty() {
        return c_rule == e_rule;
    }

    let c_title = c_item
        .get("title")
        .or_else(|| c_item.get("feeTitle"))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    let e_title = e_item
        .get("title")
        .or_else(|| e_item.get("feeTitle"))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if !c_title.is_empty() && !e_title.is_empty() {
        return branch_matches(c_title, e_title);
    }

    let c_amt = c_item
        .get("amount")
        .and_then(parse_json_amount_cents)
        .unwrap_or(0);
    let e_amt = e_item
        .get("amount")
        .and_then(parse_json_amount_cents)
        .unwrap_or(0);
    c_amt == e_amt
}

pub fn items_represent_same_payment(cand_items: &[Value], exist_items: &[Value]) -> bool {
    cand_items
        .iter()
        .any(|c_item| exist_items.iter().any(|e_item| items_represent_same_fee_head(c_item, e_item)))
}

/// Checks whether two bill JSON payloads represent the same vehicle, same time (date),
/// same `sanghBranch`, and same fee head (ignoring soft-deleted, amended, or rolled-back existing bills).
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

    // 3. Same sanghBranch check
    let cand_branch = extract_bill_sangh_branch(candidate);
    let exist_branch = extract_bill_sangh_branch(existing);
    let same_branch = match (cand_branch.as_deref(), exist_branch.as_deref()) {
        (Some(cb), Some(eb)) => branch_matches(cb, eb),
        (None, None) => true,
        _ => false,
    };
    if !same_branch {
        return false;
    }

    // 4. Same Fee Head / Payment check
    // 4a. Explicit transaction / payment reference ID match
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

    // 4b. Items fee head check when both contain non-empty items
    let cand_items = candidate.get("items").and_then(Value::as_array);
    let exist_items = existing.get("items").and_then(Value::as_array);
    if let (Some(ci), Some(ei)) = (cand_items, exist_items) {
        if !ci.is_empty() && !ei.is_empty() {
            return items_represent_same_payment(ci, ei);
        }
    }

    // 4c. Fallback total/period/billingType comparison when items are absent
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VehicleBillingDetailsError {
    VehicleMismatch { detail: String },
    BranchUnavailable,
}

pub fn build_vehicle_mismatch_detail(
    q_id: &str,
    q_num: &str,
    conflict_inner: &Value,
) -> String {
    let (found_id, found_num) = extract_vehicle_id_and_number(conflict_inner);
    format!(
        "Requested vehicleId '{}' and vehicleNumber '{}' do not belong to the same vehicle (found vehicleId: '{}', vehicleNumber: '{}')",
        q_id,
        q_num,
        found_id.as_deref().unwrap_or("unknown"),
        found_num.as_deref().unwrap_or("unknown")
    )
}

