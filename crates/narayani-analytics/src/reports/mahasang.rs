use super::common::{paginate, round2, DateBasis, DateRangeFilter, PaginationMeta};
use super::income::receipt_matches_fee_head;
use super::rbac::CallerBranchContext;
use narayani_core::{normalize_key, normalize_vehicle_search};
use narayani_date::canonical_fiscal_year_short;
use narayani_settings::AppSettings;
use narayani_storage::{ProjectedReceipt, ReportIndex};
use narayani_vehicle_history::VehicleAliasMap;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MahasangReportRequest {
    pub fiscal_year: Option<String>,
    pub branch: Option<String>,
    pub fee_head: Option<String>,
    pub vehicle_types: Option<Vec<String>>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub date_basis: Option<String>,
    pub search: Option<String>,
    pub page: Option<usize>,
    pub page_size: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MahasangSummary {
    pub total_bills: usize,
    pub total_membership: f64,
    pub total_other: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MahasangRow {
    pub id: String,
    pub vehicle_number: String,
    pub previous_vehicle_number: String,
    pub vehicle_type: String,
    pub owner_name: String,
    pub operator_name: String,
    pub owner_phone: String,
    pub note: String,
    pub receipt_number: String,
    pub membership_income: f64,
    pub other_income: f64,
    pub total: f64,
    pub period_months: i32,
    pub payment_method: String,
    pub billing_date: String,
    pub period_start: String,
    pub period_end: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MahasangReportResponse {
    pub summary: MahasangSummary,
    pub rows: Vec<MahasangRow>,
    pub pagination: PaginationMeta,
}

pub fn build_mahasang_rows(
    req: &MahasangReportRequest,
    caller: &CallerBranchContext,
    index: &ReportIndex,
    aliases: &VehicleAliasMap,
    settings: &AppSettings,
) -> (Vec<MahasangRow>, MahasangSummary, bool) {
    let dr = DateRangeFilter::parse(req.date_from.as_deref(), req.date_to.as_deref());
    if dr.invalid_range {
        return (Vec::new(), MahasangSummary::default(), true);
    }

    let basis = DateBasis::from_opt(req.date_basis.as_deref());
    let eff_branch = caller.effective_branch_lower(req.branch.as_deref());
    let fy_canon = req
        .fiscal_year
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("all"))
        .map(canonical_fiscal_year_short);

    let effective_vtypes: Vec<String> = match &req.vehicle_types {
        Some(explicit) => explicit
            .iter()
            .map(|s| normalize_key(s))
            .filter(|s| !s.is_empty())
            .collect(),
        None => settings
            .reports
            .mahasang
            .resolve_default_vehicle_types()
            .into_iter()
            .map(|s| normalize_key(&s))
            .filter(|s| !s.is_empty())
            .collect(),
    };
    let vt_set: Option<HashSet<String>> = if effective_vtypes.is_empty() {
        None
    } else {
        Some(effective_vtypes.into_iter().collect())
    };

    let search_q = req
        .search
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(normalize_key);
    let search_plate = req
        .search
        .as_deref()
        .map(normalize_vehicle_search)
        .filter(|s| !s.is_empty());

    let mut matched: Vec<(&ProjectedReceipt, MahasangRow)> = Vec::new();
    for r in index.receipts.values() {
        if let Some(ref fy) = fy_canon {
            if r.fiscal_year_canonical != *fy {
                continue;
            }
        }
        if !caller.matches_receipt(r, eff_branch.as_deref()) {
            continue;
        }
        if !receipt_matches_fee_head(r, req.fee_head.as_deref()) {
            continue;
        }
        // Spec: dateBasis == billingDate evaluates validity period interval overlap!
        if !dr.matches_receipt(r, basis, true) {
            continue;
        }

        let resolved = aliases.resolve(Some(r.vehicle_id.as_str()), Some(r.vehicle_number.as_str()));
        let (curr_vnum, prev_vnum, resolved_vt, resolved_owner, resolved_phone) =
            if let Some(res) = resolved {
                let target = res.target;
                let curr_num = if !target.vehicle_number.is_empty() {
                    target.vehicle_number.clone()
                } else {
                    r.vehicle_number.clone()
                };
                let prev_num = if !r.vehicle_number_norm.is_empty()
                    && !target.norm_vehicle_number.is_empty()
                    && r.vehicle_number_norm != target.norm_vehicle_number
                {
                    r.vehicle_number.clone()
                } else {
                    target
                        .previous_vehicle_numbers
                        .last()
                        .cloned()
                        .unwrap_or_else(|| "—".to_string())
                };
                let vt = if !r.vehicle_type.is_empty() {
                    r.vehicle_type.clone()
                } else {
                    target.vehicle_type.clone()
                };
                let owner = if !r.customer_name.is_empty() {
                    r.customer_name.clone()
                } else {
                    target.owner_name.clone()
                };
                let phone = if !r.customer_phone.is_empty() {
                    r.customer_phone.clone()
                } else {
                    "—".to_string()
                };
                (curr_num, prev_num, vt, owner, phone)
            } else {
                let phone = if !r.customer_phone.is_empty() {
                    r.customer_phone.clone()
                } else {
                    "—".to_string()
                };
                (
                    r.vehicle_number.clone(),
                    "—".to_string(),
                    r.vehicle_type.clone(),
                    r.customer_name.clone(),
                    phone,
                )
            };

        if let Some(ref set) = vt_set {
            if !set.contains(&normalize_key(&resolved_vt)) {
                continue;
            }
        }

        let paid_total = if r.is_paid { round2(r.total_amount) } else { 0.0 };
        let is_general = r.billing_type.eq_ignore_ascii_case("GENERAL");
        let membership_income = if !is_general { paid_total } else { 0.0 };
        let other_income = if is_general { paid_total } else { 0.0 };

        let row = MahasangRow {
            id: r.receipt_id.to_string(),
            vehicle_number: curr_vnum,
            previous_vehicle_number: prev_vnum,
            vehicle_type: resolved_vt,
            owner_name: resolved_owner,
            operator_name: r.operator_name.clone(),
            owner_phone: resolved_phone,
            note: r.note.clone(),
            receipt_number: r.receipt_number.clone(),
            membership_income,
            other_income,
            total: paid_total,
            period_months: r.billing_months,
            payment_method: r.payment_method.clone(),
            billing_date: r.billing_date_str.clone(),
            period_start: r.period_start_str.clone(),
            period_end: r.period_end_str.clone(),
            status: r.status.clone(),
        };

        if let Some(ref sq) = search_q {
            let hay_match = normalize_key(&row.vehicle_number).contains(sq)
                || normalize_key(&row.previous_vehicle_number).contains(sq)
                || normalize_key(&row.owner_name).contains(sq)
                || normalize_key(&row.operator_name).contains(sq)
                || normalize_key(&row.owner_phone).contains(sq)
                || normalize_key(&row.receipt_number).contains(sq)
                || normalize_key(&row.vehicle_type).contains(sq)
                || normalize_key(&row.note).contains(sq)
                || search_plate.as_ref().is_some_and(|sp| {
                    normalize_vehicle_search(&row.vehicle_number).contains(sp)
                        || normalize_vehicle_search(&row.previous_vehicle_number).contains(sp)
                });
            if !hay_match {
                continue;
            }
        }

        matched.push((r.as_ref(), row));
    }

    matched.sort_by(|a, b| {
        b.0.billing_date
            .cmp(&a.0.billing_date)
            .then_with(|| b.0.receipt_id.cmp(&a.0.receipt_id))
    });

    let mut total_bills = 0usize;
    let mut total_mem = 0.0;
    let mut total_oth = 0.0;
    let rows: Vec<MahasangRow> = matched
        .into_iter()
        .map(|(_, row)| {
            total_bills += 1;
            total_mem += row.membership_income;
            total_oth += row.other_income;
            row
        })
        .collect();

    let summary = MahasangSummary {
        total_bills,
        total_membership: round2(total_mem),
        total_other: round2(total_oth),
    };
    (rows, summary, false)
}

pub fn compute_mahasang_report(
    req: &MahasangReportRequest,
    caller: &CallerBranchContext,
    index: &ReportIndex,
    aliases: &VehicleAliasMap,
    settings: &AppSettings,
) -> MahasangReportResponse {
    let page = req.page.unwrap_or(1);
    let page_size = req.page_size.unwrap_or(25).clamp(1, 200);
    let (all_rows, summary, invalid) = build_mahasang_rows(req, caller, index, aliases, settings);
    if invalid {
        return MahasangReportResponse {
            summary: MahasangSummary::default(),
            rows: Vec::new(),
            pagination: PaginationMeta {
                page: page.max(1),
                page_size,
                total: 0,
                total_pages: 0,
            },
        };
    }
    let (rows, pagination) = paginate(&all_rows, page, page_size, 200);
    MahasangReportResponse {
        summary,
        rows,
        pagination,
    }
}
