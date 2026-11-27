use super::common::{paginate, round2, DateBasis, DateRangeFilter, PaginationMeta};
use super::filter_options::{build_user_filter_aliases, UserFilterAliases};
use super::income::receipt_matches_fee_head;
use super::rbac::CallerBranchContext;
use narayani_core::{normalize_key, DatabaseResource};
use narayani_date::canonical_fiscal_year_short;
use narayani_storage::{ProjectedReceipt, ReportIndex};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FeeHeadWiseQuery {
    pub fiscal_year: Option<String>,
    pub branch: Option<String>,
    pub user: Option<String>,
    pub fee_head: Option<String>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub date_basis: Option<String>,
    pub page: Option<usize>,
    pub page_size: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeeHeadColumnDto {
    pub id: String,
    pub name: String,
    pub display_order: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeeHeadWiseRow {
    pub receipt_id: String,
    pub receipt_number: String,
    pub billing_date: String,
    pub vehicle_number: String,
    pub vehicle_type: String,
    pub customer_name: String,
    pub operator_name: String,
    pub phone: String,
    pub address: String,
    pub note: String,
    pub total_amount: f64,
    pub amounts: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FeeHeadWiseTotals {
    pub amounts: BTreeMap<String, f64>,
    pub total: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeeHeadWiseResponse {
    pub configured_fee_heads: Vec<FeeHeadColumnDto>,
    pub visible_fee_heads: Vec<FeeHeadColumnDto>,
    pub rows: Vec<FeeHeadWiseRow>,
    pub page_totals: FeeHeadWiseTotals,
    pub all_totals: FeeHeadWiseTotals,
    pub pagination: PaginationMeta,
}

pub fn compute_fee_head_wise_report(
    query: &FeeHeadWiseQuery,
    caller: &CallerBranchContext,
    index: &ReportIndex,
    extra_users: &[DatabaseResource],
) -> FeeHeadWiseResponse {
    let page = query.page.unwrap_or(1);
    let page_size = query.page_size.unwrap_or(15).clamp(1, 200);
    let fy_canon = query
        .fiscal_year
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("all"))
        .map(canonical_fiscal_year_short);

    let mut configured_map: HashMap<String, FeeHeadColumnDto> = HashMap::new();
    let src_heads = match fy_canon.as_deref() {
        Some(fy) => index
            .fee_heads_by_fy
            .get(fy)
            .map(Vec::as_slice)
            .unwrap_or(&[]),
        None => index.all_fee_heads.as_slice(),
    };
    for fh in src_heads {
        configured_map
            .entry(fh.id.clone())
            .or_insert_with(|| FeeHeadColumnDto {
                id: fh.id.clone(),
                name: fh.name.clone(),
                display_order: fh.display_order,
            });
    }
    let mut configured_fee_heads: Vec<FeeHeadColumnDto> = configured_map.into_values().collect();
    configured_fee_heads.sort_by(|a, b| {
        a.display_order
            .cmp(&b.display_order)
            .then_with(|| a.name.cmp(&b.name))
    });

    let dr = DateRangeFilter::parse(query.date_from.as_deref(), query.date_to.as_deref());
    if dr.invalid_range {
        return FeeHeadWiseResponse {
            configured_fee_heads,
            visible_fee_heads: Vec::new(),
            rows: Vec::new(),
            page_totals: FeeHeadWiseTotals::default(),
            all_totals: FeeHeadWiseTotals::default(),
            pagination: PaginationMeta {
                page: page.max(1),
                page_size,
                total: 0,
                total_pages: 0,
            },
        };
    }

    let basis = DateBasis::from_opt(query.date_basis.as_deref());
    let eff_branch = caller.effective_branch_lower(query.branch.as_deref());
    let user_match: Option<UserFilterAliases> =
        build_user_filter_aliases(query.user.as_deref(), index, extra_users);

    let mut matched: Vec<&ProjectedReceipt> = index
        .receipts
        .values()
        .filter(|r| {
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

    // Sort by billingDate DESC, receiptNumber DESC
    matched.sort_by(|a, b| {
        b.billing_date
            .cmp(&a.billing_date)
            .then_with(|| b.receipt_number.cmp(&a.receipt_number))
            .then_with(|| b.receipt_id.cmp(&a.receipt_id))
    });

    // Include any fee heads appearing in matched receipts not already in configured_fee_heads
    let mut cfg_ids: HashMap<String, usize> = configured_fee_heads
        .iter()
        .enumerate()
        .map(|(i, c)| (c.id.clone(), i))
        .collect();

    let mut all_amounts: BTreeMap<String, f64> = BTreeMap::new();
    let mut all_total = 0.0f64;
    let mut all_rows: Vec<FeeHeadWiseRow> = Vec::with_capacity(matched.len());

    for r in &matched {
        let mut row_amounts: BTreeMap<String, f64> = BTreeMap::new();
        for item in &r.items {
            let key = if !item.fee_head_id.is_empty() {
                item.fee_head_id.clone()
            } else {
                item.fee_title_norm.clone()
            };
            if key.is_empty() {
                continue;
            }
            if !cfg_ids.contains_key(&key) {
                let idx = configured_fee_heads.len();
                configured_fee_heads.push(FeeHeadColumnDto {
                    id: key.clone(),
                    name: if !item.fee_title.is_empty() {
                        item.fee_title.clone()
                    } else {
                        key.clone()
                    },
                    display_order: 10_000 + idx as i32,
                });
                cfg_ids.insert(key.clone(), idx);
            }
            let item_val = if r.is_paid { item.amount } else { 0.0 };
            *row_amounts.entry(key.clone()).or_insert(0.0) += item_val;
            *all_amounts.entry(key).or_insert(0.0) += item_val;
        }
        for v in row_amounts.values_mut() {
            *v = round2(*v);
        }
        let r_total = if r.is_paid { round2(r.total_amount) } else { 0.0 };
        all_total += r_total;
        all_rows.push(FeeHeadWiseRow {
            receipt_id: r.receipt_id.to_string(),
            receipt_number: r.receipt_number.clone(),
            billing_date: r.billing_date_str.clone(),
            vehicle_number: r.vehicle_number.clone(),
            vehicle_type: r.vehicle_type.clone(),
            customer_name: r.customer_name.clone(),
            operator_name: r.operator_name.clone(),
            phone: r.customer_phone.clone(),
            address: r.address.clone(),
            note: r.note.clone(),
            total_amount: r_total,
            amounts: row_amounts,
        });
    }

    for v in all_amounts.values_mut() {
        *v = round2(*v);
    }

    let fh_filter = query
        .fee_head
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("all"));
    let fh_filter_norm = fh_filter.map(normalize_key);

    let visible_fee_heads: Vec<FeeHeadColumnDto> = configured_fee_heads
        .iter()
        .filter(|fh| {
            let matches_filter = match (&fh_filter, &fh_filter_norm) {
                (Some(raw), Some(norm)) => {
                    fh.id == *raw || normalize_key(&fh.name) == *norm
                }
                _ => true,
            };
            let non_zero = all_amounts.get(&fh.id).copied().unwrap_or(0.0).abs() > 1e-6;
            matches_filter && non_zero
        })
        .cloned()
        .collect();

    let (rows, pagination) = paginate(&all_rows, page, page_size, 200);
    let mut page_amounts: BTreeMap<String, f64> = BTreeMap::new();
    let mut page_total = 0.0;
    for row in &rows {
        page_total += row.total_amount;
        for (k, &v) in &row.amounts {
            *page_amounts.entry(k.clone()).or_insert(0.0) += v;
        }
    }
    for v in page_amounts.values_mut() {
        *v = round2(*v);
    }

    FeeHeadWiseResponse {
        configured_fee_heads,
        visible_fee_heads,
        rows,
        page_totals: FeeHeadWiseTotals {
            amounts: page_amounts,
            total: round2(page_total),
        },
        all_totals: FeeHeadWiseTotals {
            amounts: all_amounts,
            total: round2(all_total),
        },
        pagination,
    }
}

pub fn compute_fee_head_wise_all_rows(
    query: &FeeHeadWiseQuery,
    caller: &CallerBranchContext,
    index: &ReportIndex,
    extra_users: &[DatabaseResource],
) -> FeeHeadWiseResponse {
    let mut full_q = query.clone();
    full_q.page = Some(1);
    full_q.page_size = Some(200_000);
    let page_resp = compute_fee_head_wise_report(&full_q, caller, index, extra_users);
    // Note paginate caps to max_page_size 200; for export fetch unpaginated rows cleanly:
    let _ = page_resp;
    compute_fee_head_wise_unpaginated(query, caller, index, extra_users)
}

pub fn compute_fee_head_wise_unpaginated(
    query: &FeeHeadWiseQuery,
    caller: &CallerBranchContext,
    index: &ReportIndex,
    extra_users: &[DatabaseResource],
) -> FeeHeadWiseResponse {
    let mut res = compute_fee_head_wise_report(query, caller, index, extra_users);
    if res.pagination.total > res.rows.len() {
        // Re-collect unpaginated rows when total exceeds 200
        let dr = DateRangeFilter::parse(query.date_from.as_deref(), query.date_to.as_deref());
        if !dr.invalid_range {
            let basis = DateBasis::from_opt(query.date_basis.as_deref());
            let eff_branch = caller.effective_branch_lower(query.branch.as_deref());
            let fy_canon = query
                .fiscal_year
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("all"))
                .map(canonical_fiscal_year_short);
            let user_match = build_user_filter_aliases(query.user.as_deref(), index, extra_users);
            let mut matched: Vec<&ProjectedReceipt> = index
                .receipts
                .values()
                .filter(|r| {
                    !matches!(
                        r.status.as_str(),
                        "CANCELLED" | "CANCELED" | "VOID" | "DELETED" | "AMENDED" | "ROLLED_BACK"
                    ) && fy_canon
                        .as_ref()
                        .is_none_or(|fy| r.fiscal_year_canonical == *fy)
                        && caller.matches_receipt(r, eff_branch.as_deref())
                        && user_match.as_ref().is_none_or(|um| um.matches(r))
                        && receipt_matches_fee_head(r, query.fee_head.as_deref())
                        && dr.matches_receipt(r, basis, false)
                })
                .map(|arc| arc.as_ref())
                .collect();
            matched.sort_by(|a, b| {
                b.billing_date
                    .cmp(&a.billing_date)
                    .then_with(|| b.receipt_number.cmp(&a.receipt_number))
                    .then_with(|| b.receipt_id.cmp(&a.receipt_id))
            });
            res.rows = matched
                .iter()
                .map(|r| {
                    let mut row_amounts = BTreeMap::new();
                    for item in &r.items {
                        let key = if !item.fee_head_id.is_empty() {
                            item.fee_head_id.clone()
                        } else {
                            item.fee_title_norm.clone()
                        };
                        if !key.is_empty() {
                            let val = if r.is_paid { item.amount } else { 0.0 };
                            *row_amounts.entry(key).or_insert(0.0) += val;
                        }
                    }
                    for v in row_amounts.values_mut() {
                        *v = round2(*v);
                    }
                    FeeHeadWiseRow {
                        receipt_id: r.receipt_id.to_string(),
                        receipt_number: r.receipt_number.clone(),
                        billing_date: r.billing_date_str.clone(),
                        vehicle_number: r.vehicle_number.clone(),
                        vehicle_type: r.vehicle_type.clone(),
                        customer_name: r.customer_name.clone(),
                        operator_name: r.operator_name.clone(),
                        phone: r.customer_phone.clone(),
                        address: r.address.clone(),
                        note: r.note.clone(),
                        total_amount: if r.is_paid { round2(r.total_amount) } else { 0.0 },
                        amounts: row_amounts,
                    }
                })
                .collect();
        }
    }
    res
}
