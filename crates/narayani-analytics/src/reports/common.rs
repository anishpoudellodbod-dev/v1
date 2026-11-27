use chrono::NaiveDate;
use narayani_date::parse_naive_date_converting_bs_to_ad;
use narayani_storage::ProjectedReceipt;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DateBasis {
    #[default]
    PaymentDate,
    BillingDate,
}

impl DateBasis {
    pub fn from_opt(raw: Option<&str>) -> Self {
        match raw.map(str::trim).unwrap_or("") {
            s if s.eq_ignore_ascii_case("billingDate")
                || s.eq_ignore_ascii_case("billing_date") =>
            {
                Self::BillingDate
            }
            _ => Self::PaymentDate,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct DateRangeFilter {
    pub date_from: Option<NaiveDate>,
    pub date_to: Option<NaiveDate>,
    pub invalid_range: bool,
}

impl DateRangeFilter {
    pub fn parse(date_from_raw: Option<&str>, date_to_raw: Option<&str>) -> Self {
        let mut invalid = false;
        let date_from = match date_from_raw.map(str::trim).filter(|s| !s.is_empty()) {
            Some(s) => match parse_naive_date_converting_bs_to_ad(s) {
                Some(d) => Some(d),
                None => {
                    invalid = true;
                    None
                }
            },
            None => None,
        };
        let date_to = match date_to_raw.map(str::trim).filter(|s| !s.is_empty()) {
            Some(s) => match parse_naive_date_converting_bs_to_ad(s) {
                Some(d) => Some(d),
                None => {
                    invalid = true;
                    None
                }
            },
            None => None,
        };
        if let (Some(df), Some(dt)) = (date_from, date_to) {
            if df > dt {
                invalid = true;
            }
        }
        Self {
            date_from,
            date_to,
            invalid_range: invalid,
        }
    }

    pub fn matches_receipt(
        &self,
        receipt: &ProjectedReceipt,
        basis: DateBasis,
        mahasang_interval: bool,
    ) -> bool {
        if self.invalid_range {
            return false;
        }
        if self.date_from.is_none() && self.date_to.is_none() {
            return true;
        }
        if mahasang_interval && basis == DateBasis::BillingDate {
            let p_start = receipt
                .period_start
                .or(receipt.billing_date)
                .or(receipt.payment_date);
            let p_end = receipt
                .period_end
                .or(receipt.period_start)
                .or(receipt.billing_date)
                .or(receipt.payment_date);
            if let Some(df) = self.date_from {
                if p_end.is_none_or(|pe| pe < df) {
                    return false;
                }
            }
            if let Some(dt) = self.date_to {
                if p_start.is_none_or(|ps| ps > dt) {
                    return false;
                }
            }
            return true;
        }

        let target = match basis {
            DateBasis::PaymentDate => receipt.payment_date,
            DateBasis::BillingDate => receipt.billing_date,
        };
        let Some(d) = target else {
            return false;
        };
        if let Some(df) = self.date_from {
            if d < df {
                return false;
            }
        }
        if let Some(dt) = self.date_to {
            if d > dt {
                return false;
            }
        }
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaginationMeta {
    pub page: usize,
    pub page_size: usize,
    pub total: usize,
    pub total_pages: usize,
}

pub fn paginate<T: Clone>(
    items: &[T],
    page: usize,
    page_size: usize,
    max_page_size: usize,
) -> (Vec<T>, PaginationMeta) {
    let total = items.len();
    let page_size = page_size.clamp(1, max_page_size.max(1));
    let page = page.max(1);
    let total_pages = if total == 0 {
        0
    } else {
        (total + page_size - 1) / page_size
    };
    let start = (page - 1).saturating_mul(page_size);
    let slice = if start >= total {
        Vec::new()
    } else {
        let end = (start + page_size).min(total);
        items[start..end].to_vec()
    };
    (
        slice,
        PaginationMeta {
            page,
            page_size,
            total,
            total_pages,
        },
    )
}

#[inline]
pub fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

pub fn format_owner_operator_lines(owner: &str, operator: &str) -> String {
    let o = owner.trim();
    let op = operator.trim();
    if !op.is_empty() && !op.eq_ignore_ascii_case(o) {
        if o.is_empty() {
            format!("संचालक: {op}")
        } else {
            format!("संचालक: {op}\nधनीको नाम: {o}")
        }
    } else {
        o.to_string()
    }
}

pub fn sanitize_csv_cell(raw: &str) -> String {
    let trimmed_start = raw.trim_start_matches(' ');
    if trimmed_start.starts_with('=')
        || trimmed_start.starts_with('+')
        || trimmed_start.starts_with('-')
        || trimmed_start.starts_with('@')
        || raw.starts_with('\t')
        || raw.starts_with('\r')
    {
        format!("'{raw}")
    } else {
        raw.to_string()
    }
}
