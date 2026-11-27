use super::rbac::CallerBranchContext;
use narayani_core::{extract_string_or_int, normalize_key, DatabaseResource};
use narayani_date::canonical_fiscal_year_short;
use narayani_settings::AppSettings;
use narayani_storage::{ProjectedReceipt, ReportIndex};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FilterOptionsQuery {
    pub fiscal_year: Option<String>,
    pub branch: Option<String>,
    pub report_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeeHeadOption {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VehicleTypeGroupDto {
    pub id: String,
    pub name: String,
    pub vehicle_types: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterOptionsResponse {
    pub fiscal_years: Vec<String>,
    pub branches: Vec<String>,
    pub users: Vec<String>,
    pub fee_heads: Vec<FeeHeadOption>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub categories: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vehicle_types: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vehicle_type_groups: Option<Vec<VehicleTypeGroupDto>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_vehicle_types: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default)]
pub struct UserFilterAliases {
    pub tokens: HashSet<String>,
    pub ids: HashSet<String>,
}

impl UserFilterAliases {
    pub fn matches(&self, r: &ProjectedReceipt) -> bool {
        if self.tokens.contains(&normalize_key(&r.created_by_name))
            || self.tokens.contains(&normalize_key(&r.created_by_username))
        {
            return true;
        }
        !r.created_by_id.is_empty() && self.ids.contains(&r.created_by_id)
    }
}

pub fn build_user_filter_aliases(
    requested_user: Option<&str>,
    index: &ReportIndex,
    extra_users: &[DatabaseResource],
) -> Option<UserFilterAliases> {
    let raw = requested_user.map(str::trim).filter(|s| !s.is_empty())?;
    if raw.eq_ignore_ascii_case("all") {
        return None;
    }
    let needle = normalize_key(raw);
    let mut out = UserFilterAliases::default();
    out.tokens.insert(needle.clone());

    for u in &index.users {
        let name_k = normalize_key(&u.name);
        let email_k = normalize_key(&u.email);
        if (!name_k.is_empty() && name_k == needle)
            || (!email_k.is_empty() && email_k == needle)
            || (!u.id.is_empty() && u.id == raw)
        {
            if !name_k.is_empty() {
                out.tokens.insert(name_k);
            }
            if !email_k.is_empty() {
                out.tokens.insert(email_k);
            }
            if !u.id.is_empty() {
                out.ids.insert(u.id.clone());
            }
        }
    }

    for u in extra_users {
        let Some(inner) = u.inner_content.as_ref() else {
            continue;
        };
        let uid = u
            .id
            .map(|x| x.to_string())
            .or_else(|| extract_string_or_int(inner.get("id")))
            .unwrap_or_default();
        let name = inner
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        let email = inner
            .get("email")
            .or_else(|| inner.get("username"))
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        let name_k = normalize_key(name);
        let email_k = normalize_key(email);
        if (!name_k.is_empty() && name_k == needle)
            || (!email_k.is_empty() && email_k == needle)
            || (!uid.is_empty() && uid == raw)
        {
            if !name_k.is_empty() {
                out.tokens.insert(name_k);
            }
            if !email_k.is_empty() {
                out.tokens.insert(email_k);
            }
            if !uid.is_empty() {
                out.ids.insert(uid);
            }
        }
    }

    Some(out)
}

pub fn compute_filter_options(
    query: &FilterOptionsQuery,
    caller: &CallerBranchContext,
    index: &ReportIndex,
    extra_users: &[DatabaseResource],
    vehicles: &[&DatabaseResource],
    settings: &AppSettings,
) -> FilterOptionsResponse {
    // 1. branches
    let branches: Vec<String> = if !caller.is_admin {
        caller.assigned_branch_name.iter().cloned().collect()
    } else {
        let mut seen_b: HashSet<String> = HashSet::new();
        let mut out_b: Vec<String> = Vec::new();
        for b in index.branches_by_id.values() {
            let trimmed = b.name.trim();
            if !trimmed.is_empty() && trimmed != "—" && seen_b.insert(b.name_lower.clone()) {
                out_b.push(trimmed.to_string());
            }
        }
        for r in index.receipts.values() {
            let trimmed = r.branch_name.trim();
            if !trimmed.is_empty()
                && trimmed != "—"
                && !r.branch_name_lower.is_empty()
                && seen_b.insert(r.branch_name_lower.clone())
            {
                out_b.push(trimmed.to_string());
            }
        }
        out_b.sort();
        out_b
    };

    // 2. feeHeads (scoped by fiscalYear)
    let fy_req = query
        .fiscal_year
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("all"))
        .map(canonical_fiscal_year_short);

    let mut fh_map: HashMap<String, String> = HashMap::new();
    let add_fh = |map: &mut HashMap<String, String>, id: &str, label: &str| {
        let tid = id.trim();
        let tlab = label.trim();
        let key = if !tid.is_empty() {
            tid.to_string()
        } else {
            normalize_key(tlab)
        };
        if key.is_empty() || tlab.is_empty() {
            return;
        }
        map.entry(key).or_insert_with(|| tlab.to_string());
    };

    if let Some(ref fy) = fy_req {
        if let Some(list) = index.fee_heads_by_fy.get(fy.as_str()) {
            for fh in list {
                add_fh(&mut fh_map, &fh.id, &fh.name);
            }
        }
        if let Some(receipt_ids) = index.by_fiscal_year.get(fy.as_str()) {
            for rid in receipt_ids {
                if let Some(r) = index.receipts.get(rid) {
                    for item in &r.items {
                        add_fh(&mut fh_map, &item.fee_head_id, &item.fee_title);
                    }
                }
            }
        }
    } else {
        for fh in &index.all_fee_heads {
            add_fh(&mut fh_map, &fh.id, &fh.name);
        }
        for r in index.receipts.values() {
            for item in &r.items {
                add_fh(&mut fh_map, &item.fee_head_id, &item.fee_title);
            }
        }
    }

    let mut fee_heads: Vec<FeeHeadOption> = fh_map
        .into_iter()
        .map(|(value, label)| FeeHeadOption { value, label })
        .collect();
    fee_heads.sort_by(|a, b| a.label.cmp(&b.label).then_with(|| a.value.cmp(&b.value)));

    // 3. users (scoped by effective branch; deduplicated name vs email alias)
    let effective_branch_lower = caller.effective_branch_lower(query.branch.as_deref());
    let selected_branch_id: Option<String> = effective_branch_lower
        .as_deref()
        .and_then(|bl| index.branches_by_name_lower.get(bl).map(|b| b.id.clone()));

    let mut users_seen: HashSet<String> = HashSet::new();
    let mut email_aliases: HashSet<String> = HashSet::new();
    let mut users_out: Vec<String> = Vec::new();

    let push_user_record = |name: &str,
                            email: &str,
                            u_bid: &str,
                            u_bname_lower: &str,
                            users_seen: &mut HashSet<String>,
                            email_aliases: &mut HashSet<String>,
                            users_out: &mut Vec<String>| {
        if let Some(ref eff_lower) = effective_branch_lower {
            let bid_match = selected_branch_id
                .as_deref()
                .is_some_and(|sb| !sb.is_empty() && u_bid == sb);
            let bname_match = !u_bname_lower.is_empty() && u_bname_lower == eff_lower.as_str();
            if !bid_match && !bname_match {
                return;
            }
        }
        let n = name.trim();
        let e = email.trim();
        let display = if !n.is_empty() { n } else { e };
        if !display.is_empty() {
            let dk = normalize_key(display);
            if users_seen.insert(dk) {
                users_out.push(display.to_string());
            }
        }
        if !n.is_empty() && !e.is_empty() {
            email_aliases.insert(normalize_key(e));
        }
    };

    for u in extra_users {
        if let Some(inner) = u.inner_content.as_ref() {
            let name = inner.get("name").and_then(Value::as_str).unwrap_or("");
            let email = inner
                .get("email")
                .or_else(|| inner.get("username"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let bid = extract_string_or_int(inner.get("branchId"))
                .or_else(|| {
                    inner
                        .get("branch")
                        .and_then(|b| extract_string_or_int(b.get("id")))
                })
                .unwrap_or_default();
            let mut bname = inner
                .get("branchName")
                .or_else(|| inner.get("branch").and_then(|b| b.get("name")))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if bname.is_empty() && !bid.is_empty() {
                if let Some(bdef) = index.branches_by_id.get(bid.as_str()) {
                    bname = bdef.name.clone();
                }
            }
            push_user_record(
                name,
                email,
                &bid,
                &normalize_key(&bname),
                &mut users_seen,
                &mut email_aliases,
                &mut users_out,
            );
        }
    }
    if extra_users.is_empty() {
        for u in &index.users {
            push_user_record(
                &u.name,
                &u.email,
                &u.branch_id,
                &normalize_key(&u.branch_name),
                &mut users_seen,
                &mut email_aliases,
                &mut users_out,
            );
        }
    }

    // Merge distinct receipt creators not covered by user email alias
    for r in index.receipts.values() {
        if !caller.matches_receipt(r, effective_branch_lower.as_deref()) {
            continue;
        }
        let cname = r.created_by_name.trim();
        let cuser = r.created_by_username.trim();
        let candidate = if !cname.is_empty() { cname } else { cuser };
        if candidate.is_empty() {
            continue;
        }
        let ck = normalize_key(candidate);
        if email_aliases.contains(&ck) {
            continue;
        }
        if users_seen.insert(ck) {
            users_out.push(candidate.to_string());
        }
    }
    users_out.sort();

    // 4. Mahasangh / vehicle-category extra dropdowns
    let include_vehicle_meta = query.report_type.as_deref().is_some_and(|rt| {
        let t = rt.trim();
        t.eq_ignore_ascii_case("mahasang")
            || t.eq_ignore_ascii_case("mahasangh")
            || t.eq_ignore_ascii_case("vehicle-category")
            || t.eq_ignore_ascii_case("vehicle_category")
    });

    let (categories, vehicle_types, vehicle_type_groups, default_vehicle_types) =
        if include_vehicle_meta {
            let mut vt_seen: HashSet<String> = HashSet::new();
            let mut vt_list: Vec<String> = Vec::new();
            let mut push_vt = |raw: &str| {
                let t = raw.trim();
                if t.is_empty() || t == "सवारी प्रकार उपलब्ध छैन" {
                    return;
                }
                if vt_seen.insert(normalize_key(t)) {
                    vt_list.push(t.to_string());
                }
            };

            for k in index.category_by_vehicle_type.keys() {
                push_vt(k);
            }
            for v in vehicles {
                if let Some(inner) = v.inner_content.as_ref() {
                    if let Some(vt) = inner
                        .get("VehicleType")
                        .or_else(|| inner.get("vehicleType"))
                        .and_then(Value::as_str)
                    {
                        push_vt(vt);
                    }
                }
            }
            for r in index.receipts.values() {
                push_vt(&r.vehicle_type);
            }
            for dt in &settings.reports.mahasang.default_vehicle_types {
                push_vt(dt);
            }
            for g in &settings.reports.mahasang.vehicle_type_groups {
                for dt in &g.vehicle_types {
                    push_vt(dt);
                }
            }
            vt_list.sort();

            let groups: Vec<VehicleTypeGroupDto> = settings
                .reports
                .mahasang
                .vehicle_type_groups
                .iter()
                .map(|g| VehicleTypeGroupDto {
                    id: g.id.clone(),
                    name: g.name.clone(),
                    vehicle_types: g.vehicle_types.clone(),
                })
                .collect();

            (
                Some(index.categories_ordered.clone()),
                Some(vt_list),
                Some(groups),
                Some(settings.reports.mahasang.resolve_default_vehicle_types()),
            )
        } else {
            (None, None, None, None)
        };

    FilterOptionsResponse {
        fiscal_years: index.all_fiscal_years.clone(),
        branches,
        users: users_out,
        fee_heads,
        categories,
        vehicle_types,
        vehicle_type_groups,
        default_vehicle_types,
    }
}
