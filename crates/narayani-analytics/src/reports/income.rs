use super::common::{
    format_owner_operator_lines, paginate, round2, DateBasis, DateRangeFilter, PaginationMeta,
};
use super::filter_options::{build_user_filter_aliases, UserFilterAliases};
use super::rbac::CallerBranchContext;
use narayani_core::{normalize_key, DatabaseResource};
use narayani_date::canonical_fiscal_year_short;
use narayani_storage::{ProjectedReceipt, ReportIndex};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct IncomeReportQuery {
    pub fiscal_year: Option<String>,
    pub branch: Option<String>,
    pub user: Option<String>,
    pub fee_head: Option<String>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub date_basis: Option<String>,
    pub page: Option<usize>,
    pub page_size: Option<usize>,
    pub sort_by: Option<String>,
    pub sort_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomeSummary {
    pub total_income: f64,
    pub total_transactions: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomeReportRow {
    pub id: String,
    pub receipt_id: String,
    pub receipt_number: String,
    pub payment_date: String,
    pub billing_date: String,
    pub period_start: String,
    pub period_end: String,
    pub billing_months: i32,
    pub customer_name: String,
    pub operator_name: String,
    pub vehicle_number: String,
    pub vehicle_type: String,
    pub branch_name: String,
    pub user_name: String,
    pub fiscal_year: String,
    pub payment_method: String,
    pub note: String,
    pub fee_head_id: String,
    pub fee_title: String,
    pub billing_entity: String,
    pub amount: f64,
    pub membership_income: f64,
    pub other_income: f64,
    pub receipt_total: f64,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomeReportResponse {
    pub summary: IncomeSummary,
    pub rows: Vec<IncomeReportRow>,
    pub pagination: PaginationMeta,
}

#[derive(Debug, Clone)]
pub struct AmdaniExportRow {
    pub sn: usize,
    pub date: String,
    pub receipt_number: String,
    pub vehicle_number: String,
    pub vehicle_type: String,
    pub owner_operator_display: String,
    pub membership_amount: f64,
    pub other_amount: f64,
    pub receipt_total: f64,
    pub payment_method: String,
    pub branch_name: String,
    pub user_name: String,
    pub fiscal_year: String,
    pub note: String,
}

pub(crate) fn receipt_matches_fee_head(r: &ProjectedReceipt, fh_raw: Option<&str>) -> bool {
    let Some(fh) = fh_raw.map(str::trim).filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("all"))
    else {
        return true;
    };
    let fh_norm = normalize_key(fh);
    r.items
        .iter()
        .any(|i| (!i.fee_head_id.is_empty() && i.fee_head_id == fh) || i.fee_title_norm == fh_norm)
}

pub(crate) fn filter_and_sort_income<'a>(
    query: &IncomeReportQuery,
    caller: &CallerBranchContext,
    index: &'a ReportIndex,
    extra_users: &[DatabaseResource],
) -> (Vec<&'a ProjectedReceipt>, bool) {
    let dr = DateRangeFilter::parse(query.date_from.as_deref(), query.date_to.as_deref());
    if dr.invalid_range {
        return (Vec::new(), true);
    }
    let basis = DateBasis::from_opt(query.date_basis.as_deref());
    let eff_branch = caller.effective_branch_lower(query.branch.as_deref());
    let fy_canon = query
        .fiscal_year
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("all"))
        .map(canonical_fiscal_year_short);
    let user_match: Option<UserFilterAliases> =
        build_user_filter_aliases(query.user.as_deref(), index, extra_users);

    let mut matched: Vec<&'a ProjectedReceipt> = index
        .receipts
        .values()
        .filter(|r| {
            // Cancelled / void / deleted receipts excluded from Income table & totals
            if matches!(
                r.status.as_str(),
                "CANCELLED" | "CANCELED" | "VOID" | "DELETED" | "AMENDED" | "ROLLED_BACK"
            ) {
                return false;
            }
            if let Some(ref fy) = fy_canon {
                if r.fiscal_year_canonical != *fy {
                    return false;
                }
            }
            if !caller.matches_receipt(r, eff_branch.as_deref()) {
                return false;
            }
            if let Some(ref um) = user_match {
                if !um.matches(r) {
                    return false;
                }
            }
            if !receipt_matches_fee_head(r, query.fee_head.as_deref()) {
                return false;
            }
            dr.matches_receipt(r, basis, false)
        })
        .map(|arc| arc.as_ref())
        .collect();

    let sort_by = query
        .sort_by
        .as_deref()
        .map(str::trim)
        .unwrap_or("paymentDate");
    let sort_asc = query
        .sort_dir
        .as_deref()
        .is_some_and(|d| d.trim().eq_ignore_ascii_case("asc"));

    matched.sort_by(|a, b| {
        let ord = match sort_by {
            "receiptNumber" => a.receipt_number.cmp(&b.receipt_number),
            "customerName" => a.customer_name.cmp(&b.customer_name),
            "receiptTotal" => a
                .total_amount
                .partial_cmp(&b.total_amount)
                .unwrap_or(std::cmp::Ordering::Equal),
            _ => a
                .payment_date
                .cmp(&b.payment_date)
                .then_with(|| a.receipt_id.cmp(&b.receipt_id)),
        };
        if sort_asc {
            ord
        } else {
            ord.reverse()
        }
    });

    (matched, false)
}

fn to_income_row(r: &ProjectedReceipt) -> IncomeReportRow {
    let first = r.items.first();
    let paid_val = if r.is_paid { r.total_amount } else { 0.0 };
    let is_general = r.billing_type.eq_ignore_ascii_case("GENERAL");
    let membership_income = if is_general { 0.0 } else { round2(paid_val) };
    let other_income = if is_general { round2(paid_val) } else { 0.0 };
    let user_name = if !r.created_by_name.is_empty() {
        r.created_by_name.clone()
    } else {
        r.created_by_username.clone()
    };

    IncomeReportRow {
        id: format!("{}-0", r.receipt_id),
        receipt_id: r.receipt_id.to_string(),
        receipt_number: r.receipt_number.clone(),
        payment_date: r.payment_date_str.clone(),
        billing_date: r.billing_date_str.clone(),
        period_start: r.period_start_str.clone(),
        period_end: r.period_end_str.clone(),
        billing_months: r.billing_months,
        customer_name: r.customer_name.clone(),
        operator_name: r.operator_name.clone(),
        vehicle_number: r.vehicle_number.clone(),
        vehicle_type: r.vehicle_type.clone(),
        branch_name: r.branch_name.clone(),
        user_name,
        fiscal_year: r.fiscal_year_canonical.clone(),
        payment_method: r.payment_method.clone(),
        note: r.note.clone(),
        fee_head_id: first.map(|i| i.fee_head_id.clone()).unwrap_or_default(),
        fee_title: r
            .items
            .iter()
            .map(|i| i.fee_title.as_str())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(", "),
        billing_entity: first.map(|i| i.billing_entity.clone()).unwrap_or_default(),
        amount: round2(paid_val),
        membership_income,
        other_income,
        receipt_total: round2(r.total_amount),
        status: r.status.clone(),
    }
}

pub fn compute_income_report(
    query: &IncomeReportQuery,
    caller: &CallerBranchContext,
    index: &ReportIndex,
    extra_users: &[DatabaseResource],
) -> IncomeReportResponse {
    let page = query.page.unwrap_or(1);
    let page_size = query.page_size.unwrap_or(15).clamp(1, 200);
    let (matched, invalid) = filter_and_sort_income(query, caller, index, extra_users);
    if invalid {
        return IncomeReportResponse {
            summary: IncomeSummary {
                total_income: 0.0,
                total_transactions: 0,
            },
            rows: Vec::new(),
            pagination: PaginationMeta {
                page: page.max(1),
                page_size,
                total: 0,
                total_pages: 0,
            },
        };
    }

    let mut total_income = 0.0;
    let mut total_transactions = 0usize;
    for r in &matched {
        if r.is_paid {
            total_income += r.total_amount;
            total_transactions += 1;
        }
    }

    let all_rows: Vec<IncomeReportRow> = matched.iter().map(|r| to_income_row(r)).collect();
    let (rows, pagination) = paginate(&all_rows, page, page_size, 200);

    IncomeReportResponse {
        summary: IncomeSummary {
            total_income: round2(total_income),
            total_transactions,
        },
        rows,
        pagination,
    }
}

pub fn build_amdani_export_rows(
    query: &IncomeReportQuery,
    caller: &CallerBranchContext,
    index: &ReportIndex,
    extra_users: &[DatabaseResource],
) -> (Vec<AmdaniExportRow>, f64, f64, f64) {
    let (matched, invalid) = filter_and_sort_income(query, caller, index, extra_users);
    if invalid {
        return (Vec::new(), 0.0, 0.0, 0.0);
    }
    let basis = DateBasis::from_opt(query.date_basis.as_deref());
    let mut total_mem = 0.0;
    let mut total_other = 0.0;
    let mut total_grand = 0.0;
    let mut out = Vec::with_capacity(matched.len());
    for (idx, r) in matched.iter().enumerate() {
        let paid_val = if r.is_paid { r.total_amount } else { 0.0 };
        let is_general = r.billing_type.eq_ignore_ascii_case("GENERAL");
        let mem = if is_general { 0.0 } else { round2(paid_val) };
        let oth = if is_general { round2(paid_val) } else { 0.0 };
        let tot = round2(paid_val);
        total_mem += mem;
        total_other += oth;
        total_grand += tot;
        let date = match basis {
            DateBasis::PaymentDate => r.payment_date_str.clone(),
            DateBasis::BillingDate => r.billing_date_str.clone(),
        };
        let user_name = if !r.created_by_name.is_empty() {
            r.created_by_name.clone()
        } else {
            r.created_by_username.clone()
        };
        out.push(AmdaniExportRow {
            sn: idx + 1,
            date,
            receipt_number: r.receipt_number.clone(),
            vehicle_number: r.vehicle_number.clone(),
            vehicle_type: r.vehicle_type.clone(),
            owner_operator_display: format_owner_operator_lines(
                &r.customer_name,
                &r.operator_name,
            ),
            membership_amount: mem,
            other_amount: oth,
            receipt_total: tot,
            payment_method: r.payment_method.clone(),
            branch_name: r.branch_name.clone(),
            user_name,
            fiscal_year: r.fiscal_year_canonical.clone(),
            note: r.note.clone(),
        });
    }
    (out, round2(total_mem), round2(total_other), round2(total_grand))
}
