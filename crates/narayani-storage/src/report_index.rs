use chrono::NaiveDate;
use narayani_core::{
    extract_bill_vehicle, extract_string_or_int, is_bill_ignored, normalize_key,
    normalize_vehicle_number, DatabaseResource,
};
use narayani_date::{
    canonical_fiscal_year_short, parse_naive_date_converting_bs_to_ad, resolve_bill_fiscal_year,
    FiscalYearConfig,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectedReceiptItem {
    pub item_index: usize,
    pub fee_head_id: String,
    pub fee_title: String,
    pub fee_title_norm: String,
    pub billing_entity: String,
    pub billing_entity_id: String,
    pub amount: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectedReceipt {
    pub receipt_id: i32,
    pub receipt_number: String,
    pub billing_type: String,
    pub status: String,
    pub is_paid: bool,
    pub branch_id: String,
    pub branch_name: String,
    pub branch_name_lower: String,
    pub branch_code: String,
    pub fiscal_year_id: String,
    pub fiscal_year_canonical: String,
    pub created_by_id: String,
    pub created_by_name: String,
    pub created_by_username: String,
    pub vehicle_id: String,
    pub vehicle_number: String,
    pub vehicle_number_norm: String,
    pub vehicle_type: String,
    pub vehicle_type_norm: String,
    pub customer_name: String,
    pub operator_name: String,
    pub customer_phone: String,
    pub address: String,
    pub route: String,
    pub route_permit_expiry: String,
    pub note: String,
    pub payment_date: Option<NaiveDate>,
    pub billing_date: Option<NaiveDate>,
    pub period_start: Option<NaiveDate>,
    pub period_end: Option<NaiveDate>,
    pub payment_date_str: String,
    pub billing_date_str: String,
    pub period_start_str: String,
    pub period_end_str: String,
    pub billing_months: i32,
    pub subtotal: f64,
    pub discount: f64,
    pub vat_amount: f64,
    pub total_amount: f64,
    pub payment_method: String,
    pub paid_at: String,
    pub created_at: String,
    pub last_printed_at: Option<String>,
    pub items: Vec<ProjectedReceiptItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeeHeadDef {
    pub id: String,
    pub name: String,
    pub display_order: i32,
    pub fiscal_year_canonical: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchDef {
    pub id: String,
    pub name: String,
    pub name_lower: String,
    pub code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserDef {
    pub id: String,
    pub name: String,
    pub email: String,
    pub branch_id: String,
    pub branch_name: String,
}

fn s_opt(v: Option<&Value>) -> String {
    v.and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("")
        .to_string()
}

fn f64_opt(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.trim().parse::<f64>().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn i32_opt(v: Option<&Value>) -> i32 {
    match v {
        Some(Value::Number(n)) => n.as_i64().and_then(|x| i32::try_from(x).ok()).unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse::<i32>().unwrap_or(0),
        _ => 0,
    }
}

fn date_part(raw: &str) -> String {
    let t = raw.trim();
    if let Some((head, _)) = t.split_once(&['T', ' '][..]) {
        head.to_string()
    } else {
        t.to_string()
    }
}

fn resolve_bill_status(inner: &Value) -> (String, bool) {
    if is_bill_ignored(inner) {
        let explicit = s_opt(inner.get("status"));
        let status = if explicit.is_empty() {
            // Check auditHistory last cancelling action
            let hist_action = inner
                .get("auditHistory")
                .and_then(Value::as_array)
                .and_then(|arr| {
                    arr.iter().rev().find_map(|e| {
                        let a = e.get("action").and_then(Value::as_str)?.trim();
                        if a.eq_ignore_ascii_case("CANCELLED")
                            || a.eq_ignore_ascii_case("CANCELED")
                            || a.eq_ignore_ascii_case("VOID")
                        {
                            Some(a.to_ascii_uppercase())
                        } else {
                            None
                        }
                    })
                });
            hist_action.unwrap_or_else(|| "CANCELLED".to_string())
        } else {
            explicit.to_ascii_uppercase()
        };
        return (status, false);
    }

    let raw_status = s_opt(inner.get("status"));
    if raw_status.is_empty() {
        return ("PAID".to_string(), true);
    }
    let up = raw_status.to_ascii_uppercase();
    let is_paid = up == "PAID";
    (up, is_paid)
}

fn extract_route_permit(vehicle_obj: Option<&Value>, inner: &Value) -> (String, String) {
    let mut route = vehicle_obj
        .map(|v| {
            let r = s_opt(v.pointer("/Compliance/RoutePermit/route"))
                .if_empty_then(|| s_opt(v.pointer("/Compliance/RoutePermit/Route")))
                .if_empty_then(|| s_opt(v.get("route")));
            r
        })
        .unwrap_or_default();
    if route.is_empty() {
        route = s_opt(inner.get("route"));
    }

    let expiry = vehicle_obj
        .map(|v| {
            s_opt(v.pointer("/Compliance/RoutePermit/ExpiryBs"))
                .if_empty_then(|| s_opt(v.pointer("/Compliance/RoutePermit/expiryBs")))
                .if_empty_then(|| s_opt(v.pointer("/Compliance/RoutePermit/ExpiryAd")))
                .if_empty_then(|| s_opt(v.pointer("/Compliance/RoutePermit/expiryAd")))
                .if_empty_then(|| s_opt(v.pointer("/Compliance/RoutePermit/expiryDate")))
                .if_empty_then(|| s_opt(v.pointer("/Compliance/RoutePermit/routePermitExpiry")))
                .if_empty_then(|| s_opt(v.get("routePermitExpiry")))
        })
        .unwrap_or_default();

    (route, expiry)
}

trait EmptyFallback {
    fn if_empty_then<F: FnOnce() -> String>(self, f: F) -> String;
}

impl EmptyFallback for String {
    fn if_empty_then<F: FnOnce() -> String>(self, f: F) -> String {
        if self.is_empty() {
            f()
        } else {
            self
        }
    }
}

impl ProjectedReceipt {
    pub fn from_bill(res: &DatabaseResource, fy_config: &mut FiscalYearConfig) -> Option<Self> {
        let receipt_id = res.id?;
        let inner = res.inner_content.as_ref()?;
        if !inner.is_object() {
            return None;
        }

        let (status, is_paid) = resolve_bill_status(inner);

        let raw_btype = s_opt(inner.get("billingType").or_else(|| inner.get("BillingType")));
        let billing_type = if raw_btype.is_empty() {
            "MEMBERSHIP".to_string()
        } else {
            raw_btype.to_ascii_uppercase()
        };

        let branch_obj = inner.get("branch");
        let branch_id = branch_obj
            .and_then(|b| extract_string_or_int(b.get("id")))
            .or_else(|| extract_string_or_int(inner.get("branchId")))
            .unwrap_or_default();
        let branch_name = branch_obj
            .map(|b| s_opt(b.get("name").or_else(|| b.get("branchName"))))
            .unwrap_or_default()
            .if_empty_then(|| s_opt(inner.get("branchName")));
        let branch_code = branch_obj
            .map(|b| {
                s_opt(
                    b.get("BranchCode")
                        .or_else(|| b.get("branchCode"))
                        .or_else(|| b.get("code")),
                )
            })
            .unwrap_or_default();
        let branch_name_lower = normalize_key(&branch_name);

        let raw_receipt_num = s_opt(inner.get("receiptNumber").or_else(|| inner.get("billNumber")));
        let receipt_number = if !raw_receipt_num.is_empty() {
            raw_receipt_num
        } else if !branch_code.is_empty() {
            format!("{branch_code}.{receipt_id}")
        } else {
            receipt_id.to_string()
        };

        let fiscal_year_id = extract_string_or_int(inner.get("fiscalYearId"))
            .or_else(|| extract_string_or_int(inner.get("fiscalYear")))
            .unwrap_or_default();
        let resolved_fy = resolve_bill_fiscal_year(inner, fy_config)
            .unwrap_or_else(|| fiscal_year_id.clone());
        let fy_display = fy_config.display_for(&resolved_fy);
        let fiscal_year_canonical = canonical_fiscal_year_short(&fy_display);

        let created_by_obj = inner.get("createdBy");
        let created_by_id = created_by_obj
            .and_then(|c| extract_string_or_int(c.get("id")))
            .or_else(|| extract_string_or_int(inner.get("createdById")))
            .or_else(|| extract_string_or_int(inner.get("userId")))
            .unwrap_or_default();
        let created_by_name = created_by_obj
            .map(|c| s_opt(c.get("name")))
            .unwrap_or_default()
            .if_empty_then(|| s_opt(inner.get("userName")))
            .if_empty_then(|| s_opt(inner.get("createdByName")));
        let created_by_username = created_by_obj
            .map(|c| s_opt(c.get("username").or_else(|| c.get("email"))))
            .unwrap_or_default();

        let (vid_opt, vnum_opt) = extract_bill_vehicle(inner);
        let vehicle_id = vid_opt.unwrap_or_default();
        let vehicle_number = vnum_opt.map(str::trim).unwrap_or("").to_string();
        let vehicle_number_norm = normalize_vehicle_number(&vehicle_number).unwrap_or_default();

        let vehicle_obj = inner.get("vehicle");
        let vehicle_type = vehicle_obj
            .map(|v| s_opt(v.get("vehicleType").or_else(|| v.get("VehicleType"))))
            .unwrap_or_default()
            .if_empty_then(|| s_opt(inner.get("vehicleType").or_else(|| inner.get("VehicleType"))));
        let vehicle_type_norm = normalize_key(&vehicle_type);

        let customer_name = s_opt(inner.get("customerName"))
            .if_empty_then(|| {
                inner
                    .get("customer")
                    .map(|c| s_opt(c.get("name")))
                    .unwrap_or_default()
            })
            .if_empty_then(|| {
                vehicle_obj
                    .map(|v| s_opt(v.get("ownerName").or_else(|| v.get("OwnerName"))))
                    .unwrap_or_default()
            })
            .if_empty_then(|| s_opt(inner.get("ownerName")));

        let operator_name = s_opt(inner.get("operatorName"))
            .if_empty_then(|| {
                inner
                    .get("operator")
                    .map(|o| s_opt(o.get("name")))
                    .unwrap_or_default()
            })
            .if_empty_then(|| {
                vehicle_obj
                    .map(|v| s_opt(v.get("operatorName").or_else(|| v.get("OperatorName"))))
                    .unwrap_or_default()
            });

        let customer_phone = inner
            .get("customer")
            .map(|c| s_opt(c.get("phone")))
            .unwrap_or_default()
            .if_empty_then(|| s_opt(inner.get("customerPhone")))
            .if_empty_then(|| {
                vehicle_obj
                    .map(|v| s_opt(v.get("smsPhone").or_else(|| v.get("phone"))))
                    .unwrap_or_default()
            })
            .if_empty_then(|| s_opt(inner.get("phone")));

        let address = vehicle_obj
            .map(|v| s_opt(v.get("address")))
            .unwrap_or_default()
            .if_empty_then(|| s_opt(inner.get("address")));

        let (route, route_permit_expiry) = extract_route_permit(vehicle_obj, inner);
        let note = s_opt(inner.get("note"));

        let paid_at = s_opt(inner.get("paidAt"));
        let payment_date_raw = s_opt(inner.get("paymentDate"));
        let billing_date_raw = s_opt(inner.get("billingDate"));
        let period_start_raw = s_opt(inner.get("periodStart"));
        let period_end_raw = s_opt(inner.get("periodEnd").or_else(|| inner.get("validUntil")));
        let created_at = res
            .external_content
            .as_ref()
            .map(|ext| s_opt(ext.get("createdTime").or_else(|| ext.get("createdAt"))))
            .unwrap_or_default();

        let pdate_src = if !paid_at.is_empty() {
            paid_at.as_str()
        } else if !payment_date_raw.is_empty() {
            payment_date_raw.as_str()
        } else {
            billing_date_raw.as_str()
        };
        let payment_date = parse_naive_date_converting_bs_to_ad(pdate_src);
        let payment_date_str = payment_date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| date_part(pdate_src));

        let bdate_src = if !billing_date_raw.is_empty() {
            billing_date_raw.as_str()
        } else {
            pdate_src
        };
        let billing_date = parse_naive_date_converting_bs_to_ad(bdate_src).or(payment_date);
        let billing_date_str = billing_date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| date_part(bdate_src));

        let pstart_src = if !period_start_raw.is_empty() {
            period_start_raw.as_str()
        } else {
            bdate_src
        };
        let period_start = parse_naive_date_converting_bs_to_ad(pstart_src).or(billing_date);
        let period_start_str = if !period_start_raw.is_empty() {
            date_part(&period_start_raw)
        } else {
            billing_date_str.clone()
        };

        let pend_src = if !period_end_raw.is_empty() {
            period_end_raw.as_str()
        } else {
            pstart_src
        };
        let period_end = parse_naive_date_converting_bs_to_ad(pend_src).or(period_start);
        let period_end_str = if !period_end_raw.is_empty() {
            date_part(&period_end_raw)
        } else {
            period_start_str.clone()
        };

        let billing_months = i32_opt(inner.get("billingMonths"));
        let subtotal = f64_opt(inner.get("subtotal"));
        let discount = f64_opt(inner.get("discount"));
        let vat_amount = f64_opt(inner.get("vatAmount"));
        let total_amount = inner
            .get("total")
            .or_else(|| inner.get("amount"))
            .map(|v| f64_opt(Some(v)))
            .unwrap_or(subtotal);
        let payment_method = s_opt(inner.get("paymentMethod"));

        let last_printed_at = inner
            .get("auditHistory")
            .and_then(Value::as_array)
            .and_then(|arr| {
                arr.iter().rev().find_map(|entry| {
                    let act = entry.get("action").and_then(Value::as_str)?.trim();
                    if act.eq_ignore_ascii_case("PRINTED")
                        || act.eq_ignore_ascii_case("REPRINTED")
                    {
                        entry
                            .get("at")
                            .and_then(Value::as_str)
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                    } else {
                        None
                    }
                })
            });

        let mut items = Vec::new();
        if let Some(arr) = inner.get("items").and_then(Value::as_array) {
            for (idx, item) in arr.iter().enumerate() {
                let fee_head_id = extract_string_or_int(item.get("feeHeadId"))
                    .or_else(|| extract_string_or_int(item.get("id")))
                    .unwrap_or_default();
                let fee_title = s_opt(item.get("title").or_else(|| item.get("feeTitle")));
                let fee_title_norm = normalize_key(&fee_title);
                let billing_entity = s_opt(item.get("billingEntity"));
                let billing_entity_id = extract_string_or_int(item.get("billingEntityId"))
                    .unwrap_or_default();
                let amount = f64_opt(item.get("amount"));
                items.push(ProjectedReceiptItem {
                    item_index: idx,
                    fee_head_id,
                    fee_title,
                    fee_title_norm,
                    billing_entity,
                    billing_entity_id,
                    amount,
                });
            }
        }

        Some(Self {
            receipt_id,
            receipt_number,
            billing_type,
            status,
            is_paid,
            branch_id,
            branch_name,
            branch_name_lower,
            branch_code,
            fiscal_year_id,
            fiscal_year_canonical,
            created_by_id,
            created_by_name,
            created_by_username,
            vehicle_id,
            vehicle_number,
            vehicle_number_norm,
            vehicle_type,
            vehicle_type_norm,
            customer_name,
            operator_name,
            customer_phone,
            address,
            route,
            route_permit_expiry,
            note,
            payment_date,
            billing_date,
            period_start,
            period_end,
            payment_date_str,
            billing_date_str,
            period_start_str,
            period_end_str,
            billing_months,
            subtotal,
            discount,
            vat_amount,
            total_amount,
            payment_method,
            paid_at,
            created_at,
            last_printed_at,
            items,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct ReportIndex {
    pub receipts: BTreeMap<i32, Arc<ProjectedReceipt>>,
    pub by_payment_date: BTreeMap<NaiveDate, Vec<i32>>,
    pub by_billing_date: BTreeMap<NaiveDate, Vec<i32>>,
    pub by_period_end: BTreeMap<NaiveDate, Vec<i32>>,
    pub by_fiscal_year: HashMap<Box<str>, Vec<i32>>,
    pub by_branch_lower: HashMap<Box<str>, Vec<i32>>,
    pub by_fee_head_id: HashMap<Box<str>, Vec<i32>>,
    pub by_vehicle_norm: HashMap<Box<str>, Vec<i32>>,
    pub fee_heads_by_fy: HashMap<Box<str>, Vec<FeeHeadDef>>,
    pub all_fee_heads: Vec<FeeHeadDef>,
    pub category_by_vehicle_type: HashMap<Box<str>, String>,
    pub categories_ordered: Vec<String>,
    pub branches_by_id: HashMap<Box<str>, BranchDef>,
    pub branches_by_name_lower: HashMap<Box<str>, BranchDef>,
    pub users: Vec<UserDef>,
    pub all_fiscal_years: Vec<String>,
}

fn push_vec_idx(map: &mut HashMap<Box<str>, Vec<i32>>, key: &str, id: i32) {
    if key.is_empty() {
        return;
    }
    if let Some(v) = map.get_mut(key) {
        if !v.contains(&id) {
            v.push(id);
        }
    } else {
        map.insert(key.into(), vec![id]);
    }
}

fn remove_vec_idx(map: &mut HashMap<Box<str>, Vec<i32>>, key: &str, id: i32) {
    if let Some(v) = map.get_mut(key) {
        v.retain(|&x| x != id);
    }
}

fn push_date_idx(map: &mut BTreeMap<NaiveDate, Vec<i32>>, date: Option<NaiveDate>, id: i32) {
    if let Some(d) = date {
        let v = map.entry(d).or_default();
        if !v.contains(&id) {
            v.push(id);
        }
    }
}

fn remove_date_idx(map: &mut BTreeMap<NaiveDate, Vec<i32>>, date: Option<NaiveDate>, id: i32) {
    if let Some(d) = date {
        if let Some(v) = map.get_mut(&d) {
            v.retain(|&x| x != id);
        }
    }
}

impl ReportIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert_bill(&mut self, res: &DatabaseResource, fy_config: &mut FiscalYearConfig) {
        let Some(projected) = ProjectedReceipt::from_bill(res, fy_config) else {
            if let Some(id) = res.id {
                self.remove_bill(id);
            }
            return;
        };
        let id = projected.receipt_id;
        self.remove_bill(id);

        let arc = Arc::new(projected);
        push_date_idx(&mut self.by_payment_date, arc.payment_date, id);
        push_date_idx(&mut self.by_billing_date, arc.billing_date, id);
        push_date_idx(&mut self.by_period_end, arc.period_end, id);
        push_vec_idx(&mut self.by_fiscal_year, &arc.fiscal_year_canonical, id);
        push_vec_idx(&mut self.by_branch_lower, &arc.branch_name_lower, id);
        push_vec_idx(&mut self.by_vehicle_norm, &arc.vehicle_number_norm, id);
        if !arc.vehicle_id.is_empty() {
            push_vec_idx(
                &mut self.by_vehicle_norm,
                &arc.vehicle_id.to_ascii_lowercase(),
                id,
            );
        }
        for item in &arc.items {
            if !item.fee_head_id.is_empty() {
                push_vec_idx(&mut self.by_fee_head_id, &item.fee_head_id, id);
            }
        }
        self.receipts.insert(id, arc);
    }

    pub fn remove_bill(&mut self, bill_id: i32) {
        let Some(old) = self.receipts.remove(&bill_id) else {
            return;
        };
        remove_date_idx(&mut self.by_payment_date, old.payment_date, bill_id);
        remove_date_idx(&mut self.by_billing_date, old.billing_date, bill_id);
        remove_date_idx(&mut self.by_period_end, old.period_end, bill_id);
        remove_vec_idx(&mut self.by_fiscal_year, &old.fiscal_year_canonical, bill_id);
        remove_vec_idx(&mut self.by_branch_lower, &old.branch_name_lower, bill_id);
        remove_vec_idx(&mut self.by_vehicle_norm, &old.vehicle_number_norm, bill_id);
        if !old.vehicle_id.is_empty() {
            remove_vec_idx(
                &mut self.by_vehicle_norm,
                &old.vehicle_id.to_ascii_lowercase(),
                bill_id,
            );
        }
        for item in &old.items {
            if !item.fee_head_id.is_empty() {
                remove_vec_idx(&mut self.by_fee_head_id, &item.fee_head_id, bill_id);
            }
        }
    }

    pub fn rebuild_dimensions<'a>(
        &mut self,
        branches: impl IntoIterator<Item = &'a DatabaseResource>,
        users: impl IntoIterator<Item = &'a DatabaseResource>,
        billing_rules: impl IntoIterator<Item = &'a DatabaseResource>,
        categories: impl IntoIterator<Item = &'a DatabaseResource>,
        fy_config: &FiscalYearConfig,
    ) {
        self.branches_by_id.clear();
        self.branches_by_name_lower.clear();
        self.users.clear();
        self.fee_heads_by_fy.clear();
        self.all_fee_heads.clear();
        self.category_by_vehicle_type.clear();
        self.categories_ordered.clear();

        for b in branches {
            let Some(inner) = b.inner_content.as_ref() else {
                continue;
            };
            let bid = b
                .id
                .map(|x| x.to_string())
                .or_else(|| extract_string_or_int(inner.get("id")))
                .unwrap_or_default();
            let name = s_opt(inner.get("name").or_else(|| inner.get("branchName")));
            if name.is_empty() {
                continue;
            }
            let code = s_opt(
                inner
                    .get("BranchCode")
                    .or_else(|| inner.get("branchCode"))
                    .or_else(|| inner.get("code")),
            );
            let def = BranchDef {
                id: bid.clone(),
                name_lower: normalize_key(&name),
                name,
                code,
            };
            if !def.id.is_empty() {
                self.branches_by_id.insert(def.id.as_str().into(), def.clone());
            }
            self.branches_by_name_lower
                .insert(def.name_lower.as_str().into(), def);
        }

        for u in users {
            let Some(inner) = u.inner_content.as_ref() else {
                continue;
            };
            let uid = u
                .id
                .map(|x| x.to_string())
                .or_else(|| extract_string_or_int(inner.get("id")))
                .unwrap_or_default();
            let name = s_opt(inner.get("name"));
            let email = s_opt(inner.get("email").or_else(|| inner.get("username")));
            let branch_id = extract_string_or_int(inner.get("branchId"))
                .or_else(|| {
                    inner
                        .get("branch")
                        .and_then(|b| extract_string_or_int(b.get("id")))
                })
                .unwrap_or_default();
            let mut branch_name = s_opt(inner.get("branchName")).if_empty_then(|| {
                inner
                    .get("branch")
                    .map(|b| s_opt(b.get("name")))
                    .unwrap_or_default()
            });
            if branch_name.is_empty() && !branch_id.is_empty() {
                if let Some(bdef) = self.branches_by_id.get(branch_id.as_str()) {
                    branch_name = bdef.name.clone();
                }
            }
            self.users.push(UserDef {
                id: uid,
                name,
                email,
                branch_id,
                branch_name,
            });
        }

        let mut seen_fh_global: HashSet<(String, String)> = HashSet::new();
        for rule in billing_rules {
            let Some(inner) = rule.inner_content.as_ref() else {
                continue;
            };
            let rtype = s_opt(inner.get("resourceType"));
            let fh_id = extract_string_or_int(inner.get("feeHeadId"))
                .or_else(|| {
                    if rtype.eq_ignore_ascii_case("FEE_HEAD") {
                        rule.id
                            .map(|x| x.to_string())
                            .or_else(|| extract_string_or_int(inner.get("id")))
                    } else {
                        None
                    }
                })
                .unwrap_or_default();
            let fh_name = s_opt(inner.get("feeHead"))
                .if_empty_then(|| s_opt(inner.get("title")))
                .if_empty_then(|| s_opt(inner.get("name")));
            if fh_id.is_empty() || fh_name.is_empty() {
                continue;
            }
            let display_order = i32_opt(
                inner
                    .get("displayOrder")
                    .or_else(|| inner.get("order"))
                    .or_else(|| inner.get("sortOrder")),
            );
            let display_order = if display_order == 0 {
                fh_id.parse::<i32>().unwrap_or(rule.id.unwrap_or(9999))
            } else {
                display_order
            };
            let raw_fy = extract_string_or_int(inner.get("fiscalYear"))
                .or_else(|| extract_string_or_int(inner.get("fiscalYearId")))
                .unwrap_or_default();
            let fy_canon = if raw_fy.is_empty() {
                String::new()
            } else {
                canonical_fiscal_year_short(&fy_config.display_for(&raw_fy))
            };
            if !seen_fh_global.insert((fy_canon.clone(), fh_id.clone())) {
                continue;
            }
            let def = FeeHeadDef {
                id: fh_id,
                name: fh_name,
                display_order,
                fiscal_year_canonical: fy_canon.clone(),
            };
            if !fy_canon.is_empty() {
                self.fee_heads_by_fy
                    .entry(fy_canon.into_boxed_str())
                    .or_default()
                    .push(def.clone());
            }
            self.all_fee_heads.push(def);
        }

        for list in self.fee_heads_by_fy.values_mut() {
            list.sort_by(|a, b| {
                a.display_order
                    .cmp(&b.display_order)
                    .then_with(|| a.name.cmp(&b.name))
            });
        }
        self.all_fee_heads.sort_by(|a, b| {
            a.display_order
                .cmp(&b.display_order)
                .then_with(|| a.name.cmp(&b.name))
        });

        let mut seen_cats = HashSet::new();
        for cat in categories {
            let Some(inner) = cat.inner_content.as_ref() else {
                continue;
            };
            let cname = s_opt(
                inner
                    .get("category_name")
                    .or_else(|| inner.get("category"))
                    .or_else(|| inner.get("name")),
            );
            if cname.is_empty() {
                continue;
            }
            if seen_cats.insert(cname.clone()) {
                self.categories_ordered.push(cname.clone());
            }
            if let Some(arr) = inner
                .get("vehicle_types")
                .or_else(|| inner.get("vehicleTypes"))
                .and_then(Value::as_array)
            {
                for vt in arr {
                    let vt_str = match vt {
                        Value::String(s) => s.trim().to_string(),
                        Value::Object(map) => s_opt(map.get("name").or_else(|| map.get("title"))),
                        _ => String::new(),
                    };
                    if !vt_str.is_empty() {
                        let k = normalize_key(&vt_str);
                        if !k.is_empty() {
                            self.category_by_vehicle_type
                                .insert(k.into_boxed_str(), cname.clone());
                        }
                    }
                }
            }
        }

        self.refresh_fiscal_years(fy_config);
    }

    pub fn refresh_fiscal_years(&mut self, fy_config: &FiscalYearConfig) {
        let mut fys: Vec<String> = Vec::new();
        for k in self.by_fiscal_year.keys() {
            if !k.is_empty() && !fys.iter().any(|x| x == k.as_ref()) {
                fys.push(k.to_string());
            }
        }
        for k in self.fee_heads_by_fy.keys() {
            if !k.is_empty() && !fys.iter().any(|x| x == k.as_ref()) {
                fys.push(k.to_string());
            }
        }
        let default_fy = canonical_fiscal_year_short(&fy_config.default_or_fallback());
        if !default_fy.is_empty() && !fys.iter().any(|x| x == &default_fy) {
            fys.push(default_fy);
        }
        fys.sort_by(|a, b| b.cmp(a));
        self.all_fiscal_years = fys;
    }

    pub fn resolve_category(&self, vehicle_type: &str) -> String {
        let norm = normalize_key(vehicle_type);
        if norm.is_empty() {
            return "वर्ग उपलब्ध छैन".to_string();
        }
        self.category_by_vehicle_type
            .get(norm.as_str())
            .cloned()
            .unwrap_or_else(|| "वर्गीकृत नभएको".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use narayani_core::ResourceName;
    use serde_json::json;

    #[test]
    fn projects_live_db_null_status_and_lodbod_null_billing_type() {
        let mut fy = FiscalYearConfig::from_settings(&[]);
        let lodbod_bill = DatabaseResource {
            id: Some(259),
            resource_name: Some(ResourceName::BillResource),
            inner_content: Some(json!({
                "paidAt": "2026-11-27T07:14:44.212Z",
                "status": null,
                "billingType": null,
                "lodbodPaymentReferenceId": "LB-99",
                "branch": { "id": "1", "name": "Main Branch", "BranchCode": "MAIN" },
                "vehicle": { "id": "24631", "vehicleNumber": "Province-0-55-744-CA-8536" },
                "fiscalYear": "2082/2083",
                "total": 2000,
                "items": [{ "feeHeadId": "116", "title": "सदस्यता शुल्क", "amount": 2000 }]
            })),
            external_content: None,
        };
        let proj = ProjectedReceipt::from_bill(&lodbod_bill, &mut fy).unwrap();
        assert_eq!(proj.status, "PAID");
        assert!(proj.is_paid);
        assert_eq!(proj.billing_type, "MEMBERSHIP");
        assert_eq!(proj.receipt_number, "MAIN.259");
        assert_eq!(proj.fiscal_year_canonical, "2082/83");
    }

    #[test]
    fn resolves_live_category_shapes_and_user_branch_join() {
        let cat_old = DatabaseResource {
            id: Some(1),
            resource_name: Some(ResourceName::VehicleCategoryResource),
            inner_content: Some(json!({ "category_name": "बस", "vehicle_types": ["Bus"] })),
            external_content: None,
        };
        let cat_new = DatabaseResource {
            id: Some(10),
            resource_name: Some(ResourceName::VehicleCategoryResource),
            inner_content: Some(json!({ "category": "ट्रक", "vehicle_types": ["10 pangree"] })),
            external_content: None,
        };
        let branch = DatabaseResource {
            id: Some(3),
            resource_name: Some(ResourceName::BranchResource),
            inner_content: Some(json!({ "id": "3", "name": "चितवन", "BranchCode": "CIT" })),
            external_content: None,
        };
        let user = DatabaseResource {
            id: Some(5),
            resource_name: Some(ResourceName::UserResource),
            inner_content: Some(json!({ "name": "Staff", "email": "s@test.com", "branchId": "3", "branchName": null })),
            external_content: None,
        };
        let fy = FiscalYearConfig::from_settings(&[]);
        let mut idx = ReportIndex::new();
        idx.rebuild_dimensions(
            [&branch],
            [&user],
            std::iter::empty(),
            [&cat_old, &cat_new],
            &fy,
        );
        assert_eq!(idx.resolve_category("Bus"), "बस");
        assert_eq!(idx.resolve_category("10 pangree"), "ट्रक");
        assert_eq!(idx.resolve_category(""), "वर्ग उपलब्ध छैन");
        assert_eq!(idx.resolve_category("UnknownType"), "वर्गीकृत नभएको");
        assert_eq!(idx.users[0].branch_name, "चितवन");
    }
}
