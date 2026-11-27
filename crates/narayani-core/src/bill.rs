use crate::extract_string_or_int;
use serde_json::Value;

/// Returns true if a BillResource inner_content represents an ignored/voided/cancelled bill.
pub fn is_bill_ignored(inner: &Value) -> bool {
    if let Some(status) = inner.get("status").and_then(Value::as_str) {
        let s = status.trim();
        if s.eq_ignore_ascii_case("DELETED")
            || s.eq_ignore_ascii_case("AMENDED")
            || s.eq_ignore_ascii_case("ROLLED_BACK")
            || s.eq_ignore_ascii_case("CANCELLED")
            || s.eq_ignore_ascii_case("CANCELED")
            || s.eq_ignore_ascii_case("VOID")
        {
            return true;
        }
    }

    if let Some(history) = inner.get("auditHistory").and_then(Value::as_array) {
        if history.iter().any(|entry| {
            entry
                .get("action")
                .and_then(Value::as_str)
                .is_some_and(|a| {
                    let action = a.trim();
                    action.eq_ignore_ascii_case("CANCELLED")
                        || action.eq_ignore_ascii_case("CANCELED")
                        || action.eq_ignore_ascii_case("VOID")
                })
        }) {
            return true;
        }
    }

    false
}

/// Extracts `(vehicle_id, vehicle_number)` from a BillResource `inner_content`.
pub fn extract_bill_vehicle<'a>(inner: &'a Value) -> (Option<String>, Option<&'a str>) {
    let vehicle_obj = inner.get("vehicle");
    let v_id = vehicle_obj
        .and_then(|v| {
            extract_string_or_int(v.get("id"))
                .or_else(|| extract_string_or_int(v.get("vehicleId")))
                .or_else(|| extract_string_or_int(v.get("VehicleId")))
        })
        .or_else(|| extract_string_or_int(inner.get("vehicleId")));

    let v_num = vehicle_obj
        .and_then(|v| {
            v.get("vehicleNumber")
                .or_else(|| v.get("VehicleNumber"))
                .and_then(Value::as_str)
        })
        .or_else(|| inner.get("vehicleNumber").and_then(Value::as_str));

    (v_id, v_num)
}
