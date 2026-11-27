use super::common::{paginate, round2, PaginationMeta};
use super::rbac::CallerBranchContext;
use narayani_core::{
    extract_string_or_int, normalize_key, normalize_vehicle_number, normalize_vehicle_search,
    DatabaseResource,
};
use narayani_storage::{ProjectedReceipt, ReportIndex};
use narayani_vehicle_history::VehicleAliasMap;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MemberSearchQuery {
    pub q: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberSearchCandidate {
    pub id: String,
    pub vehicle_number: String,
    pub member_number: String,
    pub owner_name: String,
    pub phone: String,
    pub address: String,
    pub vehicle_type: String,
    pub branch_name: String,
    pub transaction_count: usize,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MemberTransactionsQuery {
    pub page: Option<usize>,
    pub page_size: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberComplianceDto {
    pub policy_number: String,
    pub insurance: String,
    pub check_pass: String,
    pub bluebook: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberProfileDto {
    pub vehicle_number: String,
    pub vehicle_id: String,
    pub member_number: String,
    pub billing_date: String,
    pub period_start: String,
    pub expiry_date: String,
    pub sawari_dhani_name: String,
    pub sanchalak_name: String,
    pub phone: String,
    pub address: String,
    pub vehicle_type: String,
    pub category: String,
    pub branch_name: String,
    pub last_printed_at: Option<String>,
    pub compliance: MemberComplianceDto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberTransactionRow {
    pub receipt_id: String,
    pub receipt_number: String,
    pub payment_date: String,
    pub billing_date: String,
    pub period_start: String,
    pub period_end: String,
    pub billing_months: i32,
    pub payment_method: String,
    pub branch_name: String,
    pub user_name: String,
    pub fiscal_year: String,
    pub status: String,
    pub note: String,
    pub items_sum: f64,
    pub extra_amount: f64,
    pub total_amount: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberTransactionsResponse {
    pub profile: MemberProfileDto,
    pub total_paid: f64,
    pub total_extra: f64,
    pub rows: Vec<MemberTransactionRow>,
    pub pagination: PaginationMeta,
}

fn s_opt(v: Option<&Value>) -> String {
    v.and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("")
        .to_string()
}

fn bs_preferred_date(obj: Option<&Value>, keys: &[&str]) -> String {
    let Some(v) = obj else {
        return String::new();
    };
    for k in keys {
        if let Some(s) = v.get(*k).and_then(Value::as_str) {
            let t = s.trim();
            if !t.is_empty() {
                return t.to_string();
            }
        }
    }
    String::new()
}

fn extract_compliance(vehicle_inner: Option<&Value>) -> MemberComplianceDto {
    let Some(v) = vehicle_inner else {
        return MemberComplianceDto {
            policy_number: String::new(),
            insurance: String::new(),
            check_pass: String::new(),
            bluebook: String::new(),
        };
    };
    let comp = v.get("Compliance").or_else(|| v.get("compliance"));
    let ins = comp.and_then(|c| c.get("Insurance").or_else(|| c.get("insurance")));
    let cp = comp.and_then(|c| c.get("CheckPass").or_else(|| c.get("checkPass")));
    let bb = comp.and_then(|c| c.get("Bluebook").or_else(|| c.get("bluebook")));

    MemberComplianceDto {
        policy_number: bs_preferred_date(ins, &["PolicyNumber", "policyNumber"]),
        insurance: bs_preferred_date(
            ins,
            &["ExpiryBs", "expiryBs", "ExpiryAd", "expiryAd", "expiryDate"],
        ),
        check_pass: bs_preferred_date(
            cp,
            &["ExpiryBs", "expiryBs", "ExpiryAd", "expiryAd", "expiryDate"],
        ),
        bluebook: bs_preferred_date(
            bb,
            &["ExpiryBs", "expiryBs", "ExpiryAd", "expiryAd", "expiryDate"],
        ),
    }
}

fn extract_member_number_and_dates(
    m: &DatabaseResource,
) -> (Option<String>, Option<String>, String, String, String, String) {
    let Some(inner) = m.inner_content.as_ref() else {
        return (None, None, String::new(), String::new(), String::new(), String::new());
    };
    let vid = extract_string_or_int(inner.get("vehicleId"))
        .or_else(|| extract_string_or_int(inner.get("VehicleId")));
    let vnum = s_opt(
        inner
            .get("vehicleNumber")
            .or_else(|| inner.get("VehicleNumber")),
    );
    let norm = if vnum.is_empty() {
        None
    } else {
        normalize_vehicle_number(&vnum)
    };
    let member_no = extract_string_or_int(inner.get("memberNumber"))
        .or_else(|| extract_string_or_int(inner.get("memberNo")))
        .or_else(|| extract_string_or_int(inner.get("membershipNumber")))
        .unwrap_or_default();
    let bdate = s_opt(inner.get("billingDate"));
    let pstart = s_opt(inner.get("periodStart"));
    let exp = s_opt(
        inner
            .get("expiryDate")
            .or_else(|| inner.get("periodEnd"))
            .or_else(|| inner.get("validUntil")),
    );
    (vid, norm, member_no, bdate, pstart, exp)
}

pub fn search_member_transactions(
    query: &MemberSearchQuery,
    caller: &CallerBranchContext,
    index: &ReportIndex,
    aliases: &VehicleAliasMap,
    members: &[&DatabaseResource],
    vehicles: &[&DatabaseResource],
) -> Vec<MemberSearchCandidate> {
    let q_raw = query.q.as_deref().map(str::trim).unwrap_or("");
    if q_raw.is_empty() {
        return Vec::new();
    }
    let q_key = normalize_key(q_raw);
    let q_plate = normalize_vehicle_search(q_raw);

    struct GroupAcc<'a> {
        vehicle_key: String,
        vehicle_number: String,
        vehicle_id: String,
        member_number: String,
        owner_name: String,
        phone: String,
        address: String,
        vehicle_type: String,
        branch_name: String,
        branch_lower: String,
        branch_id: String,
        receipts: Vec<&'a ProjectedReceipt>,
    }

    let mut member_by_norm: HashMap<String, String> = HashMap::new();
    let mut member_by_vid: HashMap<String, String> = HashMap::new();
    for m in members {
        let (vid, vnorm, mno, _, _, _) = extract_member_number_and_dates(m);
        if mno.is_empty() {
            continue;
        }
        if let Some(n) = vnorm {
            member_by_norm.insert(n, mno.clone());
        }
        if let Some(id) = vid {
            member_by_vid.insert(id.to_ascii_lowercase(), mno);
        }
    }

    let mut groups: HashMap<String, GroupAcc<'_>> = HashMap::new();
    for r in index.receipts.values() {
        if r.billing_type.eq_ignore_ascii_case("GENERAL") {
            continue;
        }
        if !caller.matches_receipt(r, None) {
            continue;
        }
        let resolved = aliases.resolve(Some(&r.vehicle_id), Some(&r.vehicle_number));
        let (gkey, vnum, vid, vtype, owner) = if let Some(res) = resolved {
            let t = res.target;
            let k = if !t.norm_vehicle_number.is_empty() {
                t.norm_vehicle_number.clone()
            } else if !t.vehicle_id.is_empty() {
                format!("vid:{}", t.vehicle_id.to_ascii_lowercase())
            } else {
                r.vehicle_number_norm.clone()
            };
            (
                k,
                if !t.vehicle_number.is_empty() {
                    t.vehicle_number.clone()
                } else {
                    r.vehicle_number.clone()
                },
                if !t.vehicle_id.is_empty() {
                    t.vehicle_id.clone()
                } else {
                    r.vehicle_id.clone()
                },
                if !r.vehicle_type.is_empty() {
                    r.vehicle_type.clone()
                } else {
                    t.vehicle_type.clone()
                },
                if !r.customer_name.is_empty() {
                    r.customer_name.clone()
                } else {
                    t.owner_name.clone()
                },
            )
        } else {
            let k = if !r.vehicle_number_norm.is_empty() {
                r.vehicle_number_norm.clone()
            } else if !r.vehicle_id.is_empty() {
                format!("vid:{}", r.vehicle_id.to_ascii_lowercase())
            } else {
                format!("rcp:{}", r.receipt_id)
            };
            (
                k,
                r.vehicle_number.clone(),
                r.vehicle_id.clone(),
                r.vehicle_type.clone(),
                r.customer_name.clone(),
            )
        };

        let mno = normalize_vehicle_number(&vnum)
            .and_then(|n| member_by_norm.get(&n).cloned())
            .or_else(|| member_by_vid.get(&vid.to_ascii_lowercase()).cloned())
            .unwrap_or_default();

        let entry = groups.entry(gkey.clone()).or_insert_with(|| GroupAcc {
            vehicle_key: gkey,
            vehicle_number: vnum,
            vehicle_id: vid,
            member_number: mno,
            owner_name: owner,
            phone: r.customer_phone.clone(),
            address: r.address.clone(),
            vehicle_type: vtype,
            branch_name: r.branch_name.clone(),
            branch_lower: r.branch_name_lower.clone(),
            branch_id: r.branch_id.clone(),
            receipts: Vec::new(),
        });
        entry.receipts.push(r.as_ref());
    }

    // Also include VehicleResource entries not yet represented in groups if query matches them
    for v in vehicles {
        let Some(inner) = v.inner_content.as_ref() else {
            continue;
        };
        let vnum = s_opt(
            inner
                .get("VehicleNumber")
                .or_else(|| inner.get("vehicleNumber")),
        );
        let norm = normalize_vehicle_number(&vnum).unwrap_or_default();
        let vid = extract_string_or_int(inner.get("VehicleId"))
            .or_else(|| extract_string_or_int(inner.get("vehicleId")))
            .unwrap_or_default();
        let gkey = if !norm.is_empty() {
            norm.clone()
        } else if !vid.is_empty() {
            format!("vid:{}", vid.to_ascii_lowercase())
        } else {
            continue;
        };
        if groups.contains_key(&gkey) {
            continue;
        }
        let branch = s_opt(
            inner
                .get("SanghBranch")
                .or_else(|| inner.get("sanghBranch")),
        );
        let branch_lower = normalize_key(&branch);
        if !caller.is_admin {
            if let Some(ref assigned) = caller.assigned_branch_lower {
                if !branch_lower.is_empty() && branch_lower != *assigned {
                    continue;
                }
            }
        }
        let owner = inner
            .get("Owners")
            .and_then(Value::as_array)
            .and_then(|arr| arr.first())
            .map(|o| s_opt(o.get("Name").or_else(|| o.get("name"))))
            .unwrap_or_else(|| s_opt(inner.get("OwnerName")));
        let phone = inner
            .get("Owners")
            .and_then(Value::as_array)
            .and_then(|arr| arr.first())
            .map(|o| s_opt(o.get("Phone").or_else(|| o.get("phone"))))
            .unwrap_or_else(|| s_opt(inner.get("Phone")));
        let address = s_opt(inner.get("Address").or_else(|| inner.get("address")));
        let vtype = s_opt(inner.get("VehicleType").or_else(|| inner.get("vehicleType")));
        let mno = member_by_norm
            .get(&norm)
            .cloned()
            .or_else(|| member_by_vid.get(&vid.to_ascii_lowercase()).cloned())
            .unwrap_or_default();
        groups.insert(
            gkey.clone(),
            GroupAcc {
                vehicle_key: gkey,
                vehicle_number: vnum,
                vehicle_id: vid,
                member_number: mno,
                owner_name: owner,
                phone,
                address,
                vehicle_type: vtype,
                branch_name: branch,
                branch_lower,
                branch_id: String::new(),
                receipts: Vec::new(),
            },
        );
    }

    let mut out = Vec::new();
    for g in groups.into_values() {
        let _ = (&g.branch_lower, &g.branch_id, &g.vehicle_id);
        let field_match = (!q_plate.is_empty()
            && normalize_vehicle_search(&g.vehicle_number).contains(&q_plate))
            || normalize_key(&g.vehicle_number).contains(&q_key)
            || normalize_key(&g.member_number).contains(&q_key)
            || normalize_key(&g.owner_name).contains(&q_key)
            || normalize_key(&g.phone).contains(&q_key)
            || normalize_key(&g.address).contains(&q_key)
            || normalize_key(&g.vehicle_type).contains(&q_key)
            || normalize_key(&g.branch_name).contains(&q_key)
            || g.receipts.iter().any(|r| {
                normalize_key(&r.receipt_number).contains(&q_key)
                    || normalize_key(&r.branch_name).contains(&q_key)
                    || normalize_key(&r.payment_method).contains(&q_key)
                    || normalize_key(&r.customer_name).contains(&q_key)
                    || r.items
                        .iter()
                        .any(|i| normalize_key(&i.fee_title).contains(&q_key))
            });
        if field_match {
            out.push(MemberSearchCandidate {
                id: if !g.vehicle_number.is_empty() {
                    g.vehicle_number.clone()
                } else {
                    g.vehicle_key
                },
                vehicle_number: g.vehicle_number,
                member_number: g.member_number,
                owner_name: g.owner_name,
                phone: g.phone,
                address: g.address,
                vehicle_type: g.vehicle_type,
                branch_name: g.branch_name,
                transaction_count: g.receipts.len(),
            });
        }
    }

    out.sort_by(|a, b| {
        b.transaction_count
            .cmp(&a.transaction_count)
            .then_with(|| a.vehicle_number.cmp(&b.vehicle_number))
    });
    out.truncate(50);
    out
}

pub fn compute_member_transactions(
    vehicle_key: &str,
    query: &MemberTransactionsQuery,
    caller: &CallerBranchContext,
    index: &ReportIndex,
    aliases: &VehicleAliasMap,
    members: &[&DatabaseResource],
    vehicles: &[&DatabaseResource],
) -> MemberTransactionsResponse {
    let page = query.page.unwrap_or(1);
    let page_size = query.page_size.unwrap_or(100).clamp(1, 200);
    let vk_trimmed = vehicle_key.trim();
    let vk_norm = normalize_vehicle_number(vk_trimmed).unwrap_or_default();

    let resolved = aliases.resolve(Some(vk_trimmed), Some(vk_trimmed));
    let mut match_ids: HashSet<String> = HashSet::new();
    let mut match_norms: HashSet<String> = HashSet::new();
    if !vk_trimmed.is_empty() {
        match_ids.insert(vk_trimmed.to_ascii_lowercase());
    }
    if !vk_norm.is_empty() {
        match_norms.insert(vk_norm.clone());
    }
    let mut resolved_vid = String::new();
    let mut resolved_vnum = vk_trimmed.to_string();
    let mut resolved_vtype = String::new();
    let mut resolved_owner = String::new();
    let mut resolved_branch = String::new();
    let mut matched_vehicle_db_id = None;

    if let Some(res) = resolved {
        let t = res.target;
        matched_vehicle_db_id = t.db_id;
        if !t.vehicle_id.is_empty() {
            resolved_vid = t.vehicle_id.clone();
            match_ids.insert(t.vehicle_id.to_ascii_lowercase());
        }
        for pid in &t.previous_vehicle_ids {
            match_ids.insert(pid.trim().to_ascii_lowercase());
        }
        if !t.vehicle_number.is_empty() {
            resolved_vnum = t.vehicle_number.clone();
        }
        if !t.norm_vehicle_number.is_empty() {
            match_norms.insert(t.norm_vehicle_number.clone());
        }
        for pnum in &t.previous_vehicle_numbers {
            if let Some(n) = normalize_vehicle_number(pnum) {
                match_norms.insert(n);
            }
        }
        resolved_vtype = t.vehicle_type.clone();
        resolved_owner = t.owner_name.clone();
        resolved_branch = t.sangh_branch.clone();
    }

    let matched_vehicle_inner = vehicles
        .iter()
        .find(|v| {
            if matched_vehicle_db_id.is_some_and(|did| v.id == Some(did)) {
                return true;
            }
            v.inner_content.as_ref().is_some_and(|inner| {
                let vid = extract_string_or_int(inner.get("VehicleId"))
                    .or_else(|| extract_string_or_int(inner.get("vehicleId")))
                    .map(|s| s.to_ascii_lowercase());
                let norm = s_opt(
                    inner
                        .get("VehicleNumber")
                        .or_else(|| inner.get("vehicleNumber")),
                );
                let pnorm = normalize_vehicle_number(&norm).unwrap_or_default();
                vid.is_some_and(|id| match_ids.contains(&id))
                    || (!pnorm.is_empty() && match_norms.contains(&pnorm))
            })
        })
        .and_then(|v| v.inner_content.as_ref());

    let mut matched_receipts: Vec<&ProjectedReceipt> = index
        .receipts
        .values()
        .filter(|r| {
            if r.billing_type.eq_ignore_ascii_case("GENERAL") {
                return false;
            }
            if !caller.matches_receipt(r, None) {
                return false;
            }
            let id_hit = !r.vehicle_id.is_empty()
                && match_ids.contains(&r.vehicle_id.to_ascii_lowercase());
            let norm_hit =
                !r.vehicle_number_norm.is_empty() && match_norms.contains(&r.vehicle_number_norm);
            id_hit || norm_hit
        })
        .map(|arc| arc.as_ref())
        .collect();

    matched_receipts.sort_by(|a, b| {
        a.payment_date
            .cmp(&b.payment_date)
            .then_with(|| a.receipt_id.cmp(&b.receipt_id))
    });

    let mut member_number = String::new();
    let mut m_billing_date = String::new();
    let mut m_period_start = String::new();
    let mut m_expiry_date = String::new();
    for m in members {
        let (mvid, mvnorm, mno, mbdate, mpstart, mexp) = extract_member_number_and_dates(m);
        let id_hit = mvid.is_some_and(|id| match_ids.contains(&id.to_ascii_lowercase()));
        let norm_hit = mvnorm.is_some_and(|n| match_norms.contains(&n));
        if id_hit || norm_hit || (!mno.is_empty() && mno == vk_trimmed) {
            member_number = mno;
            m_billing_date = mbdate;
            m_period_start = mpstart;
            m_expiry_date = mexp;
            break;
        }
    }

    let mut total_paid = 0.0;
    let mut total_extra = 0.0;
    let mut last_printed_at: Option<String> = None;
    let mut last_billing_date = m_billing_date;
    let mut last_period_start = m_period_start;
    let mut last_period_end = m_expiry_date;
    let mut latest_operator = String::new();
    let mut latest_phone = String::new();
    let mut latest_address = String::new();

    let all_rows: Vec<MemberTransactionRow> = matched_receipts
        .iter()
        .map(|r| {
            let items_sum: f64 = r.items.iter().map(|i| i.amount).sum();
            let extra = (r.total_amount - items_sum).max(0.0);
            if r.is_paid {
                total_paid += r.total_amount;
                total_extra += extra;
            }
            if r.last_printed_at.is_some() {
                last_printed_at = r.last_printed_at.clone();
            }
            if !r.billing_date_str.is_empty() {
                last_billing_date = r.billing_date_str.clone();
            }
            if !r.period_start_str.is_empty() {
                last_period_start = r.period_start_str.clone();
            }
            if !r.period_end_str.is_empty() {
                last_period_end = r.period_end_str.clone();
            }
            if resolved_owner.is_empty() && !r.customer_name.is_empty() {
                resolved_owner = r.customer_name.clone();
            }
            if latest_operator.is_empty() && !r.operator_name.is_empty() {
                latest_operator = r.operator_name.clone();
            }
            if latest_phone.is_empty() && !r.customer_phone.is_empty() {
                latest_phone = r.customer_phone.clone();
            }
            if latest_address.is_empty() && !r.address.is_empty() {
                latest_address = r.address.clone();
            }
            if resolved_vtype.is_empty() && !r.vehicle_type.is_empty() {
                resolved_vtype = r.vehicle_type.clone();
            }
            if resolved_branch.is_empty() && !r.branch_name.is_empty() {
                resolved_branch = r.branch_name.clone();
            }
            if resolved_vid.is_empty() && !r.vehicle_id.is_empty() {
                resolved_vid = r.vehicle_id.clone();
            }
            let user_name = if !r.created_by_name.is_empty() {
                r.created_by_name.clone()
            } else {
                r.created_by_username.clone()
            };
            MemberTransactionRow {
                receipt_id: r.receipt_id.to_string(),
                receipt_number: r.receipt_number.clone(),
                payment_date: r.payment_date_str.clone(),
                billing_date: r.billing_date_str.clone(),
                period_start: r.period_start_str.clone(),
                period_end: r.period_end_str.clone(),
                billing_months: r.billing_months,
                payment_method: r.payment_method.clone(),
                branch_name: r.branch_name.clone(),
                user_name,
                fiscal_year: r.fiscal_year_canonical.clone(),
                status: r.status.clone(),
                note: r.note.clone(),
                items_sum: round2(items_sum),
                extra_amount: round2(extra),
                total_amount: if r.is_paid { round2(r.total_amount) } else { 0.0 },
            }
        })
        .collect();

    if let Some(vi) = matched_vehicle_inner {
        if latest_address.is_empty() {
            latest_address = s_opt(vi.get("Address").or_else(|| vi.get("address")));
        }
        if let Some(owners) = vi.get("Owners").and_then(Value::as_array) {
            if let Some(first) = owners.first() {
                if resolved_owner.is_empty() {
                    resolved_owner = s_opt(first.get("Name").or_else(|| first.get("name")));
                }
                if latest_phone.is_empty() {
                    latest_phone = s_opt(first.get("Phone").or_else(|| first.get("phone")));
                }
            }
            if latest_operator.is_empty() && owners.len() > 1 {
                latest_operator = s_opt(owners[1].get("Name").or_else(|| owners[1].get("name")));
            }
        }
    }

    let category = index.resolve_category(&resolved_vtype);
    let compliance = extract_compliance(matched_vehicle_inner);
    let (rows, pagination) = paginate(&all_rows, page, page_size, 200);

    MemberTransactionsResponse {
        profile: MemberProfileDto {
            vehicle_number: resolved_vnum,
            vehicle_id: resolved_vid,
            member_number,
            billing_date: last_billing_date,
            period_start: last_period_start,
            expiry_date: last_period_end,
            sawari_dhani_name: resolved_owner,
            sanchalak_name: latest_operator,
            phone: latest_phone,
            address: latest_address,
            vehicle_type: resolved_vtype,
            category,
            branch_name: resolved_branch,
            last_printed_at,
            compliance,
        },
        total_paid: round2(total_paid),
        total_extra: round2(total_extra),
        rows,
        pagination,
    }
}

pub fn json_empty_member_search() -> Value {
    json!([])
}
