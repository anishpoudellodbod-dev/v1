use crate::resource::{DatabaseResource, ResourceName};
use crate::ServerData;
use actix_web::{web, HttpMessage, HttpRequest, HttpResponse};
use narayani_core::normalize_vehicle_number;
pub use narayani_vehicle_history::*;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub fn expand_vehicle_search_from_cache(cache: &crate::cache::Cache, query: &str) -> VehicleSearchExpansion {
    match cache.read_all(ResourceName::VehicleHistoryResource) {
        Ok(histories) => VehicleSearchExpansion::from_histories(histories, query),
        Err(_) => VehicleSearchExpansion::from_histories(std::iter::empty(), query),
    }
}

async fn load_vehicle_histories(s_data: &ServerData) -> Vec<DatabaseResource> {
    {
        let cache = s_data.cache.read().await;
        if let Ok(items) = cache.read_all(ResourceName::VehicleHistoryResource) {
            if !items.is_empty() || cache.is_fully_loaded(ResourceName::VehicleHistoryResource) {
                return items.into_iter().cloned().collect();
            }
        }
    }

    match s_data
        .db
        .read_all_resource(ResourceName::VehicleHistoryResource)
        .await
    {
        Ok(rows) => {
            let mut cache = s_data.cache.write().await;
            for row in &rows {
                if let Some(id) = row.id {
                    cache.write(ResourceName::VehicleHistoryResource, id, row.clone());
                }
            }
            cache.mark_fully_loaded(ResourceName::VehicleHistoryResource);
            rows
        }
        Err(_) => Vec::new(),
    }
}

/// Records a vehicle lifecycle or identity change onto the vehicle's `VehicleHistoryResource`
/// linked list in PostgreSQL and `Cache`.
#[allow(clippy::too_many_arguments)]
pub async fn record_vehicle_change(
    s_data: &ServerData,
    change_type: VehicleChangeType,
    db_id: Option<i32>,
    before_state: Option<&Value>,
    after_state: Option<&Value>,
    changed_by: Option<String>,
    reason: Option<String>,
    explicit_vehicle_uid: Option<&str>,
) -> Result<DatabaseResource, String> {
    let (b_vid, b_num, b_norm) = before_state
        .map(extract_vehicle_identifiers)
        .unwrap_or((None, None, None));
    let (a_vid, a_num, a_norm) = after_state
        .map(extract_vehicle_identifiers)
        .unwrap_or((None, None, None));

    let (effective_change_type, effective_reason) = if change_type == VehicleChangeType::Update {
        if let Some(detected) = detect_vehicle_number_only_change(before_state, after_state) {
            let auto_reason = reason.or_else(|| {
                Some(format!(
                    "Vehicle number changed only ({:?}): '{}' -> '{}'",
                    detected.kind,
                    detected.previous_number.as_deref().unwrap_or(""),
                    detected.new_number.as_deref().unwrap_or("")
                ))
            });
            (VehicleChangeType::VehicleNumberChange, auto_reason)
        } else {
            (change_type, reason)
        }
    } else {
        (change_type, reason)
    };

    let existing_histories = load_vehicle_histories(s_data).await;

    let matched = existing_histories.into_iter().find_map(|res| {
        let inner = res.inner_content.as_ref()?;
        let chain = VehicleHistoryChain::from_json_value(inner).ok()?;
        let hit = chain.matches_identifier(
            db_id,
            b_vid.as_deref(),
            b_norm.as_deref(),
            explicit_vehicle_uid,
        ) || chain.matches_identifier(
            db_id,
            a_vid.as_deref(),
            a_norm.as_deref(),
            explicit_vehicle_uid,
        ) || b_num
            .as_deref()
            .is_some_and(|n| chain.matches_vehicle_number_raw(n))
            || a_num
                .as_deref()
                .is_some_and(|n| chain.matches_vehicle_number_raw(n));
        if hit {
            Some((res, chain))
        } else {
            None
        }
    });

    if let Some((existing_res, mut chain)) = matched {
        if matches!(
            effective_change_type,
            VehicleChangeType::Update | VehicleChangeType::VehicleNumberChange
        ) && before_state.is_some()
            && before_state == after_state
        {
            return Ok(existing_res);
        }

        let Some(hist_id) = existing_res.id else {
            return Err("Matched VehicleHistoryResource is missing id".to_string());
        };

        chain.prepend_node(
            effective_change_type,
            db_id,
            before_state,
            after_state,
            changed_by,
            effective_reason,
        );

        let updated_res = DatabaseResource {
            id: Some(hist_id),
            resource_name: Some(ResourceName::VehicleHistoryResource),
            inner_content: Some(chain.to_json_value()),
            external_content: existing_res.external_content,
        };

        s_data
            .db
            .update_resource(
                ResourceName::VehicleHistoryResource,
                hist_id,
                updated_res.clone(),
            )
            .await
            .map_err(|e| format!("Failed to update VehicleHistoryResource {}: {:?}", hist_id, e))?;

        s_data
            .cache
            .write()
            .await
            .write(ResourceName::VehicleHistoryResource, hist_id, updated_res.clone());

        return Ok(updated_res);
    }

    let uid = explicit_vehicle_uid
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_else(generate_vehicle_uid);
    let mut chain = VehicleHistoryChain::new(uid);

    if matches!(
        effective_change_type,
        VehicleChangeType::Update
            | VehicleChangeType::Delete
            | VehicleChangeType::IdentityChange
            | VehicleChangeType::VehicleNumberChange
    ) && before_state.is_some()
    {
        chain.prepend_node(
            VehicleChangeType::Create,
            db_id,
            None,
            before_state,
            changed_by.clone(),
            Some("Initial state captured prior to change".to_string()),
        );
    }

    chain.prepend_node(
        effective_change_type,
        db_id,
        before_state,
        after_state,
        changed_by,
        effective_reason,
    );

    let mut new_res = DatabaseResource {
        id: None,
        resource_name: Some(ResourceName::VehicleHistoryResource),
        inner_content: Some(chain.to_json_value()),
        external_content: None,
    };

    let created_id = s_data
        .db
        .create_resource(ResourceName::VehicleHistoryResource, new_res.clone())
        .await
        .map_err(|e| format!("Failed to create VehicleHistoryResource: {:?}", e))?;

    new_res.id = Some(created_id);
    s_data
        .cache
        .write()
        .await
        .write(ResourceName::VehicleHistoryResource, created_id, new_res.clone());

    Ok(new_res)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordVehicleHistoryRequest {
    pub vehicle_uid: Option<String>,
    pub previous_db_id: Option<i32>,
    pub previous_vehicle_id: Option<String>,
    pub previous_vehicle_number: Option<String>,
    pub new_db_id: Option<i32>,
    pub new_vehicle_id: Option<String>,
    pub new_vehicle_number: Option<String>,
    pub snapshot: Option<Value>,
    pub change_type: Option<VehicleChangeType>,
    pub reason: Option<String>,
}

pub async fn list_vehicle_histories(
    req: HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    if let crate::permissions::PermissionCheckOutcome::Denied { message, .. } =
        crate::permissions::enforce_request_permission(
            &req,
            &s_data,
            "VehicleHistoryResource",
            crate::permissions::action::READ,
            crate::permissions::extract_branch_id_from_req(&req),
        )
        .await
    {
        return HttpResponse::Forbidden().body(message);
    }

    let histories = load_vehicle_histories(&s_data).await;
    HttpResponse::Ok().json(histories)
}

pub async fn get_vehicle_history_by_identifier(
    path: web::Path<String>,
    req: HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    if let crate::permissions::PermissionCheckOutcome::Denied { message, .. } =
        crate::permissions::enforce_request_permission(
            &req,
            &s_data,
            "VehicleHistoryResource",
            crate::permissions::action::READ,
            crate::permissions::extract_branch_id_from_req(&req),
        )
        .await
    {
        return HttpResponse::Forbidden().body(message);
    }

    let raw_identifier = path.into_inner();
    let trimmed = raw_identifier.trim();
    if trimmed.is_empty() {
        return HttpResponse::BadRequest().json(json!({
            "success": false,
            "message": "Vehicle identifier is required"
        }));
    }

    let parsed_db_id = trimmed.parse::<i32>().ok();
    let norm_num = normalize_vehicle_number(trimmed);
    let histories = load_vehicle_histories(&s_data).await;

    let found = histories.into_iter().find(|res| {
        res.inner_content
            .as_ref()
            .and_then(|inner| VehicleHistoryChain::from_json_value(inner).ok())
            .is_some_and(|chain| {
                chain.matches_identifier(
                    parsed_db_id,
                    Some(trimmed),
                    norm_num.as_deref(),
                    Some(trimmed),
                ) || chain.matches_vehicle_number_raw(trimmed)
            })
    });

    match found {
        Some(res) => HttpResponse::Ok().json(res),
        None => HttpResponse::NotFound().json(json!({
            "success": false,
            "message": format!("No VehicleHistoryResource found for identifier '{}'", trimmed)
        })),
    }
}

pub async fn record_vehicle_history_endpoint(
    body: web::Json<RecordVehicleHistoryRequest>,
    req: HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let payload = body.into_inner();

    if let crate::permissions::PermissionCheckOutcome::Denied { message, .. } =
        crate::permissions::enforce_request_permission(
            &req,
            &s_data,
            "VehicleHistoryResource",
            crate::permissions::action::UPDATE,
            crate::permissions::extract_branch_id_from_req(&req),
        )
        .await
    {
        return HttpResponse::Forbidden().body(message);
    }

    let actor = req
        .extensions()
        .get::<crate::auth::Claims>()
        .map(|c| c.sub.clone());

    let mut before_obj = serde_json::Map::new();
    if let Some(ref vid) = payload.previous_vehicle_id {
        before_obj.insert("VehicleId".to_string(), Value::String(vid.clone()));
    }
    if let Some(ref vnum) = payload.previous_vehicle_number {
        before_obj.insert("VehicleNumber".to_string(), Value::String(vnum.clone()));
    }
    let before_val = if before_obj.is_empty() {
        None
    } else {
        Some(Value::Object(before_obj))
    };

    let after_val = match payload.snapshot {
        Some(Value::Object(mut map)) => {
            if let Some(ref vid) = payload.new_vehicle_id {
                map.entry("VehicleId".to_string())
                    .or_insert_with(|| Value::String(vid.clone()));
            }
            if let Some(ref vnum) = payload.new_vehicle_number {
                map.entry("VehicleNumber".to_string())
                    .or_insert_with(|| Value::String(vnum.clone()));
            }
            Some(Value::Object(map))
        }
        Some(other) => Some(other),
        None => {
            let mut after_obj = serde_json::Map::new();
            if let Some(ref vid) = payload.new_vehicle_id {
                after_obj.insert("VehicleId".to_string(), Value::String(vid.clone()));
            }
            if let Some(ref vnum) = payload.new_vehicle_number {
                after_obj.insert("VehicleNumber".to_string(), Value::String(vnum.clone()));
            }
            if after_obj.is_empty() {
                None
            } else {
                Some(Value::Object(after_obj))
            }
        }
    };

    if before_val.is_none()
        && after_val.is_none()
        && payload.vehicle_uid.as_deref().map(str::trim).unwrap_or("").is_empty()
    {
        return HttpResponse::BadRequest().json(json!({
            "success": false,
            "message": "Provide at least one vehicle identifier or snapshot"
        }));
    }

    // If previous_db_id was provided alongside a different new_db_id, match using previous_db_id first
    // so both DB IDs get registered on the same chain.
    if let (Some(prev_db), Some(new_db)) = (payload.previous_db_id, payload.new_db_id) {
        if prev_db != new_db {
            if let Ok(mut existing) = record_vehicle_change(
                &s_data,
                VehicleChangeType::Update,
                Some(prev_db),
                before_val.as_ref(),
                before_val.as_ref(),
                actor.clone(),
                None,
                payload.vehicle_uid.as_deref(),
            )
            .await
            {
                let uid = existing
                    .inner_content
                    .as_ref()
                    .and_then(|v| v.get("vehicleUid"))
                    .and_then(Value::as_str)
                    .map(String::from);
                let _ = &mut existing;
                return match record_vehicle_change(
                    &s_data,
                    payload
                        .change_type
                        .unwrap_or(VehicleChangeType::IdentityChange),
                    Some(new_db),
                    before_val.as_ref(),
                    after_val.as_ref(),
                    actor,
                    payload.reason,
                    uid.as_deref().or(payload.vehicle_uid.as_deref()),
                )
                .await
                {
                    Ok(res) => HttpResponse::Ok().json(res),
                    Err(err) => HttpResponse::InternalServerError().body(err),
                };
            }
        }
    }

    let effective_db_id = payload.new_db_id.or(payload.previous_db_id);
    let change_type = payload.change_type.unwrap_or_else(|| {
        if detect_vehicle_number_only_change(before_val.as_ref(), after_val.as_ref()).is_some() {
            VehicleChangeType::VehicleNumberChange
        } else if before_val.is_some() {
            VehicleChangeType::IdentityChange
        } else {
            VehicleChangeType::Update
        }
    });

    match record_vehicle_change(
        &s_data,
        change_type,
        effective_db_id,
        before_val.as_ref(),
        after_val.as_ref(),
        actor,
        payload.reason,
        payload.vehicle_uid.as_deref(),
    )
    .await
    {
        Ok(res) => HttpResponse::Ok().json(res),
        Err(err) => HttpResponse::InternalServerError().body(err),
    }
}

pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.route("", web::get().to(list_vehicle_histories))
        .route("/record", web::post().to(record_vehicle_history_endpoint))
        .route(
            "/{identifier}",
            web::get().to(get_vehicle_history_by_identifier),
        );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::test_helpers::{create_mock_server_data_with_state, MockDbState};
    use crate::auth::Claims;
    use crate::roles::Roles;
    use actix_web::{http::StatusCode, test as actix_test, App};

    #[test]
    fn test_vehicle_history_linked_list_json_value_across_id_and_number_changes() {
        let mut chain = VehicleHistoryChain::new("VHIST-TEST-1".to_string());

        let v1 = json!({
            "VehicleId": "1001",
            "VehicleNumber": "BA-01-PA-1001",
            "VehicleType": "Truck"
        });
        chain.prepend_node(
            VehicleChangeType::Create,
            Some(10),
            None,
            Some(&v1),
            Some("admin@narayani.com".to_string()),
            Some("Initial registration".to_string()),
        );

        // Both VehicleId AND VehicleNumber change simultaneously
        let v2 = json!({
            "VehicleId": "9999",
            "VehicleNumber": "GA-02-KHA-9999",
            "VehicleType": "Container Truck"
        });
        chain.prepend_node(
            VehicleChangeType::IdentityChange,
            Some(10),
            Some(&v1),
            Some(&v2),
            Some("admin@narayani.com".to_string()),
            Some("Re-registered with new plate and ID".to_string()),
        );

        let as_json = chain.to_json_value();

        // Verify explicit linked list structure in JSON Value: head -> next -> null
        assert_eq!(as_json["vehicleUid"], "VHIST-TEST-1");
        assert_eq!(as_json["nodeCount"], 2);
        assert_eq!(as_json["head"]["sequence"], 2);
        assert_eq!(as_json["head"]["vehicleId"], "9999");
        assert_eq!(as_json["head"]["vehicleNumber"], "GA-02-KHA-9999");
        assert_eq!(as_json["head"]["next"]["sequence"], 1);
        assert_eq!(as_json["head"]["next"]["vehicleId"], "1001");
        assert_eq!(as_json["head"]["next"]["vehicleNumber"], "BA-01-PA-1001");
        assert!(as_json["head"]["next"]["next"].is_null());

        let diffs = as_json["head"]["changedFields"].as_array().unwrap();
        assert_eq!(diffs.len(), 3);

        // Roundtrip and lookup by both old and new ID/number/uid
        let parsed = VehicleHistoryChain::from_json_value(&as_json).unwrap();
        assert_eq!(parsed.traverse_nodes().len(), 2);
        assert!(parsed.matches_identifier(None, Some("1001"), None, None));
        assert!(parsed.matches_identifier(None, Some("9999"), None, None));
        assert!(parsed.matches_identifier(None, None, Some("BA01PA1001"), None));
        assert!(parsed.matches_identifier(None, None, Some("GA02KHA9999"), None));
        assert!(parsed.matches_identifier(Some(10), None, None, None));
        assert!(parsed.matches_identifier(None, None, None, Some("VHIST-TEST-1")));

        let (db_ids, vids, nums) = extract_all_identifiers_from_history(&as_json);
        assert_eq!(db_ids, vec![10]);
        assert!(vids.contains(&"1001".to_string()));
        assert!(vids.contains(&"9999".to_string()));
        assert!(nums.contains(&"BA01PA1001".to_string()));
        assert!(nums.contains(&"GA02KHA9999".to_string()));
    }

    #[actix_web::test]
    async fn test_vehicle_actions_and_history_endpoints_track_full_lifecycle() {
        let state = MockDbState::default();
        let (s_data, db_state) = create_mock_server_data_with_state(state).await;

        let make_admin_req = || {
            let req = actix_test::TestRequest::default().to_http_request();
            req.extensions_mut().insert(Claims {
                sub: "admin@narayani.com".to_string(),
                exp: 9999999999,
                iat: 1000000000,
                role: Roles::Admin,
                user_id: Some(1),
                jti: None,
                role_name: Some("Admin".to_string()),
                tenant_id: Some("loadboard-sthaniya".to_string()),
            });
            req
        };

        // 1. Create VehicleResource via actions::write -> creates VehicleHistoryResource node 1
        let create_resp = crate::actions::write(
            web::Path::from(ResourceName::VehicleResource),
            web::Json(DatabaseResource {
                id: None,
                resource_name: Some(ResourceName::VehicleResource),
                inner_content: Some(json!({
                    "VehicleId": "35930",
                    "VehicleNumber": "State-3-G-KK-8211",
                    "VehicleType": "Truck"
                })),
                external_content: None,
            }),
            make_admin_req(),
            s_data.clone(),
        )
        .await;
        assert_eq!(create_resp.status(), StatusCode::OK);
        let created_vehicle: DatabaseResource = serde_json::from_slice(
            &actix_web::body::to_bytes(create_resp.into_body())
                .await
                .unwrap(),
        )
        .unwrap();
        let v_db_id = created_vehicle.id.unwrap();

        // 2. Update VehicleResource changing BOTH VehicleId AND VehicleNumber -> prepends node 2
        let update_resp = crate::actions::update(
            web::Path::from((ResourceName::VehicleResource, v_db_id)),
            web::Json(DatabaseResource {
                id: Some(v_db_id),
                resource_name: Some(ResourceName::VehicleResource),
                inner_content: Some(json!({
                    "VehicleId": "88888",
                    "VehicleNumber": "Province-3-01-KA-8888",
                    "VehicleType": "Heavy Truck"
                })),
                external_content: None,
            }),
            make_admin_req(),
            s_data.clone(),
        )
        .await;
        assert_eq!(update_resp.status(), StatusCode::OK);

        // 3. Delete VehicleResource -> prepends node 3 (DELETE) to the SAME linked list
        let delete_resp = crate::actions::delete(
            web::Path::from((ResourceName::VehicleResource, v_db_id)),
            s_data.clone(),
            make_admin_req(),
        )
        .await;
        assert_eq!(delete_resp.status(), StatusCode::OK);

        // Verify exactly ONE VehicleHistoryResource exists in DB for this single vehicle
        {
            let guard = db_state.lock().await;
            let hist_rows = guard.tables.get("vehiclehistoryresource").unwrap();
            assert_eq!(hist_rows.len(), 1);
            let inner = hist_rows[0].inner_content.as_ref().unwrap();
            assert_eq!(inner["nodeCount"], 3);
            assert_eq!(inner["head"]["changeType"], "DELETE");
            assert_eq!(inner["head"]["next"]["changeType"], "UPDATE");
            assert_eq!(inner["head"]["next"]["vehicleId"], "88888");
            assert_eq!(inner["head"]["next"]["next"]["changeType"], "CREATE");
            assert_eq!(inner["head"]["next"]["next"]["vehicleId"], "35930");
            assert!(inner["head"]["next"]["next"]["next"].is_null());
        }

        // 4. Query HTTP endpoints using old ID, new ID, old number, and new number -> all return the same vehicleUid
        let app = actix_test::init_service(
            App::new()
                .app_data(s_data.clone())
                .service(web::scope("/api/v1/vehicles/history").configure(configure_routes)),
        )
        .await;

        let mut uids = Vec::new();
        for identifier in ["35930", "88888", "State-3-G-KK-8211", "Province-3-01-KA-8888"] {
            let resp = actix_test::call_service(
                &app,
                actix_test::TestRequest::get()
                    .uri(&format!("/api/v1/vehicles/history/{}", identifier))
                    .to_request(),
            )
            .await;
            assert_eq!(resp.status(), StatusCode::OK, "lookup failed for {identifier}");
            let hist_res: DatabaseResource = actix_test::read_body_json(resp).await;
            let uid = hist_res.inner_content.unwrap()["vehicleUid"]
                .as_str()
                .unwrap()
                .to_string();
            uids.push(uid);
        }
        assert!(uids.windows(2).all(|w| w[0] == w[1]));

        // 5. Explicitly link another identity change via POST /api/v1/vehicles/history/record
        let record_resp = actix_test::call_service(
            &app,
            actix_test::TestRequest::post()
                .uri("/api/v1/vehicles/history/record")
                .set_json(&json!({
                    "previousVehicleId": "88888",
                    "newVehicleId": "99999",
                    "newVehicleNumber": "Bagmati-01-GA-9999",
                    "reason": "Manual number plate transfer"
                }))
                .to_request(),
        )
        .await;
        assert_eq!(record_resp.status(), StatusCode::OK);
        let updated_hist: DatabaseResource = actix_test::read_body_json(record_resp).await;
        let updated_inner = updated_hist.inner_content.unwrap();
        assert_eq!(updated_inner["vehicleUid"], uids[0]);
        assert_eq!(updated_inner["nodeCount"], 4);
        assert_eq!(updated_inner["head"]["changeType"], "IDENTITY_CHANGE");
        assert_eq!(updated_inner["head"]["vehicleId"], "99999");
        assert_eq!(updated_inner["head"]["next"]["changeType"], "DELETE");
    }

    #[test]
    fn test_detect_vehicle_number_only_change_classifications() {
        let base = json!({
            "VehicleId": "24631",
            "VehicleNumber": "Ga Pra 15-23-KHA-5555",
            "VehicleType": "Truck"
        });
        let alias = json!({
            "VehicleId": "24631",
            "VehicleNumber": "Province-4-15-23-KHA-5555",
            "VehicleType": "Truck"
        });
        let detected_alias =
            detect_vehicle_number_only_change(Some(&base), Some(&alias)).expect("alias change");
        assert_eq!(
            detected_alias.kind,
            VehicleNumberChangeKind::CanonicalFormatAlias
        );
        assert_eq!(
            detected_alias.previous_canonical.as_deref(),
            Some("Province-4-15-23-KHA-5555")
        );
        assert_eq!(
            detected_alias.new_canonical.as_deref(),
            Some("Province-4-15-23-KHA-5555")
        );

        let zonal_old = json!({
            "VehicleId": "1001",
            "VehicleNumber": "BA-23-KHA-5555",
            "VehicleType": "Truck"
        });
        let provincial_new = json!({
            "VehicleId": "1001",
            "VehicleNumber": "Province-3-01-23-KHA-5555",
            "VehicleType": "Truck"
        });
        let detected_z2p = detect_vehicle_number_only_change(Some(&zonal_old), Some(&provincial_new))
            .expect("zone-to-province transition");
        assert_eq!(
            detected_z2p.kind,
            VehicleNumberChangeKind::ZoneToProvinceTransition
        );

        let plate_changed = json!({
            "VehicleId": "1001",
            "VehicleNumber": "NA-04-KHA-9999",
            "VehicleType": "Truck"
        });
        let detected_plate =
            detect_vehicle_number_only_change(Some(&zonal_old), Some(&plate_changed))
                .expect("plate number changed");
        assert_eq!(
            detected_plate.kind,
            VehicleNumberChangeKind::PlateNumberChanged
        );

        // If another field also changed, it must NOT be classified as vehicle-number-only
        let multi_field_change = json!({
            "VehicleId": "1001",
            "VehicleNumber": "Province-3-01-23-KHA-5555",
            "VehicleType": "Heavy Truck"
        });
        assert!(detect_vehicle_number_only_change(Some(&zonal_old), Some(&multi_field_change)).is_none());

        let id_also_changed = json!({
            "VehicleId": "9999",
            "VehicleNumber": "Province-3-01-23-KHA-5555",
            "VehicleType": "Truck"
        });
        assert!(detect_vehicle_number_only_change(Some(&zonal_old), Some(&id_also_changed)).is_none());
    }

    #[actix_web::test]
    async fn test_actions_update_vehicle_number_only_detects_history_without_mutating_stored_format() {
        let state = MockDbState::default();
        let (s_data, db_state) = create_mock_server_data_with_state(state).await;

        let make_admin_req = || {
            let req = actix_test::TestRequest::default().to_http_request();
            req.extensions_mut().insert(Claims {
                sub: "admin@narayani.com".to_string(),
                exp: 9999999999,
                iat: 1000000000,
                role: Roles::Admin,
                user_id: Some(1),
                jti: None,
                role_name: Some("Admin".to_string()),
                tenant_id: Some("loadboard-sthaniya".to_string()),
            });
            req
        };

        // 1. Create vehicle with zonal plate
        let create_resp = crate::actions::write(
            web::Path::from(ResourceName::VehicleResource),
            web::Json(DatabaseResource {
                id: None,
                resource_name: Some(ResourceName::VehicleResource),
                inner_content: Some(json!({
                    "VehicleId": "77001",
                    "VehicleNumber": "BA-23-KHA-5555",
                    "VehicleType": "Truck"
                })),
                external_content: None,
            }),
            make_admin_req(),
            s_data.clone(),
        )
        .await;
        assert_eq!(create_resp.status(), StatusCode::OK);
        let created_vehicle: DatabaseResource = serde_json::from_slice(
            &actix_web::body::to_bytes(create_resp.into_body())
                .await
                .unwrap(),
        )
        .unwrap();
        let v_db_id = created_vehicle.id.unwrap();

        // 2. Update ONLY VehicleNumber -> should record VEHICLE_NUMBER_CHANGE without rewriting stored VehicleNumber format
        let update_resp = crate::actions::update(
            web::Path::from((ResourceName::VehicleResource, v_db_id)),
            web::Json(DatabaseResource {
                id: Some(v_db_id),
                resource_name: Some(ResourceName::VehicleResource),
                inner_content: Some(json!({
                    "VehicleId": "77001",
                    "VehicleNumber": "Ba Pra 01-23-KHA-5555",
                    "VehicleType": "Truck"
                })),
                external_content: None,
            }),
            make_admin_req(),
            s_data.clone(),
        )
        .await;
        assert_eq!(update_resp.status(), StatusCode::OK);

        // Verify stored VehicleResource preserves exact submitted format ("Ba Pra 01-23-KHA-5555")
        {
            let cache = s_data.cache.read().await;
            let cached_vehicle = cache.read(ResourceName::VehicleResource, v_db_id).unwrap();
            assert_eq!(
                cached_vehicle.inner_content.as_ref().unwrap()["VehicleNumber"],
                "Ba Pra 01-23-KHA-5555"
            );
        }

        // Verify VehicleHistoryResource captured VEHICLE_NUMBER_CHANGE & ZONE_TO_PROVINCE_TRANSITION
        {
            let guard = db_state.lock().await;
            let hist_rows = guard.tables.get("vehiclehistoryresource").unwrap();
            assert_eq!(hist_rows.len(), 1);
            let inner = hist_rows[0].inner_content.as_ref().unwrap();
            assert_eq!(inner["nodeCount"], 2);
            assert_eq!(inner["head"]["changeType"], "VEHICLE_NUMBER_CHANGE");
            assert_eq!(inner["head"]["numberChangeKind"], "ZONE_TO_PROVINCE_TRANSITION");
            assert_eq!(inner["head"]["canonicalVehicleNumber"], "Province-3-1-23-KHA-5555");
            let chain = VehicleHistoryChain::from_json_value(inner).unwrap();
            assert!(chain.has_vehicle_number_only_change());
        }

        // Verify HTTP history lookup works by canonical alias of the new plate ("Province-3-01-23-KHA-5555")
        // as well as old zonal plate ("Bagmati 23 KHA 5555")
        let app = actix_test::init_service(
            App::new()
                .app_data(s_data.clone())
                .service(web::scope("/api/v1/vehicles/history").configure(configure_routes)),
        )
        .await;

        for alias in ["Province-3-01-23-KHA-5555", "Bagmati 23 KHA 5555"] {
            let encoded = alias.replace(' ', "%20");
            let resp = actix_test::call_service(
                &app,
                actix_test::TestRequest::get()
                    .uri(&format!("/api/v1/vehicles/history/{}", encoded))
                    .to_request(),
            )
            .await;
            assert_eq!(resp.status(), StatusCode::OK, "alias lookup failed for {alias}");
        }
    }
}
