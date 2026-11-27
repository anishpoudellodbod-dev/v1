use chrono::NaiveDate;
use narayani_core::{extract_string_or_int, normalize_ascii_digits};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;

/// Compact single-instance AD <-> BS lookup table using packed `yyyymmdd` u32 keys.
/// Replaces 6 redundant String-keyed date HashMaps across the workspace.
pub struct CompactDateTable {
    ad_to_bs: HashMap<u32, u32>,
    bs_to_ad: HashMap<u32, u32>,
}

impl CompactDateTable {
    pub fn shared() -> &'static Self {
        static TABLE: OnceLock<CompactDateTable> = OnceLock::new();
        TABLE.get_or_init(Self::load_from_default_paths)
    }

    fn pack_ymd(y: i32, m: u32, d: u32) -> Option<u32> {
        if y < 0 {
            return None;
        }
        Some((y as u32) * 10_000 + m * 100 + d)
    }

    fn unpack_ymd(packed: u32) -> (i32, u32, u32) {
        let y = (packed / 10_000) as i32;
        let m = (packed / 100) % 100;
        let d = packed % 100;
        (y, m, d)
    }

    fn load_from_default_paths() -> Self {
        let mut mapper = crate::DateMapper::empty();
        let candidates = [
            "date_range.json",
            "date_mappings.json",
            "../../date_range.json",
            "../../date_mappings.json",
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../date_range.json"),
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../date_mappings.json"),
        ];
        for path in candidates {
            if mapper.load_from_json_file(path).is_ok() {
                break;
            }
        }
        Self::from_mapper(&mapper)
    }

    pub fn from_mapper(mapper: &crate::DateMapper) -> Self {
        let mut ad_to_bs = HashMap::with_capacity(mapper.ad_to_bs.len());
        let mut bs_to_ad = HashMap::with_capacity(mapper.ad_to_bs.len());
        for (ad_str, bs_str) in &mapper.ad_to_bs {
            if let (Some((ay, am, ad)), Some((by, bm, bd))) =
                (parse_ymd_parts(ad_str), parse_ymd_parts(bs_str))
            {
                if let (Some(ak), Some(bk)) = (Self::pack_ymd(ay, am, ad), Self::pack_ymd(by, bm, bd))
                {
                    ad_to_bs.insert(ak, bk);
                    bs_to_ad.insert(bk, ak);
                }
            }
        }
        Self { ad_to_bs, bs_to_ad }
    }

    pub fn lookup_ad_to_bs(&self, year: i32, month: u32, day: u32) -> Option<(i32, u32, u32)> {
        let key = Self::pack_ymd(year, month, day)?;
        self.ad_to_bs.get(&key).copied().map(Self::unpack_ymd)
    }

    pub fn lookup_bs_to_ad(&self, year: i32, month: u32, day: u32) -> Option<(i32, u32, u32)> {
        let key = Self::pack_ymd(year, month, day)?;
        self.bs_to_ad.get(&key).copied().map(Self::unpack_ymd)
    }

    pub fn ad_len(&self) -> usize {
        self.ad_to_bs.len()
    }
}

#[inline]
fn parse_ascii_digits_u32(bytes: &[u8]) -> Option<u32> {
    if bytes.is_empty() {
        return None;
    }
    let mut val = 0u32;
    for &b in bytes {
        if !b.is_ascii_digit() {
            return None;
        }
        val = val.checked_mul(10)?.checked_add((b - b'0') as u32)?;
    }
    Some(val)
}

fn parse_ymd_parts_from_ascii(s: &str) -> Option<(i32, u32, u32)> {
    let trimmed = s.trim();
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 10
        && (bytes[4] == b'-' || bytes[4] == b'/')
        && (bytes[7] == b'-' || bytes[7] == b'/')
        && (bytes.len() == 10 || bytes[10] == b'T' || bytes[10] == b' ')
    {
        let year = parse_ascii_digits_u32(&bytes[0..4])? as i32;
        let month = parse_ascii_digits_u32(&bytes[5..7])?;
        let day = parse_ascii_digits_u32(&bytes[8..10])?;
        if (1..=12).contains(&month) && (1..=32).contains(&day) {
            return Some((year, month, day));
        }
        return None;
    }

    let date_part = trimmed.split(&['T', ' '][..]).next()?;
    let mut parts = date_part.split(&['-', '/'][..]);
    let year: i32 = parts.next()?.trim().parse().ok()?;
    let month: u32 = parts.next()?.trim().parse().ok()?;
    let day: u32 = parts.next()?.trim().parse().ok()?;

    if !(1..=12).contains(&month) || !(1..=32).contains(&day) {
        return None;
    }

    Some((year, month, day))
}

pub fn parse_ymd_parts(raw: &str) -> Option<(i32, u32, u32)> {
    if raw.is_ascii() {
        parse_ymd_parts_from_ascii(raw)
    } else {
        let ascii = normalize_ascii_digits(raw);
        parse_ymd_parts_from_ascii(&ascii)
    }
}

/// Strictly parses a Gregorian `YYYY-MM-DD` (or ISO timestamp prefix) into `NaiveDate`.
pub fn parse_ad_naive_date(raw: &str) -> Option<NaiveDate> {
    let trimmed = normalize_ascii_digits(raw.trim());
    let date_part = trimmed.split(&['T', ' '][..]).next()?;
    let bytes = date_part.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let y = date_part[0..4].parse::<i32>().ok()?;
    let m = date_part[5..7].parse::<u32>().ok()?;
    let d = date_part[8..10].parse::<u32>().ok()?;
    NaiveDate::from_ymd_opt(y, m, d)
}

pub fn ad_date_str_to_bs_ymd(raw: &str) -> Option<(i32, u32, u32)> {
    let (year, month, day) = parse_ymd_parts(raw)?;

    if (2051..=2095).contains(&year) {
        return Some((year, month, day));
    }

    if (1970..=2050).contains(&year) {
        if let Some(bs_ymd) = CompactDateTable::shared().lookup_ad_to_bs(year, month, day) {
            return Some(bs_ymd);
        }
        // Fallback arithmetic conversion when date_range.json is not present
        let (bs_year, bs_month, bs_day) = if month > 4 || (month == 4 && day >= 14) {
            (year + 57, month - 3, day)
        } else {
            (year + 56, month + 9, day)
        };
        return Some((bs_year, bs_month, bs_day));
    }

    None
}

/// Parses an ISO/AD date string or BS date string (`year >= 2050`) into a Gregorian `NaiveDate`.
pub fn parse_naive_date_converting_bs_to_ad(raw: &str) -> Option<NaiveDate> {
    use chrono::Datelike;

    let d = parse_ad_naive_date(raw)?;
    if d.year() >= 2050 {
        if let Some((ay, am, ad)) =
            CompactDateTable::shared().lookup_bs_to_ad(d.year(), d.month(), d.day())
        {
            if let Some(ad_date) = NaiveDate::from_ymd_opt(ay, am, ad) {
                return Some(ad_date);
            }
        }
        let approx_year = d.year() - 57;
        return NaiveDate::from_ymd_opt(approx_year, d.month(), d.day().min(28));
    }
    Some(d)
}

pub fn bs_ymd_to_ad_date_str(year: i32, month: u32, day: u32) -> Option<String> {
    CompactDateTable::shared()
        .lookup_bs_to_ad(year, month, day)
        .map(|(ay, am, ad)| format!("{:04}-{:02}-{:02}", ay, am, ad))
}

pub fn infer_bs_fiscal_year_from_date_str(raw: &str) -> Option<String> {
    let (bs_year, bs_month, _bs_day) = ad_date_str_to_bs_ymd(raw)?;
    let bs_start = if bs_month >= 4 { bs_year } else { bs_year - 1 };
    Some(format!("{bs_start}/{:02}", (bs_start + 1) % 100))
}

fn parse_fiscal_year_start_ascii(raw: &str) -> Option<i32> {
    let first = raw.trim().split(&['/', '-'][..]).next()?.trim();
    if first.len() == 4 && first.bytes().all(|b| b.is_ascii_digit()) {
        first.parse::<i32>().ok()
    } else {
        None
    }
}

pub fn parse_fiscal_year_start(raw: &str) -> Option<i32> {
    if raw.is_ascii() {
        parse_fiscal_year_start_ascii(raw)
    } else {
        let ascii = normalize_ascii_digits(raw);
        parse_fiscal_year_start_ascii(&ascii)
    }
}

/// Canonicalizes BS fiscal year inputs (`"2083/2084"`, `"2083-84"`, `"२०८३-८४"`, `"2083"`)
/// into `"2083/84"` form. Non-parseable strings are returned trimmed.
pub fn canonical_fiscal_year_short(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(start) = parse_fiscal_year_start(trimmed) {
        format!("{start}/{:02}", (start + 1) % 100)
    } else {
        trimmed.to_string()
    }
}

fn push_fiscal_year_start_aliases(aliases: &mut Vec<String>, start_num: i32) {
    let start = format!("{:04}", start_num);
    let next_year = start_num + 1;
    let expected_short = format!("{:02}", next_year % 100);
    let expected_three = format!("{:03}", next_year % 1000);
    let expected_full = format!("{:04}", next_year);
    if (2000..=2099).contains(&start_num) {
        aliases.push(start.clone());
    }
    aliases.push(format!("{start}/{expected_short}"));
    aliases.push(format!("{start}/{expected_three}"));
    aliases.push(format!("{start}/{expected_full}"));
    aliases.push(format!("{start}-{expected_short}"));
    aliases.push(format!("{start}-{expected_three}"));
    aliases.push(format!("{start}-{expected_full}"));
}

pub fn expand_fiscal_year_aliases(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let ascii = normalize_ascii_digits(trimmed);
    let mut aliases = vec![trimmed.to_lowercase(), ascii.to_lowercase()];
    if let Some((start_str, end_str)) = ascii.split_once(&['/', '-'][..]) {
        let start = start_str.trim();
        let end = end_str.trim();
        if start.len() == 4 && start.chars().all(|c| c.is_ascii_digit()) {
            if let Ok(start_num) = start.parse::<i32>() {
                let next_year = start_num + 1;
                let expected_short = format!("{:02}", next_year % 100);
                let expected_three = format!("{:03}", next_year % 1000);
                let expected_full = format!("{:04}", next_year);
                if end == expected_short || end == expected_three || end == expected_full {
                    push_fiscal_year_start_aliases(&mut aliases, start_num);
                }
            }
        }
    } else if ascii.len() == 4 && ascii.chars().all(|c| c.is_ascii_digit()) {
        if let Ok(start_num) = ascii.parse::<i32>() {
            if (2000..=2099).contains(&start_num) {
                push_fiscal_year_start_aliases(&mut aliases, start_num);
            }
        }
    }

    aliases.sort();
    aliases.dedup();
    aliases
}

pub fn extract_bill_period_start_fiscal_year(inner: &Value) -> Option<String> {
    inner
        .get("periodStart")
        .and_then(Value::as_str)
        .and_then(infer_bs_fiscal_year_from_date_str)
}

pub fn extract_bill_inferred_fiscal_year(inner: &Value) -> Option<String> {
    ["periodStart", "billingDate", "paidAt", "periodEnd"]
        .into_iter()
        .find_map(|key| {
            inner
                .get(key)
                .and_then(Value::as_str)
                .and_then(infer_bs_fiscal_year_from_date_str)
        })
}

#[derive(Debug, Clone, Default)]
pub struct FiscalYearConfig {
    default_fiscal_year: Option<String>,
    fallback_fiscal_year: Option<String>,
    billing_rule_fiscal_year: Option<String>,
    /// Maps any normalized fiscal year token (UUID, short label, full label) to a canonical group id.
    alias_to_group: HashMap<String, usize>,
    /// Maps a canonical group id to its display string and optional chronological order rank.
    group_display: Vec<String>,
    group_rank: Vec<Option<i64>>,
    /// Maps explicit BS start years from SettingResource to their explicit SettingResource id.
    pub setting_year_to_id: HashMap<i32, String>,
}

impl FiscalYearConfig {
    pub fn from_settings(settings: &[&narayani_core::DatabaseResource]) -> Self {
        let mut config = Self {
            default_fiscal_year: None,
            fallback_fiscal_year: None,
            billing_rule_fiscal_year: None,
            alias_to_group: HashMap::new(),
            group_display: Vec::new(),
            group_rank: Vec::new(),
            setting_year_to_id: HashMap::new(),
        };

        for setting in settings {
            if let Some(inner) = &setting.inner_content {
                config.collect_from_value(inner);
            }
        }

        config
    }

    pub fn apply_billing_rules(&mut self, billing_rules: &[&narayani_core::DatabaseResource]) {
        let mut best: Option<(i32, String)> = None;
        for rule in billing_rules {
            let Some(inner) = rule.inner_content.as_ref() else {
                continue;
            };
            if let Some(status) = inner.get("status").and_then(Value::as_str) {
                if !status.trim().eq_ignore_ascii_case("ACTIVE") {
                    continue;
                }
            }
            let Some(fy_raw) = extract_string_or_int(inner.get("fiscalYear"))
                .or_else(|| extract_string_or_int(inner.get("fiscalYearId")))
            else {
                continue;
            };
            if let Some(start_year) = parse_fiscal_year_start(&fy_raw) {
                self.register_group(&fy_raw, &[fy_raw.clone()]);
                let should_update = match &best {
                    None => true,
                    Some((best_year, _)) => start_year > *best_year,
                };
                if should_update {
                    best = Some((start_year, fy_raw));
                }
            }
        }

        if let Some((_, best_fy)) = best {
            if self.billing_rule_fiscal_year.is_none() {
                self.billing_rule_fiscal_year = Some(best_fy);
            }
        }
    }

    fn collect_from_value(&mut self, value: &Value) {
        match value {
            Value::Object(map) => {
                if let Some(setting_key) = map.get("key").and_then(Value::as_str).map(str::trim) {
                    match setting_key {
                        "organization-vat"
                        | "billing-sms-phone"
                        | "fiscal-year-selector"
                        | "tqms-sangh-branch-filter" => return,
                        "arrears-list-rules" => {
                            if let Some(monthly_paid) =
                                map.get("monthlyPaid").and_then(Value::as_object)
                            {
                                let enabled = monthly_paid
                                    .get("enabled")
                                    .and_then(Value::as_bool)
                                    .unwrap_or(false);
                                if enabled && self.fallback_fiscal_year.is_none() {
                                    if let Some(fy_id) =
                                        extract_string_or_int(monthly_paid.get("fiscalYearId"))
                                            .or_else(|| {
                                                extract_string_or_int(
                                                    monthly_paid.get("fiscalYear"),
                                                )
                                            })
                                    {
                                        self.fallback_fiscal_year = Some(fy_id);
                                    }
                                }
                            }
                            return;
                        }
                        "linked-fee-rules" => {
                            let enabled = map
                                .get("enabled")
                                .and_then(Value::as_bool)
                                .unwrap_or(true);
                            if enabled && self.fallback_fiscal_year.is_none() {
                                if let Some(rules) = map.get("rules").and_then(Value::as_array) {
                                    for rule in rules {
                                        let active = rule
                                            .get("active")
                                            .and_then(Value::as_bool)
                                            .unwrap_or(false);
                                        if active {
                                            if let Some(fy_id) =
                                                extract_string_or_int(rule.get("fiscalYearId"))
                                                    .or_else(|| {
                                                        extract_string_or_int(
                                                            rule.get("fiscalYear"),
                                                        )
                                                    })
                                            {
                                                self.fallback_fiscal_year = Some(fy_id);
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                            return;
                        }
                        _ => {}
                    }
                }

                let billing_obj = map.get("billing").and_then(Value::as_object);
                if let Some(arrears_obj) = billing_obj
                    .and_then(|b| b.get("arrears"))
                    .or_else(|| map.get("arrears"))
                    .and_then(Value::as_object)
                {
                    if let Some(monthly_paid) =
                        arrears_obj.get("monthlyPaid").and_then(Value::as_object)
                    {
                        let enabled = monthly_paid
                            .get("enabled")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        if enabled && self.fallback_fiscal_year.is_none() {
                            if let Some(fy_id) =
                                extract_string_or_int(monthly_paid.get("fiscalYearId")).or_else(
                                    || extract_string_or_int(monthly_paid.get("fiscalYear")),
                                )
                            {
                                self.fallback_fiscal_year = Some(fy_id);
                            }
                        }
                    }
                }

                if let Some(linked_obj) = billing_obj
                    .and_then(|b| b.get("linkedFeeRules"))
                    .or_else(|| map.get("linkedFeeRules"))
                    .and_then(Value::as_object)
                {
                    let enabled = linked_obj
                        .get("enabled")
                        .and_then(Value::as_bool)
                        .unwrap_or(true);
                    if enabled && self.fallback_fiscal_year.is_none() {
                        if let Some(rules) = linked_obj.get("rules").and_then(Value::as_array) {
                            for rule in rules {
                                let active = rule
                                    .get("active")
                                    .and_then(Value::as_bool)
                                    .unwrap_or(false);
                                if active {
                                    if let Some(fy_id) =
                                        extract_string_or_int(rule.get("fiscalYearId")).or_else(
                                            || extract_string_or_int(rule.get("fiscalYear")),
                                        )
                                    {
                                        self.fallback_fiscal_year = Some(fy_id);
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }

                if let Some(fy_obj) = map.get("fiscalYear").and_then(Value::as_object) {
                    if let Some(active_fys) =
                        fy_obj.get("activeFiscalYears").and_then(Value::as_array)
                    {
                        for item in active_fys {
                            if let Some(s) = item.as_str().map(str::trim).filter(|s| !s.is_empty())
                            {
                                if parse_fiscal_year_start(s).is_some() {
                                    self.register_group(s, &[s.to_string()]);
                                    if self.default_fiscal_year.is_none() {
                                        self.default_fiscal_year = Some(s.to_string());
                                    }
                                }
                            } else if item.is_object() {
                                self.collect_from_value(item);
                            }
                        }
                    }
                    for (sub_k, sub_v) in fy_obj {
                        if sub_k != "activeFiscalYears"
                            && sub_k != "selector"
                            && sub_k != "billingAccess"
                            && (sub_v.is_object() || sub_v.is_array())
                        {
                            self.collect_from_value(sub_v);
                        }
                    }
                }

                for key in ["currentFiscalYear", "currentFiscalYearId", "activeFiscalYear"] {
                    if let Some(val) = map.get(key).and_then(Value::as_str).map(str::trim) {
                        if !val.is_empty() && self.default_fiscal_year.is_none() {
                            self.default_fiscal_year = Some(val.to_string());
                        }
                    }
                }

                let is_rule_like = map.contains_key("linkedFeeHeadId")
                    || map.contains_key("sourceFeeHeadId")
                    || map.contains_key("billingRuleId")
                    || map.contains_key("feeHead")
                    || map
                        .get("resourceType")
                        .and_then(Value::as_str)
                        .is_some_and(|t| t.eq_ignore_ascii_case("BILLING_RULE"));

                let id_str = extract_string_or_int(map.get("id"))
                    .or_else(|| extract_string_or_int(map.get("fiscalYearId")));
                let mut label_str = extract_string_or_int(map.get("name"))
                    .or_else(|| extract_string_or_int(map.get("code")))
                    .or_else(|| extract_string_or_int(map.get("label")))
                    .or_else(|| extract_string_or_int(map.get("title")))
                    .or_else(|| extract_string_or_int(map.get("fiscalYear")))
                    .or_else(|| extract_string_or_int(map.get("year")));
                let start_inferred = map
                    .get("startDate")
                    .or_else(|| map.get("start_date"))
                    .or_else(|| map.get("from"))
                    .and_then(Value::as_str)
                    .and_then(infer_bs_fiscal_year_from_date_str);

                if label_str
                    .as_deref()
                    .is_none_or(|l| parse_fiscal_year_start(l).is_none())
                {
                    if let Some(inferred) = start_inferred {
                        label_str = Some(inferred);
                    }
                }

                let is_fy_object = !is_rule_like
                    && (label_str
                        .as_deref()
                        .is_some_and(|l| parse_fiscal_year_start(l).is_some())
                        || map
                            .get("type")
                            .and_then(Value::as_str)
                            .is_some_and(|t| t.eq_ignore_ascii_case("FISCAL_YEAR"))
                        || (id_str.is_some()
                            && (map.contains_key("startDate")
                                || map.contains_key("start_date")
                                || map.contains_key("endDate")
                                || map.contains_key("end_date"))));

                if is_fy_object {
                    if let (Some(id), Some(y)) = (
                        id_str.as_ref(),
                        label_str.as_deref().and_then(parse_fiscal_year_start),
                    ) {
                        self.setting_year_to_id.entry(y).or_insert_with(|| id.clone());
                    }

                    let mut tokens = Vec::new();
                    if let Some(ref id) = id_str {
                        tokens.push(id.clone());
                    }
                    if let Some(ref label) = label_str {
                        tokens.push(label.clone());
                    }
                    let display = label_str.clone().or(id_str.clone());
                    if let Some(display_val) = display {
                        let is_current = map
                            .get("isCurrent")
                            .or_else(|| map.get("isActive"))
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                            || map
                                .get("status")
                                .and_then(Value::as_str)
                                .is_some_and(|s| s.trim().eq_ignore_ascii_case("ACTIVE"));

                        if is_current && self.default_fiscal_year.is_none() {
                            self.default_fiscal_year = Some(display_val.clone());
                        }

                        self.register_group(&display_val, &tokens);
                    }
                }

                for (k, v) in map {
                    if matches!(
                        k.as_str(),
                        "billing"
                            | "receipts"
                            | "insurance"
                            | "integrations"
                            | "notifications"
                            | "vehicleManagement"
                            | "billingAccess"
                            | "arrears"
                            | "linkedFeeRules"
                            | "vat"
                            | "smsSettings"
                            | "displacedTagging"
                            | "receiptCancelWindow"
                            | "insuranceEligibility"
                            | "selector"
                    ) || (k == "fiscalYear" && v.is_object())
                    {
                        continue;
                    }
                    if k == "fiscalYears" || k == "fiscal_years" || v.is_array() || v.is_object() {
                        self.collect_from_value(v);
                    }
                }
            }
            Value::Array(arr) => {
                for item in arr {
                    self.collect_from_value(item);
                }
            }
            _ => {}
        }
    }

    pub fn register_group(&mut self, display: &str, tokens: &[String]) {
        let mut all_aliases = Vec::new();
        for token in tokens {
            all_aliases.extend(expand_fiscal_year_aliases(token));
        }

        let mut matching_groups: Vec<usize> = all_aliases
            .iter()
            .filter_map(|a| self.alias_to_group.get(a).copied())
            .collect();
        matching_groups.sort_unstable();
        matching_groups.dedup();

        let parsed_rank = tokens.iter().find_map(|t| parse_fiscal_year_start(t));

        let group_id = if matching_groups.is_empty() {
            let gid = self.group_display.len();
            let rank = parsed_rank.map(|y| y as i64);
            self.group_display.push(display.to_string());
            self.group_rank.push(rank);
            gid
        } else {
            let primary_gid = matching_groups[0];

            if matching_groups.len() > 1 {
                for mapped_gid in self.alias_to_group.values_mut() {
                    if matching_groups[1..].contains(mapped_gid) {
                        *mapped_gid = primary_gid;
                    }
                }
                for &sec_gid in &matching_groups[1..] {
                    if self.group_rank[primary_gid].is_none() {
                        self.group_rank[primary_gid] = self.group_rank[sec_gid];
                    }
                    if parse_fiscal_year_start(&self.group_display[primary_gid]).is_none()
                        && parse_fiscal_year_start(&self.group_display[sec_gid]).is_some()
                    {
                        self.group_display[primary_gid] = self.group_display[sec_gid].clone();
                    }
                }
            }

            if self.group_rank[primary_gid].is_none() {
                if let Some(y) = parsed_rank {
                    self.group_rank[primary_gid] = Some(y as i64);
                }
            }
            if parse_fiscal_year_start(&self.group_display[primary_gid]).is_none()
                && parse_fiscal_year_start(display).is_some()
            {
                self.group_display[primary_gid] = display.to_string();
            }

            primary_gid
        };

        for alias in all_aliases {
            self.alias_to_group.insert(alias, group_id);
        }
    }

    fn lookup_group_id(&self, trimmed: &str) -> Option<usize> {
        if trimmed.is_empty() {
            return None;
        }
        if let Some(&gid) = self.alias_to_group.get(trimmed) {
            return Some(gid);
        }
        if trimmed.is_ascii() && trimmed.bytes().any(|b| b.is_ascii_uppercase()) {
            let lower = trimmed.to_ascii_lowercase();
            if let Some(&gid) = self.alias_to_group.get(&lower) {
                return Some(gid);
            }
        }
        expand_fiscal_year_aliases(trimmed)
            .iter()
            .find_map(|a| self.alias_to_group.get(a).copied())
    }

    pub fn has_ranked_group(&self, raw: &str) -> bool {
        self.lookup_group_id(raw.trim())
            .is_some_and(|gid| self.group_rank.get(gid).copied().flatten().is_some())
    }

    pub fn lookup_group_and_rank(&self, raw: &str) -> (Option<usize>, Option<i64>) {
        let trimmed = raw.trim();
        let gid = self.lookup_group_id(trimmed);
        let rank = gid
            .and_then(|g| self.group_rank.get(g).copied().flatten())
            .or_else(|| parse_fiscal_year_start(trimmed).map(|y| y as i64));
        (gid, rank)
    }

    pub fn canonicalize(&mut self, raw: &str) -> (usize, Option<i64>) {
        let trimmed = raw.trim();
        if let Some(gid) = self.lookup_group_id(trimmed) {
            return (gid, self.group_rank[gid]);
        }

        let aliases = expand_fiscal_year_aliases(trimmed);
        let gid = self.group_display.len();
        let rank = parse_fiscal_year_start(trimmed).map(|y| y as i64);
        self.group_display.push(trimmed.to_string());
        self.group_rank.push(rank);
        for alias in aliases {
            self.alias_to_group.insert(alias, gid);
        }
        (gid, rank)
    }

    pub fn display_for(&self, raw: &str) -> String {
        let trimmed = raw.trim();
        if let Some(gid) = self.lookup_group_id(trimmed) {
            if let Some(label) = self.group_display.get(gid) {
                return label.clone();
            }
        }
        trimmed.to_string()
    }

    pub fn default_or_fallback(&self) -> String {
        self.resolve_target_fiscal_year(None)
    }

    pub fn resolve_target_fiscal_year(&self, requested: Option<&str>) -> String {
        if let Some(req) = requested.map(str::trim).filter(|s| !s.is_empty()) {
            if let Some(gid) = self.lookup_group_id(req) {
                if let Some(label) = self.group_display.get(gid) {
                    if parse_fiscal_year_start(req).is_some() {
                        return req.to_string();
                    }
                    return label.clone();
                }
            }
            return req.to_string();
        }

        if let Some(def) = self.default_fiscal_year.as_ref() {
            if let Some(gid) = self.lookup_group_id(def.trim()) {
                if let Some(label) = self.group_display.get(gid) {
                    return label.clone();
                }
            }
            return def.clone();
        }

        if let Some(fb) = self.fallback_fiscal_year.as_ref() {
            if self.has_ranked_group(fb) || parse_fiscal_year_start(fb).is_some() {
                if let Some(gid) = self.lookup_group_id(fb.trim()) {
                    if let Some(label) = self.group_display.get(gid) {
                        return label.clone();
                    }
                }
                return fb.clone();
            }
            if let Some(rule_fy) = self.billing_rule_fiscal_year.as_ref() {
                if let Some(gid) = self.lookup_group_id(rule_fy.trim()) {
                    if let Some(label) = self.group_display.get(gid) {
                        return label.clone();
                    }
                }
                return rule_fy.clone();
            }
            return fb.clone();
        }

        if let Some(rule_fy) = self.billing_rule_fiscal_year.as_ref() {
            if let Some(gid) = self.lookup_group_id(rule_fy.trim()) {
                if let Some(label) = self.group_display.get(gid) {
                    return label.clone();
                }
            }
            return rule_fy.clone();
        }

        "2083/2084".to_string()
    }
}

pub fn resolve_bill_fiscal_year(
    inner: &Value,
    fy_config: &mut FiscalYearConfig,
) -> Option<String> {
    let bill_fy_id = extract_string_or_int(inner.get("fiscalYearId"));
    let bill_fy_label = extract_string_or_int(inner.get("fiscalYear"));
    if let (Some(ref fy_id), Some(ref fy_label)) = (&bill_fy_id, &bill_fy_label) {
        if !fy_id.eq_ignore_ascii_case(fy_label) {
            let distinct_explicit_years = matches!(
                (parse_fiscal_year_start(fy_id), parse_fiscal_year_start(fy_label)),
                (Some(y1), Some(y2)) if y1 != y2
            );
            if !distinct_explicit_years {
                if let Some(y) = parse_fiscal_year_start(fy_label) {
                    let conflicting_setting = fy_config
                        .setting_year_to_id
                        .get(&y)
                        .is_some_and(|sid| !sid.eq_ignore_ascii_case(fy_id));
                    if !conflicting_setting {
                        fy_config.register_group(fy_label, &[fy_id.clone(), fy_label.clone()]);
                    }
                }
            }
        }
    }

    let bill_fy_raw = bill_fy_label.or(bill_fy_id);

    if let Some(fy) = bill_fy_raw {
        if parse_fiscal_year_start(&fy).is_some() {
            fy_config.register_group(&fy, &[fy.clone()]);
            return Some(fy);
        }

        if let Some(period_fy) = extract_bill_period_start_fiscal_year(inner) {
            let conflicting_setting = parse_fiscal_year_start(&period_fy).is_some_and(|y| {
                fy_config
                    .setting_year_to_id
                    .get(&y)
                    .is_some_and(|sid| !sid.eq_ignore_ascii_case(&fy))
            });
            if !conflicting_setting {
                fy_config.register_group(&period_fy, &[fy, period_fy.clone()]);
            } else {
                fy_config.register_group(&period_fy, &[period_fy.clone()]);
            }
            return Some(period_fy);
        }

        if !fy_config.has_ranked_group(&fy) {
            if let Some(inferred_fy) = extract_bill_inferred_fiscal_year(inner) {
                let conflicting_setting = parse_fiscal_year_start(&inferred_fy).is_some_and(|y| {
                    fy_config
                        .setting_year_to_id
                        .get(&y)
                        .is_some_and(|sid| !sid.eq_ignore_ascii_case(&fy))
                });
                if !conflicting_setting {
                    fy_config.register_group(&inferred_fy, &[fy.clone(), inferred_fy.clone()]);
                }
            }
        }

        return Some(fy);
    }

    if let Some(period_fy) = extract_bill_period_start_fiscal_year(inner) {
        fy_config.register_group(&period_fy, &[period_fy.clone()]);
        return Some(period_fy);
    }

    if let Some(inferred_fy) = extract_bill_inferred_fiscal_year(inner) {
        fy_config.register_group(&inferred_fy, &[inferred_fy.clone()]);
        return Some(inferred_fy);
    }

    None
}
