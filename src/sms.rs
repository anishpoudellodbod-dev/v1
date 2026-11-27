use crate::resource::ResourceName;
use crate::ServerData;
use actix_web::{web, HttpMessage, HttpResponse};
use anyhow::{anyhow, bail, Result};
pub use narayani_integrations::sms::*;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Instant;

pub async fn generate_templates_from_server_data(
    s_data: &ServerData,
) -> Vec<GeneratedSmsTemplate> {
    {
        let cache = s_data.cache.read().await;
        let cached = generate_templates_from_cache(&cache);
        if !cached.is_empty() {
            return cached;
        }
    }

    let Ok(db_settings) = s_data
        .db
        .read_all_resource(ResourceName::SettingResource)
        .await
    else {
        return Vec::new();
    };
    generate_templates_from_settings(db_settings.iter())
}


/// Looks up the matching SMS template from `s_data.cache` (`ResourceName::SettingResource`),
/// falling back to `s_data.db.read_all_resource(ResourceName::SettingResource)` when cache misses.
pub async fn find_sms_template_in_server_data(
    s_data: &ServerData,
    sms_type: &SmsType,
    template_id_override: Option<&str>,
) -> Option<SettingSmsTemplate> {
    {
        let cache = s_data.cache.read().await;
        if let Some(tpl) = find_sms_template_in_cache(&cache, sms_type, template_id_override) {
            return Some(tpl);
        }
    }

    let db_settings = s_data
        .db
        .read_all_resource(ResourceName::SettingResource)
        .await
        .ok()?;
    find_sms_template_in_settings(db_settings.iter(), sms_type, template_id_override)
}


pub async fn find_sms_notification_settings_in_server_data(
    s_data: &ServerData,
) -> Option<crate::settings::SmsNotificationSettings> {
    {
        let cache = s_data.cache.read().await;
        if let Ok(settings) = cache.read_all(ResourceName::SettingResource) {
            if let Some(found) = extract_sms_notification_settings(settings) {
                return Some(found);
            }
        }
    }
    let db_settings = s_data
        .db
        .read_all_resource(ResourceName::SettingResource)
        .await
        .ok()?;
    extract_sms_notification_settings(db_settings.iter())
}


pub async fn send_template_with_server_data(
    client: &SmsClient,
    template: &SmsTemplate,
    s_data: &ServerData,
) -> Result<SmsSendResult> {
        let setting_sms = find_sms_notification_settings_in_server_data(s_data).await;
        if let Some(ref sms_cfg) = setting_sms {
            if !sms_cfg.enabled {
                bail!("SMS notifications are disabled in SettingResource");
            }
        }
        let setting_tpl = find_sms_template_in_server_data(
            s_data,
            &template.sms_type,
            template.template_id.as_deref(),
        )
        .await
        .ok_or_else(|| {
            anyhow!(
                "SMS template for {:?} not found in SettingResource",
                template.sms_type
            )
        })?;
        if !setting_tpl.active {
            bail!(
                "SMS template '{}' is inactive in SettingResource",
                setting_tpl.id
            );
        }

        let effective_cc = client.config.default_country_code.as_deref().or_else(|| {
            setting_sms
                .as_ref()
                .map(|s| s.default_country_code.as_str())
                .filter(|s| !s.trim().is_empty())
        });
        let contacts =
            normalize_contacts_with_country_code(&template.receiver, effective_cc)?;
        let message = template.render_with_template(&setting_tpl.content)?;
        client.send_raw_with_contacts(&contacts, &message, template.scheduled_time.as_deref())
            .await
    }


#[derive(Debug, Deserialize)]
pub struct SendRawSmsRequest {
    #[serde(deserialize_with = "deserialize_receiver")]
    pub contacts: String,
    pub msg: String,
    #[serde(default, alias = "scheduled_time")]
    pub time: Option<String>,
}

/// GET /api/v1/sms/templates — Lists SMS templates generated dynamically from `SettingResource`.
pub async fn list_sms_templates(s_data: web::Data<Arc<ServerData>>) -> HttpResponse {
    let templates = generate_templates_from_server_data(&s_data).await;
    let setting_sms = find_sms_notification_settings_in_server_data(&s_data).await;
    let resolved_cfg = SmsConfig::from_setting_or_env(setting_sms.as_ref());

    HttpResponse::Ok().json(serde_json::json!({
        "source": "SettingResource",
        "enabled": resolved_cfg.enabled,
        "provider": resolved_cfg.provider,
        "senderId": resolved_cfg.sender_id,
        "baseUrl": resolved_cfg.base_url,
        "routeId": resolved_cfg.route_id,
        "campaignId": resolved_cfg.campaign_id,
        "defaultCountryCode": resolved_cfg.default_country_code.unwrap_or_default(),
        "templates": templates,
    }))
}

/// GET /api/v1/sms/templates/{template_id} — Returns a generated SMS template by ID or alias.
pub async fn get_sms_template_by_id(
    path: web::Path<String>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let raw_id = path.into_inner();
    let trimmed_id = raw_id.trim();
    if trimmed_id.is_empty() {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "error": "template_id is required"
        }));
    }

    let templates = generate_templates_from_server_data(&s_data).await;
    let requested_type = SmsType::from_id(trimmed_id);
    let found = templates
        .iter()
        .find(|t| t.id.trim().eq_ignore_ascii_case(trimmed_id))
        .or_else(|| {
            templates
                .iter()
                .find(|t| requested_type.matches_template_id(&t.id))
        });

    match found {
        Some(tpl) => HttpResponse::Ok().json(tpl),
        None => HttpResponse::NotFound().json(serde_json::json!({
            "error": format!("SMS template '{trimmed_id}' not found in SettingResource")
        })),
    }
}

/// POST /api/v1/sms/preview — Renders and validates an `SmsTemplate` using `SettingResource` without sending.
pub async fn preview_sms_template(
    template: web::Json<SmsTemplate>,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let tpl = template.into_inner();
    let setting_sms = find_sms_notification_settings_in_server_data(&s_data).await;
    let default_cc = setting_sms
        .as_ref()
        .map(|s| s.default_country_code.as_str())
        .filter(|s| !s.trim().is_empty());
    let contacts = match normalize_contacts_with_country_code(&tpl.receiver, default_cc) {
        Ok(c) => c,
        Err(e) => return HttpResponse::BadRequest().json(serde_json::json!({ "error": e.to_string() })),
    };

    let Some(setting_tpl) =
        find_sms_template_in_server_data(&s_data, &tpl.sms_type, tpl.template_id.as_deref()).await
    else {
        return HttpResponse::NotFound().json(serde_json::json!({
            "error": format!("SMS template for {:?} not found in SettingResource", tpl.sms_type)
        }));
    };

    let resolved_sms_type = if matches!(tpl.sms_type, SmsType::CUSTOM(ref id) if id.is_empty()) {
        SmsType::from_id(&setting_tpl.id)
    } else {
        tpl.sms_type.clone()
    };

    match tpl.render_with_template(&setting_tpl.content) {
        Ok(message) => HttpResponse::Ok().json(serde_json::json!({
            "contacts": contacts,
            "sms_type": resolved_sms_type,
            "template_id": setting_tpl.id,
            "template_name": setting_tpl.name,
            "template_active": setting_tpl.active,
            "placeholders": extract_template_placeholders(&setting_tpl.content),
            "sms_enabled": setting_sms.as_ref().is_none_or(|s| s.enabled),
            "template_source": "setting_resource",
            "message": message,
            "char_count": message.chars().count(),
            "scheduled_time": tpl.scheduled_time,
        })),
        Err(e) => HttpResponse::BadRequest().json(serde_json::json!({ "error": e.to_string() })),
    }
}

/// POST /api/v1/sms/send — Renders an `SmsTemplate` (using `SettingResource` templates) and sends it via SamayaSMS.
pub async fn send_template_sms(
    template: web::Json<SmsTemplate>,
    http_req: actix_web::HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let started_at = Instant::now();
    if http_req.extensions().get::<crate::auth::Claims>().is_some() {
        if let crate::permissions::PermissionCheckOutcome::Denied { message, .. } =
            crate::permissions::enforce_request_permission(
                &http_req,
                &s_data,
                "SettingResource",
                crate::permissions::action::MANAGE_SMS,
                crate::permissions::extract_branch_id_from_req(&http_req),
            )
            .await
        {
            return HttpResponse::Forbidden().json(serde_json::json!({ "error": message }));
        }
    }
    let tpl = template.into_inner();
    let setting_sms = find_sms_notification_settings_in_server_data(&s_data).await;
    let cfg = SmsConfig::from_setting_or_env(setting_sms.as_ref());
    let provider = cfg.provider.clone();
    let client = SmsClient::new(cfg);

    let (response, summary, meta) = match send_template_with_server_data(&client, &tpl, &s_data).await
    {
        Ok(result) => {
            let meta = serde_json::json!({
                "templateId": tpl.template_id,
                "receiver": result.contacts,
                "shootId": result.shoot_id,
                "provider": provider,
                "vehicleNumber": tpl.vehicle_number,
            });
            let summary = format!("Sent template SMS to {}", result.contacts);
            (HttpResponse::Ok().json(result), summary, meta)
        }
        Err(e) => {
            let msg = e.to_string();
            let resp = if msg.contains("disabled") || msg.contains("inactive") {
                HttpResponse::Forbidden().json(serde_json::json!({ "error": msg }))
            } else if msg.contains("not found in SettingResource") {
                HttpResponse::NotFound().json(serde_json::json!({ "error": msg }))
            } else if msg.contains("is required")
                || msg.contains("exceeds maximum length")
                || msg.contains("empty")
                || msg.contains("Unsupported SMS provider")
            {
                HttpResponse::BadRequest().json(serde_json::json!({ "error": msg }))
            } else {
                HttpResponse::BadGateway().json(serde_json::json!({ "error": msg }))
            };
            let meta = serde_json::json!({
                "templateId": tpl.template_id,
                "receiver": tpl.receiver,
                "provider": provider,
                "vehicleNumber": tpl.vehicle_number,
                "error": msg,
            });
            (resp, format!("Template SMS send failed: {}", msg), meta)
        }
    };

    crate::audit::record_http_audit_async(crate::audit::HttpAuditEvent {
        req: Some(&http_req),
        category: crate::audit::AuditCategory::Notification,
        action: crate::audit::AuditAction::SmsSend,
        status_code: response.status().as_u16(),
        started_at: Some(started_at),
        resource_name: Some("SettingResource".to_string()),
        summary,
        branch_override: Some(tpl.sangh_branch),
        metadata: Some(meta),
        ..Default::default()
    });

    response
}

/// POST /api/v1/sms/send-raw — Sends a raw SMS message to comma-separated contacts via SamayaSMS.
pub async fn send_raw_sms(
    req: web::Json<SendRawSmsRequest>,
    http_req: actix_web::HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let started_at = Instant::now();
    if http_req.extensions().get::<crate::auth::Claims>().is_some() {
        if let crate::permissions::PermissionCheckOutcome::Denied { message, .. } =
            crate::permissions::enforce_request_permission(
                &http_req,
                &s_data,
                "SettingResource",
                crate::permissions::action::MANAGE_SMS,
                crate::permissions::extract_branch_id_from_req(&http_req),
            )
            .await
        {
            return HttpResponse::Forbidden().json(serde_json::json!({ "error": message }));
        }
    }
    let body = req.into_inner();
    let setting_sms = find_sms_notification_settings_in_server_data(&s_data).await;
    let cfg = SmsConfig::from_setting_or_env(setting_sms.as_ref());
    let provider = cfg.provider.clone();
    let client = SmsClient::new(cfg);

    let (response, summary, meta) = match client
        .send_raw(&body.contacts, &body.msg, body.time.as_deref())
        .await
    {
        Ok(result) => {
            let meta = serde_json::json!({
                "contacts": result.contacts,
                "shootId": result.shoot_id,
                "provider": provider,
                "scheduledTime": body.time,
            });
            let summary = format!("Sent raw SMS to {}", result.contacts);
            (HttpResponse::Ok().json(result), summary, meta)
        }
        Err(e) => {
            let msg = e.to_string();
            let resp = if msg.contains("disabled") {
                HttpResponse::Forbidden().json(serde_json::json!({ "error": msg }))
            } else if msg.contains("is required")
                || msg.contains("exceeds maximum length")
                || msg.contains("empty")
                || msg.contains("Unsupported SMS provider")
            {
                HttpResponse::BadRequest().json(serde_json::json!({ "error": msg }))
            } else {
                HttpResponse::BadGateway().json(serde_json::json!({ "error": msg }))
            };
            let meta = serde_json::json!({
                "contacts": body.contacts,
                "provider": provider,
                "scheduledTime": body.time,
                "error": msg,
            });
            (resp, format!("Raw SMS send failed: {}", msg), meta)
        }
    };

    crate::audit::record_http_audit_async(crate::audit::HttpAuditEvent {
        req: Some(&http_req),
        category: crate::audit::AuditCategory::Notification,
        action: crate::audit::AuditAction::SmsSend,
        status_code: response.status().as_u16(),
        started_at: Some(started_at),
        resource_name: Some("SettingResource".to_string()),
        summary,
        metadata: Some(meta),
        ..Default::default()
    });

    response
}

pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.route("/templates", web::get().to(list_sms_templates))
        .route("/templates/{template_id}", web::get().to(get_sms_template_by_id))
        .route("/preview", web::post().to(preview_sms_template))
        .route("/send", web::post().to(send_template_sms))
        .route("/send-raw", web::post().to(send_raw_sms));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::Cache;
    use crate::resource::DatabaseResource;
    use serde_json::Value;
    use std::collections::HashMap;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn test_template_validation_errors() {
        let mut cache = Cache::new();
        cache.write(ResourceName::SettingResource, 1, sample_setting_resource());

        // Missing amount & expiry_bs for PAID
        let invalid_paid = SmsTemplate {
            receiver: "9841000000".to_string(),
            sms_type: SmsType::PAID,
            template_id: None,
            vehicle_number: "BA-01-PA-1234".to_string(),
            sangh_branch: "Main Branch".to_string(),
            expiry_bs: None,
            due_date_bs: None,
            amount: None,
            scheduled_time: None,
            variables: HashMap::new(),
        };
        assert!(invalid_paid.render_with_cache(&cache).is_err());

        // Missing due_date_bs for PAYMENT_REMAINDER
        let invalid_rem = SmsTemplate {
            receiver: "9841000000".to_string(),
            sms_type: SmsType::PAYMENT_REMAINDER,
            template_id: None,
            vehicle_number: "BA-01-PA-1234".to_string(),
            sangh_branch: "Main Branch".to_string(),
            expiry_bs: None,
            due_date_bs: None,
            amount: None,
            scheduled_time: None,
            variables: HashMap::new(),
        };
        assert!(invalid_rem.render_with_cache(&cache).is_err());

        // Exceeds 720 chars in SettingResource template
        let mut max_len_cache = Cache::new();
        max_len_cache.write(
            ResourceName::SettingResource,
            2,
            DatabaseResource {
                id: Some(2),
                resource_name: Some(ResourceName::SettingResource),
                inner_content: Some(serde_json::json!({
                    "smsSettings": {
                        "templates": [{
                            "id": "payment-received",
                            "active": true,
                            "channel": "SMS",
                            "content": "a".repeat(MAX_SMS_CHAR_LEN + 1)
                        }]
                    }
                })),
                external_content: None,
            },
        );
        let too_long =
            SmsTemplate::new_paid("9841000000", "V1", "Branch", 100.0, "2083-01-01");
        assert!(too_long.render_with_cache(&max_len_cache).is_err());
    }

    #[test]
    fn test_json_deserialization_numeric_or_string_receiver() {
        let mut cache = Cache::new();
        cache.write(ResourceName::SettingResource, 1, sample_setting_resource());

        let json_num = r#"{
            "receiver": 9841234567,
            "sms_type": "PAID",
            "vehicle_number": "BA-01-PA-1234",
            "sangh_branch": "Main",
            "amount": 1500,
            "expiry_bs": "2083-09-01"
        }"#;
        let tpl: SmsTemplate = serde_json::from_str(json_num).unwrap();
        assert_eq!(tpl.receiver, "9841234567");
        assert_eq!(tpl.sms_type, SmsType::PAID);
        assert!(tpl.render_with_cache(&cache).is_ok());

        let json_alias = r#"{
            "receiver": "9841234567,9801234567",
            "sms_type": "PAYMENT_REMINDER",
            "vehicle_number": "BA-01-PA-1234",
            "sangh_branch": "Main",
            "amount": 500,
            "due_day_bs": "10"
        }"#;
        let tpl2: SmsTemplate = serde_json::from_str(json_alias).unwrap();
        assert_eq!(tpl2.sms_type, SmsType::PAYMENT_REMAINDER);
        assert_eq!(tpl2.due_date_bs.as_deref(), Some("10"));
        assert!(tpl2.render_with_cache(&cache).is_ok());
    }

    #[test]
    fn test_parse_samaya_sms_response() {
        assert_eq!(
            parse_samaya_sms_response("SMS-SHOOT-ID/ABC123XYZ").unwrap(),
            "ABC123XYZ"
        );
        assert!(parse_samaya_sms_response("ERR: INVALID API KEY").is_err());
        assert!(parse_samaya_sms_response("ERR: Rate limit exceeded. Try again later.").is_err());
        assert!(parse_samaya_sms_response("").is_err());
    }

    async fn spawn_mock_sms_server(
        status_line: &'static str,
        response_body: &'static str,
    ) -> (String, tokio::sync::oneshot::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();

        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = vec![0u8; 8192];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let req_str = String::from_utf8_lossy(&buf[..n]).to_string();
                let _ = tx.send(req_str);

                let resp = format!(
                    "HTTP/1.1 {status_line}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                    response_body.len()
                );
                let _ = socket.write_all(resp.as_bytes()).await;
                let _ = socket.flush().await;
            }
        });

        (format!("http://{addr}/smsapi/index.php"), rx)
    }

    #[actix_web::test]
    async fn test_send_template_via_mock_samaya_sms_api() {
        let (mock_url, req_rx) =
            spawn_mock_sms_server("200 OK", "SMS-SHOOT-ID/SHOOT998877").await;

        let client = SmsClient::new(SmsConfig {
            base_url: mock_url,
            api_key: DEFAULT_SMS_API_KEY.to_string(),
            campaign_id: Some(DEFAULT_SMS_CAMPAIGN_ID.to_string()),
            route_id: Some(DEFAULT_SMS_ROUTE_ID.to_string()),
            sender_id: "NARAYANI".to_string(),
            response_type: None,
            sms_api_type: None,
            ..SmsConfig::default()
        });

        let mut cache = Cache::new();
        cache.write(ResourceName::SettingResource, 1, sample_setting_resource());

        let tpl = SmsTemplate::new_paid(
            "9841111111",
            "BA-01-PA-9999",
            "Main Branch",
            2500.0,
            "2083-10-15",
        )
        .with_scheduled_time("2026-11-19 14:30");

        let result = client
            .send_template_with_cache(&tpl, &cache)
            .await
            .expect("SMS send should succeed");
        assert_eq!(result.shoot_id, "SHOOT998877");
        assert_eq!(result.contacts, "9841111111");
        assert_eq!(result.raw_response, "SMS-SHOOT-ID/SHOOT998877");

        let raw_http = req_rx.await.unwrap();
        assert!(raw_http.starts_with("POST /smsapi/index.php HTTP/1.1"));
        assert!(raw_http.contains("key=366979AD75A35B"));
        assert!(raw_http.contains("campaign=8572"));
        assert!(raw_http.contains("routeid=10244"));
        assert!(raw_http.contains("type=unicode"));
        assert!(raw_http.contains("contacts=9841111111"));
        assert!(raw_http.contains("senderid=NARAYANI"));
        assert!(raw_http.contains("time=2026-11-19+14%3A30"));
    }

    #[actix_web::test]
    async fn test_send_raw_handles_samaya_error_and_rate_limit_status() {
        let (mock_url, _rx) = spawn_mock_sms_server(
            "429 Too Many Requests",
            "ERR: Rate limit exceeded. Try again later.",
        )
        .await;

        let client = SmsClient::new(SmsConfig {
            base_url: mock_url,
            api_key: DEFAULT_SMS_API_KEY.to_string(),
            campaign_id: Some(DEFAULT_SMS_CAMPAIGN_ID.to_string()),
            route_id: Some(DEFAULT_SMS_ROUTE_ID.to_string()),
            sender_id: "NARAYANI".to_string(),
            response_type: None,
            sms_api_type: None,
            ..SmsConfig::default()
        });

        let err = client
            .send_raw("9841111111", "Hello test", None)
            .await
            .expect_err("HTTP 429 should return Err");
        assert!(err.to_string().contains("429"));
        assert!(err.to_string().contains("Rate limit exceeded"));
    }

    fn sample_setting_resource() -> DatabaseResource {
        DatabaseResource {
            id: Some(1),
            resource_name: Some(ResourceName::SettingResource),
            inner_content: Some(serde_json::json!({
                "vat": {
                    "rate": 13,
                    "enabled": true,
                    "billingEntityIds": ["1"]
                },
                "arrears": {
                    "monthlyPaid": {
                        "enabled": true,
                        "fiscalYearId": "15f21721-50e8-41af-9596-9d8cdb8d6a16",
                        "billingRuleId": "5"
                    },
                    "paidExpired": {
                        "enabled": true,
                        "billingRuleId": "5"
                    }
                },
                "fiscalYear": {
                    "selector": {
                        "yearsBack": 4,
                        "yearsForward": 1,
                        "archivedYears": []
                    },
                    "activeFiscalYears": []
                },
                "SanghBranch": "Loadboard sthaniya",
                "smsSettings": {
                    "templates": [
                        {
                            "id": "payment-received",
                            "name": "भुक्तानी प्राप्त",
                            "active": true,
                            "channel": "SMS",
                            "content": "तपाईंको सवारी नम्बर {vehicleNumber} को रकम रु {amount} प्राप्त भएको छ।\nधन्यवाद। {organizationName}\nम्याद सकिने मिति: {expiryDate}",
                            "category": "Billing"
                        },
                        {
                            "id": "payment-reminder",
                            "name": "भुक्तानी रिमाइन्डर",
                            "active": true,
                            "channel": "SMS",
                            "content": "नमस्कार, सवारी नम्बर {vehicleNumber} को रु {amount} भुक्तानी बाँकी छ। कृपया {dueDate} भित्र भुक्तानी गर्नुहोस्।\n{organizationName}",
                            "category": "Billing"
                        },
                        {
                            "id": "urgent-reminder",
                            "name": "अत्यावश्यक रिमाइन्डर",
                            "active": false,
                            "channel": "SMS",
                            "content": "अत्यावश्यक सूचना: सवारी नम्बर {vehicleNumber} को भुक्तानी म्याद {dueDate} मा सकिँदैछ। सेवा निरन्तरताका लागि समयमै भुक्तानी गर्नुहोस्।\n{organizationName}",
                            "category": "Billing"
                        }
                    ],
                    "automationRules": [
                        {
                            "id": "rule-bill",
                            "name": "बिल तयार भएपछि",
                            "enabled": true,
                            "template": "बिल तयार भएको सूचना",
                            "description": "नयाँ बिल सफलतापूर्वक तयार हुँदा सम्बन्धित व्यक्तिलाई सूचना पठाउने।"
                        },
                        {
                            "id": "rule-payment",
                            "name": "भुक्तानी प्राप्त भएपछि",
                            "enabled": true,
                            "template": "भुक्तानी प्राप्त",
                            "description": "भुक्तानी प्राप्त भएपछि रसिद वा भुक्तानी पुष्टि सन्देश पठाउने।"
                        },
                        {
                            "id": "rule-overdue",
                            "name": "भुक्तानी म्याद नजिकिँदा",
                            "enabled": false,
                            "template": "भुक्तानी रिमाइन्डर",
                            "description": "भुक्तानीको अन्तिम मिति अघि सम्झना सन्देश पठाउने।"
                        }
                    ]
                },
                "schemaVersion": 1,
                "linkedFeeRules": {
                    "rules": [
                        {
                            "id": "f72c97c7-ddd4-4d35-bbdd-86e4746e57db",
                            "active": true,
                            "fiscalYearId": "15f21721-50e8-41af-9596-9d8cdb8d6a16",
                            "linkedFeeHeadId": "21",
                            "sourceFeeHeadId": "3",
                            "oncePerFiscalYear": true
                        },
                        {
                            "id": "f604ebf4-8c7a-4e48-b9c5-40be292dcb7d",
                            "active": true,
                            "fiscalYearId": "15f21721-50e8-41af-9596-9d8cdb8d6a16",
                            "linkedFeeHeadId": "36",
                            "sourceFeeHeadId": "25",
                            "oncePerFiscalYear": true
                        }
                    ],
                    "enabled": true
                },
                "displacedTagging": {
                    "enabled": true
                },
                "receiptCancelWindow": {
                    "hours": 24,
                    "enabled": false
                },
                "insuranceEligibility": {
                    "enabled": false,
                    "ruleIds": [],
                    "minPaidRules": 1
                }
            })),
            external_content: None,
        }
    }

    #[test]
    fn test_render_templates_from_setting_resource_cache() {
        let mut cache = Cache::new();
        cache.write(ResourceName::SettingResource, 1, sample_setting_resource());

        // PAID -> payment-received
        let paid_tpl = SmsTemplate::new_paid(
            "9841000000",
            "BA-01-PA-1234",
            "Loadboard sthaniya",
            2000.0,
            "2083-08-30",
        );
        assert_eq!(
            paid_tpl.render_with_cache(&cache).unwrap(),
            "तपाईंको सवारी नम्बर BA-01-PA-1234 को रकम रु 2000 प्राप्त भएको छ।\nधन्यवाद। Loadboard sthaniya\nम्याद सकिने मिति: 2083-08-30"
        );

        // PAYMENT_REMAINDER -> payment-reminder (uses {vehicleNumber}, {amount}, {dueDate}, {organizationName})
        let rem_tpl = SmsTemplate::new_payment_remainder(
            "9841000000",
            "BA-01-PA-1234",
            "Loadboard sthaniya",
            "2083-09-15",
        )
        .with_amount(500.0);
        assert_eq!(
            rem_tpl.render_with_cache(&cache).unwrap(),
            "नमस्कार, सवारी नम्बर BA-01-PA-1234 को रु 500 भुक्तानी बाँकी छ। कृपया 2083-09-15 भित्र भुक्तानी गर्नुहोस्।\nLoadboard sthaniya"
        );

        // URGENT -> urgent-reminder
        let urgent_tpl = SmsTemplate::new_urgent(
            "9841000000, 9801000000",
            "BA-01-PA-1234",
            "Loadboard sthaniya",
            "2083-09-20",
        );
        assert_eq!(
            urgent_tpl.normalized_contacts().unwrap(),
            "9841000000,9801000000"
        );
        assert_eq!(
            urgent_tpl.render_with_cache(&cache).unwrap(),
            "अत्यावश्यक सूचना: सवारी नम्बर BA-01-PA-1234 को भुक्तानी म्याद 2083-09-20 मा सकिँदैछ। सेवा निरन्तरताका लागि समयमै भुक्तानी गर्नुहोस्।\nLoadboard sthaniya"
        );

        // Errors when SettingResource is absent in cache (no hardcoded fallback)
        let empty_cache = Cache::new();
        assert!(paid_tpl.render_with_cache(&empty_cache).is_err());
    }

    #[actix_web::test]
    async fn test_live_send_paid_sms_using_setting_resource_template() {
        dotenv::dotenv().ok();

        let mut cache = Cache::new();
        cache.write(ResourceName::SettingResource, 1, sample_setting_resource());

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        let random_num = 1000 + (nanos % 9000);
        let vehicle_number = format!("BA-01-PA-{random_num}");

        let tpl = SmsTemplate::new_paid(
            "9861282176",
            &vehicle_number,
            "lodbod test sangh",
            0.0,
            "2083-12-30",
        );

        let client = SmsClient::from_env();
        let result = client
            .send_template_with_cache(&tpl, &cache)
            .await
            .expect("Live SamayaSMS request with SettingResource PAID template should succeed");

        println!("Live SettingResource SMS sent: {result:?}");
        assert!(!result.shoot_id.is_empty());
        assert_eq!(result.contacts, "9861282176");
        assert_eq!(
            result.message,
            format!(
                "तपाईंको सवारी नम्बर {vehicle_number} को रकम रु 0 प्राप्त भएको छ।\nधन्यवाद। lodbod test sangh\nम्याद सकिने मिति: 2083-12-30"
            )
        );
    }

    fn nested_notifications_setting(enabled: bool, webhook_url: &str, provider: &str) -> DatabaseResource {
        DatabaseResource {
            id: Some(10),
            resource_name: Some(ResourceName::SettingResource),
            inner_content: Some(serde_json::json!({
                "integrations": { "tqms": { "SanghBranch": "Loadboard sthaniya" } },
                "notifications": {
                    "sms": {
                        "apiKey": "36A1291E165814",
                        "baseUrl": "https://samayasms.com.np/smsapi/index.php",
                        "enabled": enabled,
                        "provider": provider,
                        "senderId": "",
                        "apiSecret": "SECRET_999",
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
                        "webhookUrl": webhook_url,
                        "defaultCountryCode": "+977"
                    }
                }
            })),
            external_content: None,
        }
    }

    #[test]
    fn test_nested_notifications_sms_templates_and_country_code() {
        let mut cache = Cache::new();
        cache.write(
            ResourceName::SettingResource,
            10,
            nested_notifications_setting(true, "", "samayasms"),
        );

        let paid = SmsTemplate::new_paid("9841000000", "BA-01-PA-1", "Loadboard", 100.0, "2083-12-30");
        assert!(paid.render_with_cache(&cache).unwrap().starts_with("Test gariyeko template"));

        let rem = SmsTemplate::new_payment_remainder("9841000000", "BA-01-PA-1", "Loadboard", "2083-12-30")
            .with_amount(250.0);
        assert!(rem.render_with_cache(&cache).unwrap().contains("रु 250"));

        let urg = SmsTemplate::new_urgent("9841000000", "BA-01-PA-1", "Loadboard", "2083-12-30");
        assert!(urg.render_with_cache(&cache).unwrap().contains("अत्यावश्यक सूचना"));

        assert_eq!(
            normalize_contacts_with_country_code("9861282176, 09841000000, +9779801112233", Some("+977"))
                .unwrap(),
            "9779861282176,9779841000000,9779801112233"
        );
    }

    #[actix_web::test]
    async fn test_notifications_sms_enabled_active_provider_secret_and_webhook_enforcement() {
        let (mock_url, req_rx) = spawn_mock_sms_server("200 OK", "SMS-SHOOT-ID/SHOOT-NESTED").await;
        let (webhook_url, webhook_rx) = spawn_mock_sms_server("200 OK", "OK").await;

        let mut cache = Cache::new();
        // 1. Disabled SMS blocks sending (even for active template)
        cache.write(
            ResourceName::SettingResource,
            10,
            nested_notifications_setting(false, "", "samayasms"),
        );
        let client_disabled = SmsClient::new(SmsConfig::from_setting_or_env(
            extract_sms_notification_settings(cache.read_all(ResourceName::SettingResource).unwrap()).as_ref(),
        ));
        let paid = SmsTemplate::new_paid("9861282176", "BA-01-PA-1", "Loadboard", 0.0, "2083-12-30");
        let disabled_err = client_disabled
            .send_template_with_cache(&paid, &cache)
            .await
            .unwrap_err();
        assert!(disabled_err.to_string().contains("disabled"));

        // 2. Enabled SMS, inactive template ("URGENT", active: false) blocked on send
        cache.write(
            ResourceName::SettingResource,
            10,
            nested_notifications_setting(true, &webhook_url, "samayasms"),
        );
        let mut cfg = SmsConfig::from_setting_or_env(
            extract_sms_notification_settings(cache.read_all(ResourceName::SettingResource).unwrap()).as_ref(),
        );
        cfg.base_url = mock_url;
        let client = SmsClient::new(cfg);

        let urg = SmsTemplate::new_urgent("9861282176", "BA-01-PA-1", "Loadboard", "2083-12-30");
        let inactive_err = client.send_template_with_cache(&urg, &cache).await.unwrap_err();
        assert!(inactive_err.to_string().contains("inactive"));

        // 3. Unsupported provider rejected
        let mut bad_provider_cfg = SmsConfig::default();
        bad_provider_cfg.provider = "twilio".to_string();
        let bad_provider_err = SmsClient::new(bad_provider_cfg)
            .send_raw("9861282176", "Hi", None)
            .await
            .unwrap_err();
        assert!(bad_provider_err.to_string().contains("Unsupported SMS provider"));

        // 4. Valid active PAID template sends with +977 country code, apiSecret, and triggers webhook
        let res = client
            .send_template_with_cache(&paid, &cache)
            .await
            .expect("active PAID template should send");
        assert_eq!(res.contacts, "9779861282176");
        assert_eq!(res.shoot_id, "SHOOT-NESTED");

        let raw_http = req_rx.await.unwrap();
        assert!(raw_http.contains("key=36A1291E165814"));
        assert!(raw_http.contains("secret=SECRET_999"));
        assert!(raw_http.contains("contacts=9779861282176"));
        assert!(
            !raw_http.contains("routeid="),
            "custom apiKey without routeId must not send default routeid=10244"
        );
        assert!(
            !raw_http.contains("campaign="),
            "custom apiKey without campaignId must not send default campaign=8572"
        );

        let webhook_http = tokio::time::timeout(Duration::from_secs(2), webhook_rx)
            .await
            .expect("webhook should fire")
            .unwrap();
        assert!(webhook_http.contains("SHOOT-NESTED"));
        assert!(webhook_http.contains("9779861282176"));
    }

    async fn spawn_multi_mock_sms_server(
        expected_requests: usize,
        response_prefix: &'static str,
    ) -> (String, tokio::sync::mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::mpsc::channel(expected_requests.max(4));

        tokio::spawn(async move {
            for idx in 1..=expected_requests {
                if let Ok((mut socket, _)) = listener.accept().await {
                    let mut buf = vec![0u8; 8192];
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    let req_str = String::from_utf8_lossy(&buf[..n]).to_string();
                    let _ = tx.send(req_str).await;

                    let body = format!("{response_prefix}-{idx}");
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                    let _ = socket.flush().await;
                }
            }
        });

        (format!("http://{addr}/smsapi/index.php"), rx)
    }

    fn user_exact_notifications_setting(base_url: &str) -> DatabaseResource {
        DatabaseResource {
            id: Some(1),
            resource_name: Some(ResourceName::SettingResource),
            inner_content: Some(serde_json::json!({
                "schemaVersion": 1,
                "notifications": {
                    "sms": {
                        "apiKey": "36A1291E165814",
                        "baseUrl": base_url,
                        "enabled": true,
                        "provider": "samayasms",
                        "senderId": "Bit_Alert",
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
                }
            })),
            external_content: None,
        }
    }

    #[actix_web::test]
    async fn test_generate_and_render_templates_from_user_notifications_sms_setting_resource() {
        let mut state = crate::access::test_helpers::MockDbState::default();
        let initial_setting =
            user_exact_notifications_setting("https://samayasms.com.np/smsapi/index.php");
        state.seed(ResourceName::SettingResource, initial_setting.clone());
        let (s_data, _) =
            crate::access::test_helpers::create_mock_server_data_with_state(state).await;

        let app = actix_web::test::init_service(
            actix_web::App::new()
                .app_data(s_data.clone())
                .service(web::scope("/api/v1/sms").configure(configure_routes)),
        )
        .await;

        // 1. GET /api/v1/sms/templates generates all 3 templates from SettingResource (DB fallback works on cold cache)
        let list_resp = actix_web::test::call_service(
            &app,
            actix_web::test::TestRequest::get()
                .uri("/api/v1/sms/templates")
                .to_request(),
        )
        .await;
        assert_eq!(list_resp.status(), actix_web::http::StatusCode::OK);
        let list_body: Value = actix_web::test::read_body_json(list_resp).await;
        assert_eq!(list_body["enabled"], true);
        assert_eq!(list_body["provider"], "samayasms");
        assert_eq!(list_body["senderId"], "Bit_Alert");
        assert_eq!(list_body["defaultCountryCode"], "+977");
        assert!(list_body.get("apiKey").is_none(), "apiKey must not be exposed");
        assert!(
            list_body.get("apiSecret").is_none(),
            "apiSecret must not be exposed"
        );

        let templates = list_body["templates"].as_array().unwrap();
        assert_eq!(templates.len(), 3);
        assert_eq!(templates[0]["id"], "PAID");
        assert_eq!(templates[0]["active"], true);
        assert_eq!(templates[0]["sms_type"], "PAID");
        assert_eq!(
            templates[0]["placeholders"],
            serde_json::json!(["vehicleNumber", "amount", "organizationName", "expiryDate"])
        );

        assert_eq!(templates[1]["id"], "PAYMENT REMAINDER");
        assert_eq!(templates[1]["active"], true);
        assert_eq!(templates[1]["sms_type"], "PAYMENT_REMAINDER");
        assert_eq!(
            templates[1]["placeholders"],
            serde_json::json!(["vehicleNumber", "amount", "dueDate", "organizationName"])
        );

        assert_eq!(templates[2]["id"], "URGENT");
        assert_eq!(templates[2]["active"], false);
        assert_eq!(templates[2]["sms_type"], "URGENT");
        assert_eq!(
            templates[2]["placeholders"],
            serde_json::json!(["vehicleNumber", "dueDate", "organizationName"])
        );

        // 2. GET /api/v1/sms/templates/{template_id} (including URL-encoded space in "PAYMENT REMAINDER")
        for (uri, expected_id, expected_active) in [
            ("/api/v1/sms/templates/PAID", "PAID", true),
            (
                "/api/v1/sms/templates/PAYMENT%20REMAINDER",
                "PAYMENT REMAINDER",
                true,
            ),
            ("/api/v1/sms/templates/URGENT", "URGENT", false),
        ] {
            let resp = actix_web::test::call_service(
                &app,
                actix_web::test::TestRequest::get().uri(uri).to_request(),
            )
            .await;
            assert_eq!(resp.status(), actix_web::http::StatusCode::OK);
            let tpl_json: Value = actix_web::test::read_body_json(resp).await;
            assert_eq!(tpl_json["id"], expected_id);
            assert_eq!(tpl_json["active"], expected_active);
        }

        let missing_resp = actix_web::test::call_service(
            &app,
            actix_web::test::TestRequest::get()
                .uri("/api/v1/sms/templates/DOES_NOT_EXIST")
                .to_request(),
        )
        .await;
        assert_eq!(missing_resp.status(), actix_web::http::StatusCode::NOT_FOUND);

        // 3. POST /api/v1/sms/preview works for PAID, PAYMENT REMAINDER, and inactive URGENT
        let preview_paid = actix_web::test::call_service(
            &app,
            actix_web::test::TestRequest::post()
                .uri("/api/v1/sms/preview")
                .set_json(serde_json::json!({
                    "receiver": "9861282176",
                    "sms_type": "PAID",
                    "vehicleNumber": "LU-01-KA-1234",
                    "organizationName": "Narayani Sangh",
                    "amount": 1500,
                    "expiryDate": "2083-12-30"
                }))
                .to_request(),
        )
        .await;
        assert_eq!(preview_paid.status(), actix_web::http::StatusCode::OK);
        let paid_body: Value = actix_web::test::read_body_json(preview_paid).await;
        assert_eq!(paid_body["contacts"], "9779861282176");
        assert_eq!(paid_body["template_id"], "PAID");
        assert_eq!(paid_body["template_active"], true);
        assert_eq!(
            paid_body["message"],
            "Test gariyeko template तपाईंको सवारी नम्बर LU-01-KA-1234 को रकम रु 1500 प्राप्त भएको छ।\nधन्यवाद। Narayani Sangh\nम्याद सकिने मिति: 2083-12-30"
        );

        let preview_rem = actix_web::test::call_service(
            &app,
            actix_web::test::TestRequest::post()
                .uri("/api/v1/sms/preview")
                .set_json(serde_json::json!({
                    "receiver": "9861282176",
                    "sms_type": "PAYMENT REMAINDER",
                    "variables": {
                        "vehicleNumber": "LU-01-KA-1234",
                        "amount": 2500,
                        "dueDate": "2083-12-25",
                        "organizationName": "Narayani Sangh"
                    }
                }))
                .to_request(),
        )
        .await;
        assert_eq!(preview_rem.status(), actix_web::http::StatusCode::OK);
        let rem_body: Value = actix_web::test::read_body_json(preview_rem).await;
        assert_eq!(rem_body["template_id"], "PAYMENT REMAINDER");
        assert_eq!(
            rem_body["message"],
            "नमस्कार, सवारी नम्बर LU-01-KA-1234 को रु 2500 भुक्तानी बाँकी छ। कृपया 2083-12-25 भित्र भुक्तानी गर्नुहोस्।\nNarayani Sangh"
        );

        let preview_urg = actix_web::test::call_service(
            &app,
            actix_web::test::TestRequest::post()
                .uri("/api/v1/sms/preview")
                .set_json(serde_json::json!({
                    "receiver": "9861282176",
                    "sms_type": "URGENT",
                    "vehicle_number": "LU-01-KA-1234",
                    "sangh_branch": "Narayani Sangh",
                    "due_date_bs": "2083-12-28"
                }))
                .to_request(),
        )
        .await;
        assert_eq!(preview_urg.status(), actix_web::http::StatusCode::OK);
        let urg_body: Value = actix_web::test::read_body_json(preview_urg).await;
        assert_eq!(urg_body["template_id"], "URGENT");
        assert_eq!(urg_body["template_active"], false);

        // 4. POST /api/v1/sms/send using cache-populated SettingResource pointed to mock SamayaSMS server
        let (mock_url, mut rx) = spawn_multi_mock_sms_server(2, "SMS-SHOOT-ID/USER").await;
        s_data.cache.write().await.write(
            ResourceName::SettingResource,
            1,
            user_exact_notifications_setting(&mock_url),
        );

        let send_paid = actix_web::test::call_service(
            &app,
            actix_web::test::TestRequest::post()
                .uri("/api/v1/sms/send")
                .set_json(serde_json::json!({
                    "receiver": "9861282176",
                    "sms_type": "PAID",
                    "vehicleNumber": "LU-01-KA-1234",
                    "organizationName": "Narayani Sangh",
                    "amount": 1500,
                    "expiryDate": "2083-12-30"
                }))
                .to_request(),
        )
        .await;
        assert_eq!(send_paid.status(), actix_web::http::StatusCode::OK);
        let send_paid_res: SmsSendResult = actix_web::test::read_body_json(send_paid).await;
        assert_eq!(send_paid_res.shoot_id, "USER-1");
        assert_eq!(send_paid_res.contacts, "9779861282176");

        let raw_paid_http = rx.recv().await.unwrap();
        assert!(raw_paid_http.contains("key=36A1291E165814"));
        assert!(raw_paid_http.contains("senderid=Bit_Alert"));
        assert!(raw_paid_http.contains("type=unicode"));
        assert!(raw_paid_http.contains("contacts=9779861282176"));
        assert!(raw_paid_http.contains("msg=Test+gariyeko+template+"));
        assert!(
            !raw_paid_http.contains("routeid="),
            "custom SettingResource apiKey without routeId must not inject default routeid"
        );
        assert!(
            !raw_paid_http.contains("campaign="),
            "custom SettingResource apiKey without campaignId must not inject default campaign"
        );

        let send_rem = actix_web::test::call_service(
            &app,
            actix_web::test::TestRequest::post()
                .uri("/api/v1/sms/send")
                .set_json(serde_json::json!({
                    "receiver": "9861282176",
                    "sms_type": "PAYMENT REMAINDER",
                    "vehicleNumber": "LU-01-KA-1234",
                    "organizationName": "Narayani Sangh",
                    "amount": 2500,
                    "dueDate": "2083-12-25"
                }))
                .to_request(),
        )
        .await;
        assert_eq!(send_rem.status(), actix_web::http::StatusCode::OK);
        let send_rem_res: SmsSendResult = actix_web::test::read_body_json(send_rem).await;
        assert_eq!(send_rem_res.shoot_id, "USER-2");
        let raw_rem_http = rx.recv().await.unwrap();
        assert!(raw_rem_http.contains("senderid=Bit_Alert"));

        // URGENT is active: false -> 403 Forbidden
        let send_urg = actix_web::test::call_service(
            &app,
            actix_web::test::TestRequest::post()
                .uri("/api/v1/sms/send")
                .set_json(serde_json::json!({
                    "receiver": "9861282176",
                    "sms_type": "URGENT",
                    "vehicleNumber": "LU-01-KA-1234",
                    "organizationName": "Narayani Sangh",
                    "dueDate": "2083-12-28"
                }))
                .to_request(),
        )
        .await;
        assert_eq!(send_urg.status(), actix_web::http::StatusCode::FORBIDDEN);
    }

    #[actix_web::test]
    async fn test_dynamic_custom_template_generated_from_setting_resource() {
        let mut setting = user_exact_notifications_setting(DEFAULT_SMS_BASE_URL);
        if let Some(arr) = setting
            .inner_content
            .as_mut()
            .and_then(|v| v.pointer_mut("/notifications/sms/templates"))
            .and_then(Value::as_array_mut)
        {
            arr.push(serde_json::json!({
                "id": "CUSTOM_WELCOME",
                "name": "Custom Welcome",
                "active": true,
                "content": "Namaste {memberName}, your code is {otp} for {organizationName}"
            }));
        }

        let (s_data, _) = crate::access::test_helpers::create_mock_server_data_with_state(
            crate::access::test_helpers::MockDbState::default(),
        )
        .await;
        s_data
            .cache
            .write()
            .await
            .write(ResourceName::SettingResource, 1, setting);

        let generated = generate_templates_from_cache(&*s_data.cache.read().await);
        let custom = generated
            .iter()
            .find(|t| t.id == "CUSTOM_WELCOME")
            .expect("CUSTOM_WELCOME template should be generated");
        assert_eq!(
            custom.placeholders,
            vec!["memberName", "otp", "organizationName"]
        );
        assert_eq!(
            custom.sms_type,
            SmsType::CUSTOM("CUSTOM_WELCOME".to_string())
        );

        let mut vars = HashMap::new();
        vars.insert("memberName".to_string(), Value::String("Sita".to_string()));
        vars.insert("otp".to_string(), serde_json::json!(4321));
        vars.insert(
            "organizationName".to_string(),
            Value::String("Narayani".to_string()),
        );
        assert_eq!(
            custom.render_with_variables(&vars).unwrap(),
            "Namaste Sita, your code is 4321 for Narayani"
        );

        let app = actix_web::test::init_service(
            actix_web::App::new()
                .app_data(s_data)
                .service(web::scope("/api/v1/sms").configure(configure_routes)),
        )
        .await;

        // Render via template_id without sms_type
        let preview_by_tid = actix_web::test::call_service(
            &app,
            actix_web::test::TestRequest::post()
                .uri("/api/v1/sms/preview")
                .set_json(serde_json::json!({
                    "receiver": "9861282176",
                    "template_id": "CUSTOM_WELCOME",
                    "variables": {
                        "memberName": "Hari",
                        "otp": 9876,
                        "organizationName": "Narayani Sangh"
                    }
                }))
                .to_request(),
        )
        .await;
        assert_eq!(preview_by_tid.status(), actix_web::http::StatusCode::OK);
        let body_by_tid: Value = actix_web::test::read_body_json(preview_by_tid).await;
        assert_eq!(body_by_tid["sms_type"], "CUSTOM_WELCOME");
        assert_eq!(
            body_by_tid["message"],
            "Namaste Hari, your code is 9876 for Narayani Sangh"
        );

        // Render via sms_type: "CUSTOM_WELCOME"
        let preview_by_type = actix_web::test::call_service(
            &app,
            actix_web::test::TestRequest::post()
                .uri("/api/v1/sms/preview")
                .set_json(serde_json::json!({
                    "receiver": "9861282176",
                    "sms_type": "CUSTOM_WELCOME",
                    "variables": {
                        "memberName": "Gita",
                        "otp": "1122",
                        "organizationName": "Narayani Sangh"
                    }
                }))
                .to_request(),
        )
        .await;
        assert_eq!(preview_by_type.status(), actix_web::http::StatusCode::OK);
        let body_by_type: Value = actix_web::test::read_body_json(preview_by_type).await;
        assert_eq!(
            body_by_type["message"],
            "Namaste Gita, your code is 1122 for Narayani Sangh"
        );

        // Missing custom placeholder -> 400 Bad Request
        let missing_otp = actix_web::test::call_service(
            &app,
            actix_web::test::TestRequest::post()
                .uri("/api/v1/sms/preview")
                .set_json(serde_json::json!({
                    "receiver": "9861282176",
                    "template_id": "CUSTOM_WELCOME",
                    "variables": {
                        "memberName": "Hari",
                        "organizationName": "Narayani Sangh"
                    }
                }))
                .to_request(),
        )
        .await;
        assert_eq!(missing_otp.status(), actix_web::http::StatusCode::BAD_REQUEST);
    }

    #[actix_web::test]
    async fn test_sms_config_route_and_campaign_resolution_for_custom_api_key() {
        // 1. Default account API key retains default route & campaign when none specified
        let (default_route, default_campaign) =
            resolve_route_and_campaign(DEFAULT_SMS_API_KEY, None, None, None, None);
        assert_eq!(default_route.as_deref(), Some(DEFAULT_SMS_ROUTE_ID));
        assert_eq!(default_campaign.as_deref(), Some(DEFAULT_SMS_CAMPAIGN_ID));

        // 2. Custom API key without routeId/campaignId omits both (prevents "Pricing is not set for your account for this route")
        let (custom_route, custom_campaign) =
            resolve_route_and_campaign("36A1291E165814", None, None, None, None);
        assert_eq!(custom_route, None);
        assert_eq!(custom_campaign, None);

        // 3. Env overrides apply to custom API key when SettingResource omits them
        let (env_route, env_campaign) = resolve_route_and_campaign(
            "36A1291E165814",
            None,
            None,
            Some("30100".to_string()),
            Some("7788".to_string()),
        );
        assert_eq!(env_route.as_deref(), Some("30100"));
        assert_eq!(env_campaign.as_deref(), Some("7788"));

        // 4. SettingResource routeId/campaignId override env
        let (setting_route, setting_campaign) = resolve_route_and_campaign(
            "36A1291E165814",
            Some("20500"),
            Some("9901"),
            Some("30100".to_string()),
            Some("7788".to_string()),
        );
        assert_eq!(setting_route.as_deref(), Some("20500"));
        assert_eq!(setting_campaign.as_deref(), Some("9901"));

        // 5. Numeric and string routeId/campaignId in SettingResource.notifications.sms deserialize & send
        let (mock_url, req_rx) =
            spawn_mock_sms_server("200 OK", "SMS-SHOOT-ID/CUSTOM-ROUTE-OK").await;
        let setting = DatabaseResource {
            id: Some(1),
            resource_name: Some(ResourceName::SettingResource),
            inner_content: Some(serde_json::json!({
                "schemaVersion": 1,
                "notifications": {
                    "sms": {
                        "apiKey": "36A1291E165814",
                        "baseUrl": mock_url,
                        "enabled": true,
                        "provider": "samayasms",
                        "senderId": "Bit_Alert",
                        "routeId": 20500,
                        "campaignId": "9901",
                        "defaultCountryCode": "+977",
                        "templates": [
                            {
                                "id": "PAID",
                                "active": true,
                                "content": "Paid {vehicleNumber} {amount} {organizationName} {expiryDate}"
                            }
                        ]
                    }
                }
            })),
            external_content: None,
        };

        let sms_settings = extract_sms_notification_settings([&setting]).unwrap();
        assert_eq!(sms_settings.route_id.as_deref(), Some("20500"));
        assert_eq!(sms_settings.campaign_id.as_deref(), Some("9901"));

        let cfg = SmsConfig::from_setting_or_env(Some(&sms_settings));
        assert_eq!(cfg.api_key, "36A1291E165814");
        assert_eq!(cfg.route_id.as_deref(), Some("20500"));
        assert_eq!(cfg.campaign_id.as_deref(), Some("9901"));

        let mut cache = Cache::new();
        cache.write(ResourceName::SettingResource, 1, setting);

        let client = SmsClient::new(cfg);
        let tpl = SmsTemplate::new_paid("9861282176", "BA-01-PA-1", "Narayani", 500.0, "2083-12-30");
        let sent = client.send_template_with_cache(&tpl, &cache).await.unwrap();
        assert_eq!(sent.shoot_id, "CUSTOM-ROUTE-OK");

        let raw_http = req_rx.await.unwrap();
        assert!(raw_http.contains("key=36A1291E165814"));
        assert!(raw_http.contains("routeid=20500"));
        assert!(raw_http.contains("campaign=9901"));
        assert!(raw_http.contains("senderid=Bit_Alert"));
    }
}
