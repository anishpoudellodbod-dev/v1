use crate::actions::is_admin;
use crate::cache::Cache;
use crate::resource::{DatabaseResource, ResourceName};
use crate::ServerData;
use actix_web::{web, HttpRequest, HttpResponse};
use chrono::Utc;
use narayani_core::{is_bill_ignored, normalize_vehicle_number};
use narayani_date::parse_naive_date_converting_bs_to_ad;
pub use narayani_settings::*;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

pub fn load_settings_from_cache(cache: &Cache) -> Option<AppSettings> {
    let settings = cache.read_all(ResourceName::SettingResource).ok()?;
    load_settings_from_resources(settings)
}

pub async fn load_settings_from_server_data(s_data: &ServerData) -> Option<AppSettings> {
    {
        let cache = s_data.cache.read().await;
        if let Some(settings) = load_settings_from_cache(&cache) {
            return Some(settings);
        }
    }
    let db_settings = s_data
        .db
        .read_all_resource(ResourceName::SettingResource)
        .await
        .ok()?;
    load_settings_from_resources(db_settings.iter())
}


pub fn resolve_tqms_sangh_branch_from_cache(cache: &Cache) -> Option<String> {
    let settings = cache.read_all(ResourceName::SettingResource).ok()?;
    let mut best: Option<(i32, String)> = None;
    for res in settings {
        let Some(inner) = res.inner_content.as_ref() else {
            continue;
        };
        if let Some(branch) = resolve_tqms_sangh_branch_from_value(inner) {
            let res_id = res.id.unwrap_or(0);
            if best.as_ref().is_none_or(|(best_id, _)| res_id >= *best_id) {
                best = Some((res_id, branch));
            }
        }
    }
    best.map(|(_, branch)| branch)
}

pub fn is_displaced_tagging_enabled_in_cache(cache: &Cache) -> bool {
    cache
        .read_all(ResourceName::SettingResource)
        .map(|s| is_displaced_tagging_enabled_in_resources(s))
        .unwrap_or(true)
}

// =========================================================================
// HTTP Handlers & Routes (/api/v1/settings)
// =========================================================================

#[derive(Debug, Deserialize)]
pub struct VatRequest {
    pub items: Vec<Value>,
    #[serde(default)]
    pub discount: f64,
}

#[derive(Debug, Deserialize)]
pub struct ArrearsQuery {
    #[serde(rename = "asOfDate", default)]
    pub as_of_date: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LinkedFeeEvalRequest {
    #[serde(rename = "fiscalYear", default)]
    pub fiscal_year: Option<String>,
    #[serde(rename = "vehicleId", default)]
    pub vehicle_id: Option<String>,
    #[serde(rename = "vehicleNumber", default)]
    pub vehicle_number: Option<String>,
    #[serde(rename = "requestedFeeHeadIds", default)]
    pub requested_fee_head_ids: Vec<String>,
    #[serde(default)]
    pub items: Vec<Value>,
}

#[derive(Debug, Deserialize)]
pub struct InsuranceEligibilityQuery {
    #[serde(rename = "fiscalYear", default)]
    pub fiscal_year: Option<String>,
}

pub async fn get_resolved_settings_handler(
    req: HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let mut settings = load_settings_from_server_data(&s_data)
        .await
        .unwrap_or_default();

    if !is_admin(req) {
        if !settings.notifications.sms.api_key.trim().is_empty() {
            settings.notifications.sms.api_key = "[REDACTED]".to_string();
        }
        if !settings.notifications.sms.api_secret.trim().is_empty() {
            settings.notifications.sms.api_secret = "[REDACTED]".to_string();
        }
    }

    HttpResponse::Ok().json(settings)
}

pub async fn calculate_vat_handler(
    body: web::Json<VatRequest>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let settings = load_settings_from_server_data(&s_data)
        .await
        .unwrap_or_default();
    let payload = body.into_inner();
    let result = calculate_vat(&settings.billing.vat, &payload.items, payload.discount);
    HttpResponse::Ok().json(result)
}

pub async fn evaluate_vehicle_arrears_handler(
    path: web::Path<String>,
    query: web::Query<ArrearsQuery>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let vehicle = path.into_inner();
    let settings = load_settings_from_server_data(&s_data)
        .await
        .unwrap_or_default();

    let as_of_ad = query
        .as_of_date
        .as_deref()
        .and_then(parse_naive_date_converting_bs_to_ad)
        .unwrap_or_else(|| chrono::Local::now().date_naive());

    let cache = s_data.cache.read().await;
    let bills = cache.read_all(ResourceName::BillResource).unwrap_or_default();
    let billing_rules = cache
        .read_all(ResourceName::BillingRuleResource)
        .unwrap_or_default();

    let result = evaluate_vehicle_arrears(
        &settings.billing.arrears,
        Some(vehicle.trim()),
        Some(vehicle.trim()),
        &bills,
        &billing_rules,
        as_of_ad,
    );
    HttpResponse::Ok().json(result)
}

pub async fn evaluate_linked_fees_handler(
    body: web::Json<LinkedFeeEvalRequest>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let settings = load_settings_from_server_data(&s_data)
        .await
        .unwrap_or_default();
    let payload = body.into_inner();

    let cache = s_data.cache.read().await;
    let all_bills = cache.read_all(ResourceName::BillResource).unwrap_or_default();
    let billing_rules = cache
        .read_all(ResourceName::BillingRuleResource)
        .unwrap_or_default();

    let mut requested = payload.requested_fee_head_ids;
    if requested.is_empty() && !payload.items.is_empty() {
        requested = extract_requested_fee_head_ids_from_items(&payload.items, &billing_rules);
    }

    let target_norm = payload
        .vehicle_number
        .as_deref()
        .and_then(normalize_vehicle_number);
    let vehicle_bills: Vec<&DatabaseResource> =
        if payload.vehicle_id.is_some() || target_norm.is_some() {
            all_bills
                .into_iter()
                .filter(|b| {
                    b.inner_content.as_ref().is_some_and(|inner| {
                        !is_bill_ignored(inner)
                            && bill_matches_vehicle(
                                inner,
                                payload.vehicle_id.as_deref(),
                                target_norm.as_deref(),
                            )
                    })
                })
                .collect()
        } else {
            Vec::new()
        };

    let result = evaluate_linked_fee_rules(
        &settings.billing.linked_fee_rules,
        payload.fiscal_year.as_deref(),
        &requested,
        &vehicle_bills,
        &billing_rules,
    );
    HttpResponse::Ok().json(result)
}

pub async fn check_receipt_cancellation_handler(
    path: web::Path<i32>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let bill_id = path.into_inner();
    let settings = load_settings_from_server_data(&s_data)
        .await
        .unwrap_or_default();

    let bill_opt = {
        let cache = s_data.cache.read().await;
        cache.read(ResourceName::BillResource, bill_id)
    };
    let bill = match bill_opt {
        Some(b) => b,
        None => match s_data.db.read_resource(ResourceName::BillResource, bill_id).await {
            Ok(b) if b.id.is_some() => b,
            _ => {
                return HttpResponse::NotFound().json(serde_json::json!({
                    "success": false,
                    "message": format!("Bill {bill_id} not found")
                }));
            }
        },
    };

    let result = check_receipt_cancellation(&settings.receipts.cancellation, &bill, Utc::now());
    HttpResponse::Ok().json(result)
}

pub async fn evaluate_insurance_eligibility_handler(
    path: web::Path<String>,
    query: web::Query<InsuranceEligibilityQuery>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let vehicle = path.into_inner();
    let settings = load_settings_from_server_data(&s_data)
        .await
        .unwrap_or_default();

    let cache = s_data.cache.read().await;
    let bills = cache.read_all(ResourceName::BillResource).unwrap_or_default();

    let result = evaluate_insurance_eligibility(
        &settings.insurance.eligibility,
        Some(vehicle.trim()),
        Some(vehicle.trim()),
        query.fiscal_year.as_deref(),
        &bills,
    );
    HttpResponse::Ok().json(result)
}

pub async fn get_fiscal_year_selector_handler(
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let settings = load_settings_from_server_data(&s_data)
        .await
        .unwrap_or_default();
    let current_start = current_bs_fiscal_year_start_year();
    let result = generate_fiscal_year_selector(&settings.fiscal_year.selector, current_start);
    HttpResponse::Ok().json(result)
}

pub async fn check_fiscal_year_access_handler(
    path: web::Path<String>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let fy = path.into_inner();
    let settings = load_settings_from_server_data(&s_data)
        .await
        .unwrap_or_default();
    let current_start = current_bs_fiscal_year_start_year();
    let result = check_fiscal_year_billing_access(&settings.fiscal_year, &fy, current_start);
    HttpResponse::Ok().json(result)
}

pub async fn get_displaced_tagging_status_handler(
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let settings = load_settings_from_server_data(&s_data)
        .await
        .unwrap_or_default();
    HttpResponse::Ok().json(serde_json::json!({
        "enabled": settings.vehicle_management.displaced_tagging.enabled
    }))
}

pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.route("/resolved", web::get().to(get_resolved_settings_handler))
        .route("/billing/calculate-vat", web::post().to(calculate_vat_handler))
        .route(
            "/billing/arrears/{vehicle}",
            web::get().to(evaluate_vehicle_arrears_handler),
        )
        .route(
            "/billing/linked-fees/evaluate",
            web::post().to(evaluate_linked_fees_handler),
        )
        .route(
            "/receipts/cancellation-check/{bill_id}",
            web::get().to(check_receipt_cancellation_handler),
        )
        .route(
            "/insurance/eligibility/{vehicle}",
            web::get().to(evaluate_insurance_eligibility_handler),
        )
        .route(
            "/fiscal-year/selector",
            web::get().to(get_fiscal_year_selector_handler),
        )
        .route(
            "/fiscal-year/access/{fiscal_year:.*}",
            web::get().to(check_fiscal_year_access_handler),
        )
        .route(
            "/vehicle-management/displaced-tagging",
            web::get().to(get_displaced_tagging_status_handler),
        );
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{http::StatusCode, test as web_test, App, HttpMessage};
    use chrono::{DateTime, NaiveDate};

    fn full_user_setting_resource() -> DatabaseResource {
        DatabaseResource {
            id: Some(1),
            resource_name: Some(ResourceName::SettingResource),
            inner_content: Some(serde_json::json!({
                "billing": {
                    "vat": {
                        "rate": 13,
                        "enabled": true,
                        "billingEntityIds": ["1"]
                    },
                    "arrears": {
                        "monthlyPaid": {
                            "enabled": true,
                            "fiscalYear": "2083/84",
                            "billingRuleId": "45"
                        },
                        "paidExpired": {
                            "enabled": true,
                            "billingRuleId": "5"
                        }
                    },
                    "linkedFeeRules": {
                        "rules": [
                            {
                                "id": "f72c97c7-ddd4-4d35-bbdd-86e4746e57db",
                                "active": true,
                                "fiscalYear": "2082/83",
                                "linkedFeeHeadId": "21",
                                "sourceFeeHeadId": "3",
                                "oncePerFiscalYear": true
                            },
                            {
                                "id": "f604ebf4-8c7a-4e48-b9c5-40be292dcb7d",
                                "active": true,
                                "fiscalYear": "2082/83",
                                "linkedFeeHeadId": "36",
                                "sourceFeeHeadId": "25",
                                "oncePerFiscalYear": true
                            }
                        ],
                        "enabled": true
                    }
                },
                "receipts": {
                    "cancellation": {
                        "hours": 24,
                        "enabled": false
                    }
                },
                "insurance": {
                    "eligibility": {
                        "enabled": false,
                        "ruleIds": [],
                        "minPaidRules": 1
                    }
                },
                "fiscalYear": {
                    "selector": {
                        "yearsBack": 4,
                        "yearsForward": 1,
                        "archivedYears": []
                    },
                    "billingAccess": {
                        "nextYear": null,
                        "previousYear": {
                            "status": "CLOSED",
                            "updatedAt": "2026-11-22T12:08:22.422Z",
                            "updatedBy": "System Administrator",
                            "fiscalYear": "2082/83"
                        }
                    }
                },
                "integrations": {
                    "tqms": {
                        "SanghBranch": "Loadboard sthaniya"
                    }
                },
                "notifications": {
                    "sms": {
                        "apiKey": "36A1291E165814",
                        "baseUrl": "https://samayasms.com.np/smsapi/index.php",
                        "enabled": false,
                        "provider": "samayasms",
                        "senderId": "",
                        "apiSecret": "",
                        "templates": [
                            {
                                "id": "PAID",
                                "name": "भुक्तानी प्राप्त",
                                "active": true,
                                "content": "Test gariyeko template तपाईंको सवारी नम्बर {vehicleNumber} को रकम रु {amount} प्राप्त भएको छ।\nधन्यवाद। {organizationName}\nम्याद सकिने मिति: {expiryDate}"
                            },
                            {
                                "id": "PAYMENT REMAINDER",
                                "name": "भुक्तानी रिमाइन्डर",
                                "active": true,
                                "content": "नमस्कार, सवारी नम्बर {vehicleNumber} को रु {amount} भुक्तानी बाँकी छ। कृपया {dueDate} भित्र भुक्तानी गर्नुहोस्।\n{organizationName}"
                            },
                            {
                                "id": "URGENT",
                                "name": "अत्यावश्यक रिमाइन्डर",
                                "active": false,
                                "content": "अत्यावश्यक सूचना: सवारी नम्बर {vehicleNumber} को भुक्तानी म्याद {dueDate} मा सकिँदैछ। सेवा निरन्तरताका लागि समयमै भुक्तानी गर्नुहोस्।\n{organizationName}"
                            }
                        ],
                        "webhookUrl": "",
                        "defaultCountryCode": "+977"
                    }
                },
                "schemaVersion": 1,
                "vehicleManagement": {
                    "displacedTagging": {
                        "enabled": true
                    }
                }
            })),
            external_content: None,
        }
    }

    #[test]
    fn test_parse_exact_nested_setting_resource_and_legacy_fallback() {
        let res = full_user_setting_resource();
        let parsed = AppSettings::from_inner_value(res.inner_content.as_ref().unwrap());

        assert_eq!(parsed.schema_version, 1);
        assert_eq!(parsed.billing.vat.rate, 13.0);
        assert!(parsed.billing.vat.enabled);
        assert_eq!(parsed.billing.vat.billing_entity_ids, vec!["1"]);

        assert!(parsed.billing.arrears.monthly_paid.enabled);
        assert_eq!(
            parsed.billing.arrears.monthly_paid.fiscal_year.as_deref(),
            Some("2083/84")
        );
        assert_eq!(
            parsed.billing.arrears.monthly_paid.billing_rule_id.as_deref(),
            Some("45")
        );
        assert!(parsed.billing.arrears.paid_expired.enabled);
        assert_eq!(
            parsed.billing.arrears.paid_expired.billing_rule_id.as_deref(),
            Some("5")
        );

        assert!(parsed.billing.linked_fee_rules.enabled);
        assert_eq!(parsed.billing.linked_fee_rules.rules.len(), 2);
        assert_eq!(parsed.billing.linked_fee_rules.rules[0].source_fee_head_id, "3");
        assert_eq!(parsed.billing.linked_fee_rules.rules[0].linked_fee_head_id, "21");

        assert_eq!(parsed.receipts.cancellation.hours, 24);
        assert!(!parsed.receipts.cancellation.enabled);

        assert!(!parsed.insurance.eligibility.enabled);
        assert_eq!(parsed.insurance.eligibility.min_paid_rules, 1);
        assert!(parsed.insurance.eligibility.rule_ids.is_empty());

        assert_eq!(parsed.fiscal_year.selector.years_back, 4);
        assert_eq!(parsed.fiscal_year.selector.years_forward, 1);
        assert!(parsed.fiscal_year.selector.archived_years.is_empty());
        assert!(parsed.fiscal_year.billing_access.next_year.is_none());
        let prev = parsed.fiscal_year.billing_access.previous_year.as_ref().unwrap();
        assert_eq!(prev.status, "CLOSED");
        assert_eq!(prev.fiscal_year, "2082/83");
        assert_eq!(prev.updated_by.as_deref(), Some("System Administrator"));

        assert_eq!(
            parsed.integrations.tqms.sangh_branch.as_deref(),
            Some("Loadboard sthaniya")
        );

        assert_eq!(parsed.notifications.sms.api_key, "36A1291E165814");
        assert!(!parsed.notifications.sms.enabled);
        assert_eq!(parsed.notifications.sms.provider, "samayasms");
        assert_eq!(parsed.notifications.sms.default_country_code, "+977");
        assert_eq!(parsed.notifications.sms.templates.len(), 3);
        assert_eq!(parsed.notifications.sms.templates[0].id, "PAID");
        assert!(parsed.vehicle_management.displaced_tagging.enabled);

        // Legacy flat schema parses identically
        let legacy = AppSettings::from_inner_value(&serde_json::json!({
            "vat": { "rate": 13, "enabled": true, "billingEntityIds": [1, 2] },
            "arrears": {
                "monthlyPaid": { "enabled": true, "fiscalYearId": "2082/83", "billingRuleId": 5 }
            },
            "receiptCancelWindow": { "hours": 48, "enabled": true },
            "insuranceEligibility": { "enabled": true, "ruleIds": [12], "minPaidRules": 1 },
            "SanghBranch": "Main Branch",
            "displacedTagging": { "enabled": false }
        }));
        assert_eq!(legacy.billing.vat.billing_entity_ids, vec!["1", "2"]);
        assert_eq!(legacy.receipts.cancellation.hours, 48);
        assert!(legacy.receipts.cancellation.enabled);
        assert!(legacy.insurance.eligibility.enabled);
        assert_eq!(
            legacy.integrations.tqms.sangh_branch.as_deref(),
            Some("Main Branch")
        );
        assert!(!legacy.vehicle_management.displaced_tagging.enabled);
    }

    #[test]
    fn test_vat_calculation_scenarios() {
        let vat = VatSettings {
            rate: 13.0,
            enabled: true,
            billing_entity_ids: vec!["1".to_string()],
        };

        // 1. All items match billingEntityId "1" -> 13% VAT
        let r1 = calculate_vat(
            &vat,
            &[serde_json::json!({ "amount": 500, "billingEntityId": "1" })],
            0.0,
        );
        assert_eq!(r1.subtotal, 500.0);
        assert_eq!(r1.taxable_subtotal, 500.0);
        assert_eq!(r1.vat_amount, 65.0);
        assert_eq!(r1.total, 565.0);

        // 2. Mixed taxable and exempt entities + discount
        let r2 = calculate_vat(
            &vat,
            &[
                serde_json::json!({ "amount": 1000, "billingEntityId": 1 }),
                serde_json::json!({ "amount": 400, "billingEntityId": "2" }),
            ],
            100.0,
        );
        assert_eq!(r2.subtotal, 1400.0);
        assert_eq!(r2.taxable_subtotal, 1000.0);
        assert_eq!(r2.vat_amount, 130.0);
        assert_eq!(r2.total, 1430.0);

        // 3. VAT disabled -> 0 VAT
        let mut disabled = vat.clone();
        disabled.enabled = false;
        let r3 = calculate_vat(
            &disabled,
            &[serde_json::json!({ "amount": 500, "billingEntityId": "1" })],
            0.0,
        );
        assert_eq!(r3.vat_rate_pct, 0.0);
        assert_eq!(r3.vat_amount, 0.0);
        assert_eq!(r3.total, 500.0);
        assert!(r3.vat_billing_entity_ids.is_empty());

        // 4. Empty billingEntityIds -> 0 taxable amount
        let mut no_entities = vat;
        no_entities.billing_entity_ids.clear();
        let r4 = calculate_vat(
            &no_entities,
            &[serde_json::json!({ "amount": 500, "billingEntityId": "1" })],
            0.0,
        );
        assert_eq!(r4.taxable_subtotal, 0.0);
        assert_eq!(r4.vat_amount, 0.0);
    }

    #[test]
    fn test_arrears_evaluation_monthly_paid_and_paid_expired_scenarios() {
        let settings = AppSettings::from_inner_value(
            full_user_setting_resource().inner_content.as_ref().unwrap(),
        );
        let rules = vec![DatabaseResource {
            id: Some(45),
            resource_name: Some(ResourceName::BillingRuleResource),
            inner_content: Some(serde_json::json!({
                "feeHead": { "id": 3, "name": "Monthly Fee" }
            })),
            external_content: None,
        }];
        let rule_refs: Vec<&DatabaseResource> = rules.iter().collect();

        let bills = vec![
            // Vehicle V1: paid 12 months on rule 45 for 2083/84 + unexpired rule 5
            DatabaseResource {
                id: Some(10),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(serde_json::json!({
                    "vehicle": { "id": "V1", "vehicleNumber": "BA-01-PA-1111" },
                    "fiscalYear": "2083/84",
                    "billingMonths": 12,
                    "status": "PAID",
                    "items": [{ "ruleId": "45", "amount": 7200 }]
                })),
                external_content: None,
            },
            DatabaseResource {
                id: Some(11),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(serde_json::json!({
                    "vehicle": { "id": "V1", "vehicleNumber": "BA-01-PA-1111" },
                    "periodEnd": "2026-12-30",
                    "status": "PAID",
                    "items": [{ "ruleId": "5", "amount": 600 }]
                })),
                external_content: None,
            },
            // Vehicle V2: partial 4 months on 2083/2084 + expired rule 5
            DatabaseResource {
                id: Some(20),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(serde_json::json!({
                    "vehicle": { "id": "V2", "vehicleNumber": "BA-01-PA-2222" },
                    "fiscalYear": "2083/2084",
                    "billingMonths": 4,
                    "status": "PAID",
                    "items": [{ "feeHeadId": "3", "amount": 2400 }]
                })),
                external_content: None,
            },
            DatabaseResource {
                id: Some(21),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(serde_json::json!({
                    "vehicle": { "id": "V2", "vehicleNumber": "BA-01-PA-2222" },
                    "periodEnd": "2026-11-01",
                    "status": "PAID",
                    "items": [{ "ruleId": "5", "amount": 600 }]
                })),
                external_content: None,
            },
            // Cancelled bill must be ignored
            DatabaseResource {
                id: Some(22),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(serde_json::json!({
                    "vehicle": { "id": "V2", "vehicleNumber": "BA-01-PA-2222" },
                    "fiscalYear": "2083/84",
                    "billingMonths": 12,
                    "status": "DELETED",
                    "items": [{ "ruleId": "45", "amount": 7200 }]
                })),
                external_content: None,
            },
        ];
        let bill_refs: Vec<&DatabaseResource> = bills.iter().collect();
        let today = NaiveDate::from_ymd_opt(2026, 11, 23).unwrap();

        let r_v1 = evaluate_vehicle_arrears(
            &settings.billing.arrears,
            Some("V1"),
            None,
            &bill_refs,
            &rule_refs,
            today,
        );
        assert!(!r_v1.has_arrears);
        assert_eq!(r_v1.monthly_paid.paid_months, 12);
        assert!(!r_v1.paid_expired.is_expired);

        let r_v2 = evaluate_vehicle_arrears(
            &settings.billing.arrears,
            None,
            Some("ba 01 pa 2222"),
            &bill_refs,
            &rule_refs,
            today,
        );
        assert!(r_v2.has_arrears);
        assert_eq!(r_v2.monthly_paid.paid_months, 4);
        assert_eq!(r_v2.monthly_paid.remaining_months, 8);
        assert!(r_v2.paid_expired.has_paid_history);
        assert!(r_v2.paid_expired.is_expired);

        // Disabled arrears -> has_arrears false
        let r_off = evaluate_vehicle_arrears(
            &ArrearsSettings::default(),
            Some("V2"),
            None,
            &bill_refs,
            &rule_refs,
            today,
        );
        assert!(!r_off.has_arrears);
    }

    #[test]
    fn test_linked_fee_rules_all_scenarios() {
        let settings = AppSettings::from_inner_value(
            full_user_setting_resource().inner_content.as_ref().unwrap(),
        );
        let rules = vec![DatabaseResource {
            id: Some(21),
            resource_name: Some(ResourceName::BillingRuleResource),
            inner_content: Some(serde_json::json!({
                "name": "सदस्यता शुल्क लिंक",
                "resourceType": "FEE_HEAD"
            })),
            external_content: None,
        }];
        let rule_refs: Vec<&DatabaseResource> = rules.iter().collect();

        // 1. Source fee "3" requested without linked fee "21" in "2082/83" -> missing
        let res_missing = evaluate_linked_fee_rules(
            &settings.billing.linked_fee_rules,
            Some("2082/83"),
            &["3".to_string()],
            &[],
            &rule_refs,
        );
        assert!(!res_missing.valid);
        assert_eq!(res_missing.missing_linked_fee_heads.len(), 1);
        assert_eq!(res_missing.missing_linked_fee_heads[0].linked_fee_head_id, "21");

        // 2. Source "3" + linked "21" together -> satisfied
        let res_ok = evaluate_linked_fee_rules(
            &settings.billing.linked_fee_rules,
            Some("2082/2083"),
            &["3".to_string(), "21".to_string()],
            &[],
            &rule_refs,
        );
        assert!(res_ok.valid);
        assert_eq!(res_ok.satisfied_linked_fee_heads, vec!["21"]);

        // 3. Linked "21" already paid in same FY & oncePerFiscalYear=true:
        //    3a. Requesting source "3" alone -> skipped cleanly (valid = true)
        let paid_bill = DatabaseResource {
            id: Some(1),
            resource_name: Some(ResourceName::BillResource),
            inner_content: Some(serde_json::json!({
                "fiscalYear": "2082/83",
                "status": "PAID",
                "items": [{ "feeHeadId": "21", "amount": 500 }]
            })),
            external_content: None,
        };
        let res_skip = evaluate_linked_fee_rules(
            &settings.billing.linked_fee_rules,
            Some("2082/83"),
            &["3".to_string()],
            &[&paid_bill],
            &rule_refs,
        );
        assert!(res_skip.valid);
        assert_eq!(res_skip.skipped_already_paid_fee_heads, vec!["21"]);

        //    3b. Requesting linked "21" again in same FY -> duplicate blocked!
        let res_dup = evaluate_linked_fee_rules(
            &settings.billing.linked_fee_rules,
            Some("2082/83"),
            &["3".to_string(), "21".to_string()],
            &[&paid_bill],
            &rule_refs,
        );
        assert!(!res_dup.valid);
        assert_eq!(res_dup.duplicate_once_per_fy_fee_heads, vec!["21"]);

        // 4. Different fiscal year ("2083/84") -> rule for "2082/83" does not apply
        let res_other_fy = evaluate_linked_fee_rules(
            &settings.billing.linked_fee_rules,
            Some("2083/84"),
            &["3".to_string()],
            &[],
            &rule_refs,
        );
        assert!(res_other_fy.valid);

        // 5. Rule for "2082/83" inactive or feature disabled -> valid = true
        let mut disabled_cfg = settings.billing.linked_fee_rules.clone();
        disabled_cfg.enabled = false;
        assert!(evaluate_linked_fee_rules(
            &disabled_cfg,
            Some("2082/83"),
            &["3".to_string()],
            &[],
            &rule_refs,
        )
        .valid);
    }

    #[test]
    fn test_receipt_cancellation_window_scenarios() {
        let now = DateTime::parse_from_rfc3339("2026-11-23T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let old_bill = DatabaseResource {
            id: Some(1),
            resource_name: Some(ResourceName::BillResource),
            inner_content: Some(serde_json::json!({ "paidAt": "2026-11-21T10:00:00Z" })),
            external_content: None,
        };
        let exact_24h_bill = DatabaseResource {
            id: Some(2),
            resource_name: Some(ResourceName::BillResource),
            inner_content: Some(serde_json::json!({ "paidAt": "2026-11-22T12:00:00Z" })),
            external_content: None,
        };
        let ext_ts_bill = DatabaseResource {
            id: Some(3),
            resource_name: Some(ResourceName::BillResource),
            inner_content: Some(serde_json::json!({})),
            external_content: Some(serde_json::json!({ "createdTime": "2026-11-23T06:00:00Z" })),
        };

        // Disabled -> old bill allowed
        let disabled = ReceiptCancellationSettings { hours: 24, enabled: false };
        assert!(check_receipt_cancellation(&disabled, &old_bill, now).allowed);

        // Enabled -> old bill blocked, exact 24h boundary & recent external_content bill allowed
        let enabled = ReceiptCancellationSettings { hours: 24, enabled: true };
        let old_res = check_receipt_cancellation(&enabled, &old_bill, now);
        assert!(!old_res.allowed);
        assert!(old_res.reason.contains("expired"));
        assert!(check_receipt_cancellation(&enabled, &exact_24h_bill, now).allowed);
        let ext_res = check_receipt_cancellation(&enabled, &ext_ts_bill, now);
        assert!(ext_res.allowed);
        assert!(ext_res.remaining_seconds.unwrap() > 0);
    }

    #[test]
    fn test_insurance_eligibility_scenarios() {
        let bills = vec![
            DatabaseResource {
                id: Some(1),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(serde_json::json!({
                    "vehicle": { "id": "V1", "vehicleNumber": "BA-01-PA-0001" },
                    "fiscalYear": "2083/84",
                    "status": "PAID",
                    "items": [{ "ruleId": "5" }, { "ruleId": "5" }]
                })),
                external_content: None,
            },
            DatabaseResource {
                id: Some(2),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(serde_json::json!({
                    "vehicle": { "id": "V1", "vehicleNumber": "BA-01-PA-0001" },
                    "fiscalYear": "2083/84",
                    "status": "PAID",
                    "items": [{ "ruleId": "12" }]
                })),
                external_content: None,
            },
            DatabaseResource {
                id: Some(3),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(serde_json::json!({
                    "vehicle": { "id": "V1", "vehicleNumber": "BA-01-PA-0001" },
                    "fiscalYear": "2083/84",
                    "status": "CANCELLED",
                    "items": [{ "ruleId": "99" }]
                })),
                external_content: None,
            },
        ];
        let bill_refs: Vec<&DatabaseResource> = bills.iter().collect();

        // Disabled -> not enabled, not eligible
        let disabled = InsuranceEligibilitySettings::default();
        let r0 = evaluate_insurance_eligibility(&disabled, Some("V1"), None, None, &bill_refs);
        assert!(!r0.enabled);
        assert!(!r0.eligible);

        // Enabled with empty ruleIds & minPaidRules = 2 -> eligible (5 and 12 paid; 99 ignored)
        let any_two = InsuranceEligibilitySettings {
            enabled: true,
            rule_ids: vec![],
            min_paid_rules: 2,
        };
        let r1 = evaluate_insurance_eligibility(&any_two, Some("V1"), None, Some("2083/84"), &bill_refs);
        assert!(r1.eligible);
        assert_eq!(r1.paid_rule_ids, vec!["12", "5"]);

        // Specific ruleIds ["5", "24"] with minPaidRules = 2 -> only "5" matched, "24" missing -> false
        let specific_two = InsuranceEligibilitySettings {
            enabled: true,
            rule_ids: vec!["5".into(), "24".into()],
            min_paid_rules: 2,
        };
        let r2 = evaluate_insurance_eligibility(&specific_two, Some("V1"), None, None, &bill_refs);
        assert!(!r2.eligible);
        assert_eq!(r2.matched_rule_ids, vec!["5"]);
        assert_eq!(r2.missing_rule_ids, vec!["24"]);
    }

    #[test]
    fn test_fiscal_year_selector_and_billing_access_scenarios() {
        let mut settings = AppSettings::from_inner_value(
            full_user_setting_resource().inner_content.as_ref().unwrap(),
        );
        settings.fiscal_year.selector.archived_years = vec!["2079/80".to_string()];

        // 1. Selector with current BS start year = 2083, yearsBack=4 (2079..=2084) excluding 2079/80
        let sel = generate_fiscal_year_selector(&settings.fiscal_year.selector, 2083);
        assert_eq!(sel.current_fiscal_year, "2083/84");
        assert_eq!(
            sel.all_window_years,
            vec!["2079/80", "2080/81", "2081/82", "2082/83", "2083/84", "2084/85"]
        );
        assert_eq!(
            sel.selectable_years,
            vec!["2080/81", "2081/82", "2082/83", "2083/84", "2084/85"]
        );

        // 2. Billing Access: current allowed, previous CLOSED blocked (canonical 2082/2083 == 2082/83), next null blocked, archived blocked
        let acc_cur = check_fiscal_year_billing_access(&settings.fiscal_year, "2083/84", 2083);
        assert!(acc_cur.allowed);

        let acc_prev_closed =
            check_fiscal_year_billing_access(&settings.fiscal_year, "2082/2083", 2083);
        assert!(!acc_prev_closed.allowed);
        assert_eq!(acc_prev_closed.code, "FISCAL_YEAR_BILLING_CLOSED");
        assert_eq!(
            acc_prev_closed.updated_by.as_deref(),
            Some("System Administrator")
        );

        let acc_next_null = check_fiscal_year_billing_access(&settings.fiscal_year, "2084/85", 2083);
        assert!(!acc_next_null.allowed);
        assert_eq!(acc_next_null.code, "FISCAL_YEAR_BILLING_CLOSED");

        let acc_archived = check_fiscal_year_billing_access(&settings.fiscal_year, "2079/80", 2083);
        assert!(!acc_archived.allowed);
        assert_eq!(acc_archived.code, "FISCAL_YEAR_ARCHIVED");

        // 3. Reopening previousYear or nextYear -> allowed!
        settings.fiscal_year.billing_access.previous_year.as_mut().unwrap().status = "OPEN".to_string();
        settings.fiscal_year.billing_access.next_year = Some(FiscalYearAccessEntry {
            status: "OPEN".to_string(),
            updated_at: None,
            updated_by: Some("Admin".to_string()),
            fiscal_year: "2084/85".to_string(),
        });
        assert!(check_fiscal_year_billing_access(&settings.fiscal_year, "2082/83", 2083).allowed);
        assert!(check_fiscal_year_billing_access(&settings.fiscal_year, "2084/85", 2083).allowed);
    }

    #[actix_web::test]
    async fn test_settings_http_endpoints_end_to_end() {
        let mut state = crate::access::test_helpers::MockDbState::default();
        state.seed(ResourceName::SettingResource, full_user_setting_resource());
        let (s_data, _) =
            crate::access::test_helpers::create_mock_server_data_with_state(state).await;

        {
            let mut cache = s_data.cache.write().await;
            cache.load_from_db(&s_data.db).await;
            cache.write(
                ResourceName::BillResource,
                501,
                DatabaseResource {
                    id: Some(501),
                    resource_name: Some(ResourceName::BillResource),
                    inner_content: Some(serde_json::json!({
                        "vehicle": { "id": "V50", "vehicleNumber": "BA-01-PA-5050" },
                        "fiscalYear": "2083/84",
                        "paidAt": Utc::now().to_rfc3339(),
                        "billingMonths": 12,
                        "status": "PAID",
                        "items": [{ "ruleId": "45", "amount": 5000 }]
                    })),
                    external_content: None,
                },
            );
        }

        let app = web_test::init_service(
            App::new()
                .app_data(s_data.clone())
                .service(web::scope("/api/v1/settings").configure(configure_routes)),
        )
        .await;

        // 1. Resolved endpoint masks secrets for non-admin, reveals for Admin
        let user_resp = web_test::call_service(
            &app,
            web_test::TestRequest::get()
                .uri("/api/v1/settings/resolved")
                .to_request(),
        )
        .await;
        assert_eq!(user_resp.status(), StatusCode::OK);
        let user_json: Value = web_test::read_body_json(user_resp).await;
        assert_eq!(user_json["notifications"]["sms"]["apiKey"], "[REDACTED]");
        assert_eq!(user_json["integrations"]["tqms"]["SanghBranch"], "Loadboard sthaniya");

        let admin_req = web_test::TestRequest::get()
            .uri("/api/v1/settings/resolved")
            .to_request();
        admin_req.extensions_mut().insert(crate::auth::Claims {
            sub: "admin@test.com".to_string(),
            exp: 9999999999,
            iat: 1000000000,
            role: crate::roles::Roles::Admin,
            user_id: Some(1),
            jti: None,
            role_name: None,
            tenant_id: None,
        });
        let admin_resp = web_test::call_service(&app, admin_req).await;
        let admin_json: Value = web_test::read_body_json(admin_resp).await;
        assert_eq!(admin_json["notifications"]["sms"]["apiKey"], "36A1291E165814");

        // 2. VAT endpoint
        let vat_resp = web_test::call_service(
            &app,
            web_test::TestRequest::post()
                .uri("/api/v1/settings/billing/calculate-vat")
                .set_json(serde_json::json!({
                    "items": [{ "amount": 1000, "billingEntityId": "1" }],
                    "discount": 0
                }))
                .to_request(),
        )
        .await;
        assert_eq!(vat_resp.status(), StatusCode::OK);
        let vat_json: VatCalculationResult = web_test::read_body_json(vat_resp).await;
        assert_eq!(vat_json.vat_amount, 130.0);
        assert_eq!(vat_json.total, 1130.0);

        // 3. Arrears endpoint
        let arr_resp = web_test::call_service(
            &app,
            web_test::TestRequest::get()
                .uri("/api/v1/settings/billing/arrears/V50")
                .to_request(),
        )
        .await;
        assert_eq!(arr_resp.status(), StatusCode::OK);
        let arr_json: ArrearsEvaluationResult = web_test::read_body_json(arr_resp).await;
        assert!(!arr_json.has_arrears);

        // 4. Linked fees endpoint
        let linked_resp = web_test::call_service(
            &app,
            web_test::TestRequest::post()
                .uri("/api/v1/settings/billing/linked-fees/evaluate")
                .set_json(serde_json::json!({
                    "fiscalYear": "2082/83",
                    "requestedFeeHeadIds": ["3"]
                }))
                .to_request(),
        )
        .await;
        assert_eq!(linked_resp.status(), StatusCode::OK);
        let linked_json: LinkedFeeEvaluationResult = web_test::read_body_json(linked_resp).await;
        assert!(!linked_json.valid);
        assert_eq!(linked_json.missing_linked_fee_heads[0].linked_fee_head_id, "21");

        // 5. Cancellation check endpoint
        let cancel_resp = web_test::call_service(
            &app,
            web_test::TestRequest::get()
                .uri("/api/v1/settings/receipts/cancellation-check/501")
                .to_request(),
        )
        .await;
        assert_eq!(cancel_resp.status(), StatusCode::OK);
        let cancel_json: ReceiptCancellationCheck = web_test::read_body_json(cancel_resp).await;
        assert!(!cancel_json.enabled);
        assert!(cancel_json.allowed);

        // 6. Insurance eligibility endpoint
        let ins_resp = web_test::call_service(
            &app,
            web_test::TestRequest::get()
                .uri("/api/v1/settings/insurance/eligibility/V50")
                .to_request(),
        )
        .await;
        assert_eq!(ins_resp.status(), StatusCode::OK);
        let ins_json: InsuranceEligibilityResult = web_test::read_body_json(ins_resp).await;
        assert!(!ins_json.enabled);

        // 7. Fiscal year selector & billing access (including slash in 2082/83)
        let sel_resp = web_test::call_service(
            &app,
            web_test::TestRequest::get()
                .uri("/api/v1/settings/fiscal-year/selector")
                .to_request(),
        )
        .await;
        assert_eq!(sel_resp.status(), StatusCode::OK);

        let fy_resp = web_test::call_service(
            &app,
            web_test::TestRequest::get()
                .uri("/api/v1/settings/fiscal-year/access/2082/83")
                .to_request(),
        )
        .await;
        assert_eq!(fy_resp.status(), StatusCode::OK);
        let fy_json: FiscalYearBillingAccessResult = web_test::read_body_json(fy_resp).await;
        assert!(!fy_json.allowed);
        assert_eq!(fy_json.code, "FISCAL_YEAR_BILLING_CLOSED");

        // 8. Displaced tagging endpoint
        let disp_resp = web_test::call_service(
            &app,
            web_test::TestRequest::get()
                .uri("/api/v1/settings/vehicle-management/displaced-tagging")
                .to_request(),
        )
        .await;
        assert_eq!(disp_resp.status(), StatusCode::OK);
        let disp_json: Value = web_test::read_body_json(disp_resp).await;
        assert_eq!(disp_json["enabled"], true);
    }
}
