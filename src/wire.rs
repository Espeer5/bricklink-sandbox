//! Transport behavior audited against the current public BrickLink manual.
//! See docs/compatibility.md for evidence, ambiguity, and local-only behavior.
use super::*;

fn bad_body(message: impl Into<String>) -> ApiError {
    ApiError(
        StatusCode::BAD_REQUEST,
        "INVALID_REQUEST_BODY",
        message.into(),
    )
}

fn percent_decode(bytes: &[u8]) -> Result<Vec<u8>, ApiError> {
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes
                .get(i + 1..i + 3)
                .ok_or_else(|| bad_body("Incomplete percent escape"))?;
            let digit = |b: u8| (b as char).to_digit(16).map(|v| v as u8);
            let high = digit(hex[0]).ok_or_else(|| bad_body("Invalid percent escape"))?;
            let low = digit(hex[1]).ok_or_else(|| bad_body("Invalid percent escape"))?;
            decoded.push(high * 16 + low);
            i += 3;
        } else {
            // Raw URL-encoded JSON is not a form field: preserve literal '+'.
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    Ok(decoded)
}

pub(super) fn decode_body(bytes: &[u8], content_type: Option<&str>) -> Result<Value, ApiError> {
    if bytes.is_empty() {
        return Ok(json!({}));
    }
    let media = content_type
        .unwrap_or("application/json")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if !["application/json", "application/x-www-form-urlencoded"].contains(&media.as_str()) {
        return Err(ApiError(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "UNSUPPORTED_MEDIA_TYPE",
            "Use JSON or URL-encoded JSON".into(),
        ));
    }
    let trimmed = bytes.trim_ascii_start();
    let decoded = if trimmed.starts_with(b"%") {
        percent_decode(trimmed)?
    } else if media == "application/x-www-form-urlencoded"
        && !trimmed.starts_with(b"{")
        && !trimmed.starts_with(b"[")
    {
        // The data= wrapper is a compatibility extension, not specified by the manual.
        let checked = percent_decode(bytes)?;
        std::str::from_utf8(&checked).map_err(|_| bad_body("Body must be UTF-8"))?;
        let fields: Vec<_> = url::form_urlencoded::parse(bytes).into_owned().collect();
        if fields.len() != 1 || fields[0].0 != "data" {
            return Err(bad_body("Form wrapper must contain exactly one data field"));
        }
        fields[0].1.as_bytes().to_vec()
    } else {
        bytes.to_vec()
    };
    serde_json::from_slice(&decoded).map_err(|_| bad_body("Malformed JSON body"))
}

pub(super) fn check_method(method: &str, path: &str) -> Result<(), ApiError> {
    let route = path.strip_prefix(BASE).and_then(|p| p.strip_prefix('/'));
    let Some(route) = route else {
        return Ok(());
    };
    let parts: Vec<_> = route.trim_end_matches('/').split('/').collect();
    let allowed: &[&str] = match parts.as_slice() {
        ["inventories"] => &["GET", "POST"],
        ["orders"] => &["GET"],
        ["inventories", id] | ["orders", id] | ["orders", id, "items"] => {
            if id.parse::<u64>().is_err() {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "INVALID_URI",
                    "Resource ID must be an unsigned integer".into(),
                ));
            }
            if parts[0] == "inventories" {
                &["GET", "PUT", "DELETE"]
            } else {
                &["GET"]
            }
        }
        _ => return Ok(()),
    };
    if !allowed.contains(&method) {
        return Err(ApiError(
            StatusCode::METHOD_NOT_ALLOWED,
            "METHOD_NOT_ALLOWED",
            "Method is not implemented for this resource".into(),
        ));
    }
    Ok(())
}

pub(super) fn order_summary(order: &Value) -> Value {
    let mut summary = serde_json::Map::new();
    for key in [
        "order_id",
        "date_ordered",
        "seller_name",
        "store_name",
        "buyer_name",
        "total_count",
        "unique_count",
        "status",
    ] {
        summary.insert(key.into(), order[key].clone());
    }
    for (key, fields) in [
        (
            "payment",
            &["method", "status", "date_paid", "currency_code"][..],
        ),
        ("cost", &["subtotal", "grand_total", "currency_code"][..]),
    ] {
        let mut nested = serde_json::Map::new();
        for field in fields {
            if let Some(value) = order[key].get(field) {
                nested.insert((*field).into(), value.clone());
            }
        }
        summary.insert(key.into(), Value::Object(nested));
    }
    Value::Object(summary)
}

pub(super) fn order_item(lot: &Inventory, count: i64) -> Value {
    json!({"inventory_id": lot.inventory_id, "item": lot.fields.item, "color_id": lot.fields.color_id,
        "quantity": count, "new_or_used": lot.fields.new_or_used, "unit_price": lot.fields.unit_price,
        "unit_price_final": lot.fields.unit_price, "disp_unit_price": lot.fields.unit_price,
        "disp_unit_price_final": lot.fields.unit_price, "currency_code": "USD", "disp_currency_code": "USD",
        "remarks": lot.fields.remarks, "description": lot.fields.description})
}
