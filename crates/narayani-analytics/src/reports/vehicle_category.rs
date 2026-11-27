use super::common::{paginate, round2, DateBasis, DateRangeFilter, PaginationMeta};
use super::income::receipt_matches_fee_head;
use super::rbac::CallerBranchContext;
use narayani_core::normalize_key;
use narayani_date::canonical_fiscal_year_short;
use narayani_storage::{ProjectedReceipt, ReportIndex};
use narayani_vehicle_history::VehicleAliasMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct VehicleCategoryQuery {
    pub fiscal_year: Option<String>,
    pub branch: Option<String>,
    pub fee_head: Option<String>,
    pub category: Option<String>,
    pub vehicle_type: Option<String>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub page: Option<usize>,
    pub page_size: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VehicleCategoryRow {
    pub receipt_id: String,
    pub receipt_number: String,
    pub vehicle_number: String,
    pub category: String,
    pub vehicle_type: String,
    pub route: String,
    pub route_permit_expiry: String,
    pub owner_name: String,
    pub operator_name: String,
    pub note: String,
    pub membership_income: f64,
    pub other_income: f64,
    pub total: f64,
    pub period_months: i32,
    pub payment_method: String,
    pub billing_date: String,
    pub period_start: String,
    pub period_end: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct VehicleCategoryTotals {
    pub membership: f64,
    pub other: f64,
    pub total: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VehicleCategoryResponse {
    pub rows: Vec<VehicleCategoryRow>,
    pub page_totals: VehicleCategoryTotals,
    pub filtered_totals: VehicleCategoryTotals,
    pub pagination: PaginationMeta,
}

fn allowed_page_size(raw: Option<usize>) -> usize {
    match raw.unwrap_or(50) {
        25 => 25,
        100 => 100,
        _ => 50,
    }
}

pub fn build_vehicle_category_rows(
    query: &VehicleCategoryQuery,
    caller: &CallerBranchContext,
    index: &ReportIndex,
    aliases: &VehicleAliasMap,
) -> (Vec<VehicleCategoryRow>, VehicleCategoryTotals, bool) {
    let dr = DateRangeFilter::parse(query.date_from.as_deref(), query.date_to.as_deref());
    if dr.invalid_range {
        return (Vec::new(), VehicleCategoryTotals::default(), true);
    }

    let eff_branch = caller.effective_branch_lower(query.branch.as_deref());
    let fy_canon = query
        .fiscal_year
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("all"))
        .map(canonical_fiscal_year_short);
    let cat_filter = query
        .category
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("all"))
        .map(normalize_key);
    let vt_filter = query
        .vehicle_type
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("all"))
        .map(normalize_key);

    let mut matched: Vec<(&ProjectedReceipt, String, String)> = Vec::new();
    for r in index.receipts.values() {
        if matches!(
            r.status.as_str(),
            "CANCELLED" | "CANCELED" | "VOID" | "DELETED" | "AMENDED" | "ROLLED_BACK"
        ) {
            continue;
        }
        if let Some(ref fy) = fy_canon {
            if r.fiscal_year_canonical != *fy {
                continue;
            }
        }
        if !caller.matches_receipt(r, eff_branch.as_deref()) {
            continue;
        }
        if !receipt_matches_fee_head(r, query.fee_head.as_deref()) {
            continue;
        }
        // Spec: dateFrom <= billingDate <= dateTo
        if !dr.matches_receipt(r, DateBasis::BillingDate, false) {
            continue;
        }

        let effective_vt = if !r.vehicle_type.is_empty() {
            r.vehicle_type.clone()
        } else {
            aliases
                .resolve(Some(r.vehicle_id.as_str()), Some(r.vehicle_number.as_str()))
                .map(|res| res.target.vehicle_type.clone())
                .unwrap_or_default()
        };
        let category = index.resolve_category(&effective_vt);

        if let Some(ref cf) = cat_filter {
            if normalize_key(&category) != *cf {
                continue;
            }
        }
        if let Some(ref vtf) = vt_filter {
            if normalize_key(&effective_vt) != *vtf {
                continue;
            }
        }
        matched.push((r.as_ref(), category, effective_vt));
    }

    matched.sort_by(|a, b| {
        b.0.billing_date
            .cmp(&a.0.billing_date)
            .then_with(|| b.0.receipt_id.cmp(&a.0.receipt_id))
    });

    let mut total_mem = 0.0;
    let mut rows = Vec::with_capacity(matched.len());
    for (r, category, effective_vt) in matched {
        let paid = if r.is_paid { round2(r.total_amount) } else { 0.0 };
        total_mem += paid;
        rows.push(VehicleCategoryRow {
            receipt_id: r.receipt_id.to_string(),
            receipt_number: r.receipt_number.clone(),
            vehicle_number: r.vehicle_number.clone(),
            category,
            vehicle_type: effective_vt,
            route: r.route.clone(),
            route_permit_expiry: r.route_permit_expiry.clone(),
            owner_name: r.customer_name.clone(),
            operator_name: r.operator_name.clone(),
            note: r.note.clone(),
            membership_income: paid,
            other_income: 0.0,
            total: paid,
            period_months: r.billing_months,
            payment_method: r.payment_method.clone(),
            billing_date: r.billing_date_str.clone(),
            period_start: r.period_start_str.clone(),
            period_end: r.period_end_str.clone(),
        });
    }

    let totals = VehicleCategoryTotals {
        membership: round2(total_mem),
        other: 0.0,
        total: round2(total_mem),
    };
    (rows, totals, false)
}

pub fn compute_vehicle_category_report(
    query: &VehicleCategoryQuery,
    caller: &CallerBranchContext,
    index: &ReportIndex,
    aliases: &VehicleAliasMap,
) -> VehicleCategoryResponse {
    let page = query.page.unwrap_or(1);
    let page_size = allowed_page_size(query.page_size);
    let (all_rows, filtered_totals, invalid) =
        build_vehicle_category_rows(query, caller, index, aliases);
    if invalid {
        return VehicleCategoryResponse {
            rows: Vec::new(),
            page_totals: VehicleCategoryTotals::default(),
            filtered_totals: VehicleCategoryTotals::default(),
            pagination: PaginationMeta {
                page: page.max(1),
                page_size,
                total: 0,
                total_pages: 0,
            },
        };
    }

    let (rows, pagination) = paginate(&all_rows, page, page_size, 200);
    let mut p_mem = 0.0;
    for r in &rows {
        p_mem += r.membership_income;
    }
    VehicleCategoryResponse {
        rows,
        page_totals: VehicleCategoryTotals {
            membership: round2(p_mem),
            other: 0.0,
            total: round2(p_mem),
        },
        filtered_totals,
        pagination,
    }
}
