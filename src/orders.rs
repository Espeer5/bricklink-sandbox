//! Order metadata and atomic lifecycle simulation. Upstream update contracts and
//! explicit simulator policies are documented in docs/order-lifecycle.md.
use super::*;
use chrono::DateTime;

pub(super) const STATUSES: &[&str] = &[
    "PENDING",
    "UPDATED",
    "PROCESSING",
    "READY",
    "PAID",
    "PACKED",
    "SHIPPED",
    "RECEIVED",
    "COMPLETED",
    "OCR",
    "NPB",
    "NPX",
    "NRS",
    "NSS",
    "CANCELLED",
];
const PAYMENTS: &[&str] = &[
    "None",
    "Sent",
    "Received",
    "Paid",
    "Clearing",
    "Returned",
    "Bounced",
    "Completed",
];
const COSTS: &[&str] = &["shipping", "insurance", "etc1", "etc2", "credit", "coupon"];

pub(super) fn empty_details() -> Value {
    json!({})
}

#[derive(Clone)]
pub(super) struct OrderState {
    pub direction: String,
    managed: bool,
    held: BTreeMap<u64, i64>,
    refunded: Decimal,
    restitution_started: bool,
    receipts: BTreeMap<String, (Value, Value)>,
}

fn denied(message: impl Into<String>) -> ApiError {
    ApiError(
        StatusCode::UNPROCESSABLE_ENTITY,
        "RESOURCE_UPDATE_NOT_ALLOWED",
        message.into(),
    )
}

fn object<'a>(
    value: &'a Value,
    path: &str,
) -> Result<&'a serde_json::Map<String, Value>, ApiError> {
    value
        .as_object()
        .ok_or_else(|| invalid(format!("{path}: expected an object")))
}

fn string<'a>(value: &'a Value, path: &str) -> Result<&'a str, ApiError> {
    value
        .as_str()
        .ok_or_else(|| invalid(format!("{path}: expected a string")))
}

fn date(value: &Value, path: &str) -> Result<Value, ApiError> {
    let value = string(value, path)?;
    let date = DateTime::parse_from_rfc3339(value)
        .map_err(|_| invalid(format!("{path}: expected RFC3339 timestamp")))?;
    if date.timestamp_subsec_nanos() % 1_000_000 != 0 {
        return Err(invalid(format!("{path}: precision exceeds milliseconds")));
    }
    Ok(json!(
        date.with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Millis, true)
    ))
}

fn decimal(value: &Value, path: &str) -> Result<Decimal, ApiError> {
    Decimal::from_str_exact(string(value, path)?)
        .map_err(|_| invalid(format!("{path}: invalid decimal")))
}

fn amount(value: &Value, path: &str) -> Result<Value, ApiError> {
    Ok(json!(
        price(string(value, path)?).map_err(|e| invalid(format!("{path}: {}", e.2)))?
    ))
}

fn line_counts(lines: &[OrderLine]) -> Result<BTreeMap<u64, i64>, ApiError> {
    let mut result = BTreeMap::<u64, i64>::new();
    for line in lines {
        if !(1..=1_000_000_000).contains(&line.quantity) {
            return Err(invalid("items.quantity: expected 1..1000000000"));
        }
        let count = result.entry(line.inventory_id).or_default();
        *count = count
            .checked_add(line.quantity)
            .ok_or_else(|| invalid("items.quantity: overflow"))?;
    }
    Ok(result)
}

fn item_map(batches: &Value) -> BTreeMap<u64, Value> {
    batches
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|batch| batch.as_array().unwrap())
        .map(|item| (item["inventory_id"].as_u64().unwrap(), item.clone()))
        .collect()
}

fn settled(order: &Value) -> bool {
    ["Received", "Clearing", "Paid", "Completed", "Returned"]
        .contains(&order["payment"]["status"].as_str().unwrap_or(""))
        || ["PAID", "PACKED", "SHIPPED", "RECEIVED", "COMPLETED"]
            .contains(&order["status"].as_str().unwrap_or(""))
}

fn recompute(order: &mut Value, batches: &Value) -> Result<(), ApiError> {
    let items = item_map(batches);
    let mut subtotal = Decimal::ZERO;
    let mut count = 0i64;
    for item in items.values() {
        let qty = item["quantity"].as_i64().unwrap();
        subtotal = subtotal
            .checked_add(
                decimal(&item["unit_price_final"], "unit_price_final")? * Decimal::from(qty),
            )
            .ok_or_else(|| invalid("Subtotal overflow"))?;
        count = count
            .checked_add(qty)
            .ok_or_else(|| invalid("Item count overflow"))?;
    }
    let mut total = subtotal;
    for field in COSTS {
        if let Some(value) = order["cost"].get(*field) {
            let value = decimal(value, field)?;
            total = if ["credit", "coupon"].contains(field) {
                total.checked_sub(value)
            } else {
                total.checked_add(value)
            }
            .ok_or_else(|| invalid("Grand total overflow"))?;
        }
    }
    if total < Decimal::ZERO {
        return Err(invalid("Discounts exceed order total"));
    }
    order["cost"]["subtotal"] = json!(format!("{subtotal:.4}"));
    order["cost"]["grand_total"] = json!(format!("{total:.4}"));
    order["total_count"] = json!(count);
    order["unique_count"] = json!(items.len());
    Ok(())
}

fn validate_shipping(shipping: &Value) -> Result<Value, ApiError> {
    let mut result = json!({});
    for (key, value) in object(shipping, "shipping")? {
        let path = format!("shipping.{key}");
        result[key] = match key.as_str() {
            "date_shipped" => date(value, &path)?,
            "method_id" if value.is_u64() || value.is_string() => value.clone(),
            "method" | "tracking_no" | "tracking_link" => json!(string(value, &path)?),
            "address" => {
                let mut address = json!({});
                for (field, value) in object(value, &path)? {
                    let path = format!("shipping.address.{field}");
                    if field == "name" {
                        let mut name = json!({});
                        for (part, value) in object(value, &path)? {
                            if !["full", "first", "last"].contains(&part.as_str()) {
                                return Err(invalid(format!("{path}.{part}: unsupported field")));
                            }
                            name[part] = json!(string(value, &format!("{path}.{part}"))?);
                        }
                        address[field] = name;
                    } else if [
                        "full",
                        "address1",
                        "address2",
                        "country_code",
                        "city",
                        "state",
                        "postal_code",
                        "phone_number",
                    ]
                    .contains(&field.as_str())
                    {
                        address[field] = json!(string(value, &path)?);
                    } else {
                        return Err(invalid(format!("{path}: unsupported field")));
                    }
                }
                address
            }
            _ => return Err(invalid(format!("{path}: unsupported field or type"))),
        };
    }
    Ok(result)
}

impl Store {
    pub(super) fn configure_order(
        &mut self,
        id: u64,
        details: Value,
        consumed: bool,
    ) -> Result<(), ApiError> {
        let details = object(&details, "details")?;
        let mut order = self.orders[&id].clone();
        let mut direction = "in".to_owned();
        let mut managed = consumed;
        let mut currency = "USD".to_owned();
        for (key, value) in details {
            match key.as_str() {
                "buyer_name" | "buyer_email" | "seller_name" | "store_name" | "remarks" => {
                    let text = string(value, key)?;
                    if text.is_empty() && key != "remarks" {
                        return Err(invalid(format!("details.{key}: must not be empty")));
                    }
                    order[key] = json!(text);
                }
                "date_ordered" | "date_status_changed" => {
                    order[key] = date(value, key)?;
                }
                "status" => {
                    let status = string(value, "status")?;
                    if !STATUSES.contains(&status) {
                        return Err(invalid("details.status: unsupported status"));
                    }
                    order[key] = value.clone();
                }
                "is_filed" => {
                    order[key] = json!(
                        value
                            .as_bool()
                            .ok_or_else(|| invalid("details.is_filed: expected boolean"))?
                    );
                }
                "direction" => {
                    direction = string(value, "direction")?.into();
                    if !["in", "out"].contains(&direction.as_str()) {
                        return Err(invalid("details.direction: expected in or out"));
                    }
                }
                "currency_code" => {
                    currency = string(value, "currency_code")?.into();
                    if currency.len() != 3 || !currency.bytes().all(|c| c.is_ascii_uppercase()) {
                        return Err(invalid(
                            "details.currency_code: expected three uppercase ASCII letters",
                        ));
                    }
                }
                "inventory_effects" => match string(value, "inventory_effects")? {
                    "reserved" => managed = true,
                    "none" if !consumed => managed = false,
                    _ => {
                        return Err(invalid(
                            "details.inventory_effects: historical fixtures allow none/reserved; purchases reserve stock",
                        ));
                    }
                },
                "shipping" => {
                    order[key] = validate_shipping(value)?;
                }
                "payment" => {
                    for (field, value) in object(value, "payment")? {
                        match field.as_str() {
                            "method" => {
                                order["payment"][field] = json!(string(value, "payment.method")?);
                            }
                            "status" => {
                                if !PAYMENTS.contains(&string(value, "payment.status")?) {
                                    return Err(invalid("payment.status: unsupported status"));
                                }
                                order["payment"][field] = value.clone();
                            }
                            "date_paid" => {
                                order["payment"][field] = date(value, "payment.date_paid")?;
                            }
                            _ => {
                                return Err(invalid(format!(
                                    "payment.{field}: unsupported field; currency uses details.currency_code"
                                )));
                            }
                        }
                    }
                }
                "cost" => {
                    for (field, value) in object(value, "cost")? {
                        if !COSTS.contains(&field.as_str()) {
                            return Err(invalid(format!("cost.{field}: unsupported charge")));
                        }
                        order["cost"][field] = amount(value, &format!("cost.{field}"))?;
                    }
                }
                _ => return Err(invalid(format!("details.{key}: unsupported field"))),
            }
        }
        if direction == "out" && managed {
            return Err(invalid("Outgoing orders cannot reserve local inventory"));
        }
        if details.contains_key("date_ordered") && !details.contains_key("date_status_changed") {
            order["date_status_changed"] = order["date_ordered"].clone();
        }
        order["cost"]["currency_code"] = json!(currency);
        order["payment"]["currency_code"] = json!(currency);
        let mut batches = self.order_items[&id].clone();
        for batch in batches.as_array_mut().unwrap() {
            for item in batch.as_array_mut().unwrap() {
                item["currency_code"] = json!(currency);
                item["disp_currency_code"] = json!(currency);
            }
        }
        recompute(&mut order, &batches)?;
        let held = if managed {
            item_map(&batches)
                .into_iter()
                .map(|(id, item)| (id, item["quantity"].as_i64().unwrap()))
                .collect()
        } else {
            BTreeMap::new()
        };
        self.order_state.insert(
            id,
            OrderState {
                direction,
                managed,
                held,
                refunded: Decimal::ZERO,
                restitution_started: false,
                receipts: BTreeMap::new(),
            },
        );
        self.orders.insert(id, order);
        self.order_items.insert(id, batches);
        Ok(())
    }

    pub(super) fn order_effects(&self, id: u64) -> ApiResult {
        let state = self.order_state.get(&id).ok_or_else(missing)?;
        Ok((
            StatusCode::OK,
            json!({"direction": state.direction, "inventory_managed": state.managed,
            "remaining_reserved": state.held.iter().map(|(id, qty)| json!({"inventory_id":id,"quantity":qty})).collect::<Vec<_>>(),
            "refunded": format!("{:.4}", state.refunded), "currency_code": self.orders[&id]["cost"]["currency_code"]}),
        ))
    }

    pub(super) fn order_action(&mut self, id: u64, action: &str, body: Value) -> ApiResult {
        self.orders.get(&id).ok_or_else(missing)?;
        let mut staged = self.clone();
        let result = staged.order_action_inner(id, action, body)?;
        *self = staged;
        Ok(result)
    }

    fn order_action_inner(&mut self, id: u64, action: &str, body: Value) -> ApiResult {
        match action {
            "update" => self.update_order(id, body),
            "status" | "payment_status" => self.patch_order(id, action, body),
            "items" => self.edit_order(id, body),
            "cancel" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Cancel {
                    #[serde(default = "yes")]
                    restock: bool,
                }
                fn yes() -> bool {
                    true
                }
                let input: Cancel =
                    serde_json::from_value(body).map_err(|e| invalid(e.to_string()))?;
                if input.restock {
                    let lines = self.order_state[&id]
                        .held
                        .iter()
                        .map(|(id, qty)| OrderLine {
                            inventory_id: *id,
                            quantity: *qty,
                        })
                        .collect::<Vec<_>>();
                    self.return_stock(id, &lines)?;
                }
                let order = self.orders.get_mut(&id).unwrap();
                if order["status"] != "CANCELLED" {
                    order["status"] = json!("CANCELLED");
                    order["date_status_changed"] = json!(self.clock.now());
                }
                Ok((StatusCode::OK, order.clone()))
            }
            "restock" | "refund" => self.restitution(id, action, body),
            _ => Err(missing()),
        }
    }

    fn update_order(&mut self, id: u64, body: Value) -> ApiResult {
        let body = object(&body, "order")?;
        let mut order = self.orders[&id].clone();
        // The current API manual explicitly ignores non-writable fields.
        if let Some(value) = body.get("remarks") {
            order["remarks"] = json!(string(value, "remarks")?);
        }
        if let Some(value) = body.get("is_filed") {
            order["is_filed"] = json!(
                value
                    .as_bool()
                    .ok_or_else(|| invalid("is_filed: expected boolean"))?
            );
        }
        if let Some(value) = body.get("shipping") {
            let filtered = object(value, "shipping")?
                .iter()
                .filter(|(key, _)| {
                    ["date_shipped", "tracking_no", "tracking_link", "method_id"]
                        .contains(&key.as_str())
                })
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            let checked = validate_shipping(&Value::Object(filtered))?;
            for (key, value) in checked.as_object().unwrap() {
                order["shipping"][key] = value.clone();
            }
        }
        if !settled(&order) && self.order_state[&id].refunded == Decimal::ZERO {
            if let Some(value) = body.get("cost") {
                for (field, value) in object(value, "cost")? {
                    if ["shipping", "insurance", "etc1", "etc2", "credit"].contains(&field.as_str())
                    {
                        order["cost"][field] = amount(value, &format!("cost.{field}"))?;
                    }
                }
            }
        }
        recompute(&mut order, &self.order_items[&id])?;
        self.orders.insert(id, order.clone());
        Ok((StatusCode::OK, order))
    }

    fn patch_order(&mut self, id: u64, action: &str, body: Value) -> ApiResult {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Patch {
            field: String,
            value: String,
        }
        let patch: Patch = serde_json::from_value(body).map_err(|e| invalid(e.to_string()))?;
        if patch.field != action {
            return Err(invalid(format!("field must be {action}")));
        }
        let mut order = self.orders[&id].clone();
        let direction = &self.order_state[&id].direction;
        if action == "status" {
            if !STATUSES.contains(&patch.value.as_str()) {
                return Err(denied("Unsupported order status"));
            }
            if order["status"] == patch.value {
                return Ok((StatusCode::OK, Value::Null));
            }
            if order["status"] == "CANCELLED" {
                return Err(denied(
                    "Cancelled orders cannot be reopened in this simulator",
                ));
            }
            let allowed = if direction == "in" {
                &[
                    "PROCESSING",
                    "READY",
                    "PAID",
                    "PACKED",
                    "SHIPPED",
                    "COMPLETED",
                ][..]
            } else {
                &["RECEIVED", "COMPLETED"][..]
            };
            if !allowed.contains(&patch.value.as_str()) {
                return Err(denied(
                    "Status is controlled by another role or the system; use mock controls for system events",
                ));
            }
            if order["status"] == "COMPLETED" {
                let completed =
                    DateTime::parse_from_rfc3339(order["date_status_changed"].as_str().unwrap())
                        .map_err(|_| invalid("Invalid stored date"))?;
                let now = DateTime::parse_from_rfc3339(&self.clock.now()).unwrap();
                if now.signed_duration_since(completed) > chrono::Duration::days(7) {
                    return Err(denied(
                        "Completed order is outside the documented one-week change window",
                    ));
                }
            }
            order["status"] = json!(patch.value);
            order["date_status_changed"] = json!(self.clock.now());
            if patch.value == "PAID" {
                order["payment"]["status"] = json!("Paid");
                if order["payment"].get("date_paid").is_none() {
                    order["payment"]["date_paid"] = json!(self.clock.now());
                }
            }
        } else {
            if !PAYMENTS.contains(&patch.value.as_str()) {
                return Err(denied("Unsupported payment status"));
            }
            if order["payment"]["status"] == patch.value {
                return Ok((StatusCode::OK, Value::Null));
            }
            if direction == "out"
                && (patch.value != "Sent"
                    || !["None", "Sent"]
                        .contains(&order["payment"]["status"].as_str().unwrap_or("")))
            {
                return Err(denied(
                    "Buyer may only mark payment Sent before seller changes payment status",
                ));
            }
            if direction == "in" && patch.value == "Sent" {
                return Err(denied("Sent is a buyer-controlled payment status"));
            }
            order["payment"]["status"] = json!(patch.value);
            if ["Paid", "Completed"].contains(&patch.value.as_str())
                && order["payment"].get("date_paid").is_none()
            {
                order["payment"]["date_paid"] = json!(self.clock.now());
            }
        }
        self.orders.insert(id, order);
        // Method pages specify empty data, not an updated resource.
        Ok((StatusCode::OK, Value::Null))
    }

    fn edit_order(&mut self, id: u64, body: Value) -> ApiResult {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Edit {
            items: Vec<OrderLine>,
        }
        let input: Edit = serde_json::from_value(body).map_err(|e| invalid(e.to_string()))?;
        if input.items.is_empty() {
            return Err(invalid("Use cancel instead of an empty order"));
        }
        let desired = line_counts(&input.items)?;
        let old = item_map(&self.order_items[&id]);
        if old
            .iter()
            .map(|(id, item)| (*id, item["quantity"].as_i64().unwrap()))
            .collect::<BTreeMap<_, _>>()
            == desired
        {
            return Ok((StatusCode::OK, self.orders[&id].clone()));
        }
        if !["PENDING", "UPDATED", "PROCESSING"]
            .contains(&self.orders[&id]["status"].as_str().unwrap_or(""))
            || settled(&self.orders[&id])
            || self.order_state[&id].restitution_started
        {
            return Err(denied(
                "Edits require an unpaid pending/updated/processing order without prior restitution",
            ));
        }
        let mut snapshots = BTreeMap::new();
        for (lot_id, qty) in &desired {
            let mut item = if let Some(old) = old.get(lot_id) {
                old.clone()
            } else {
                let lot = self.inventories.get(lot_id).ok_or_else(missing)?;
                wire::order_item(lot, *qty)
            };
            item["quantity"] = json!(qty);
            item["currency_code"] = self.orders[&id]["cost"]["currency_code"].clone();
            item["disp_currency_code"] = item["currency_code"].clone();
            snapshots.insert(*lot_id, item);
        }
        if self.order_state[&id].managed {
            let held = self.order_state[&id].held.clone();
            let ids: std::collections::BTreeSet<_> =
                held.keys().chain(desired.keys()).copied().collect();
            for lot_id in ids {
                let delta = desired.get(&lot_id).copied().unwrap_or(0)
                    - held.get(&lot_id).copied().unwrap_or(0);
                if delta == 0 {
                    continue;
                }
                let lot = self.inventories.get_mut(&lot_id).ok_or_else(missing)?;
                if delta > 0 && (lot.fields.is_stock_room || lot.fields.quantity < delta) {
                    return Err(denied("Insufficient available stock for edit"));
                }
                lot.fields.quantity = quantity(
                    lot.fields
                        .quantity
                        .checked_sub(delta)
                        .ok_or_else(|| invalid("Quantity overflow"))?,
                )?;
            }
            self.order_state.get_mut(&id).unwrap().held = desired;
        }
        let batches = json!([snapshots.into_values().collect::<Vec<_>>()]);
        let order = self.orders.get_mut(&id).unwrap();
        order["status"] = json!("UPDATED");
        order["payment"]["status"] = json!("None");
        order["date_status_changed"] = json!(self.clock.now());
        recompute(order, &batches)?;
        self.order_items.insert(id, batches);
        Ok((StatusCode::OK, order.clone()))
    }

    fn return_stock(&mut self, id: u64, lines: &[OrderLine]) -> Result<(), ApiError> {
        let quantities = line_counts(lines)?;
        let state = self.order_state.get_mut(&id).unwrap();
        if !state.managed && !quantities.is_empty() {
            return Err(denied(
                "Order has no local inventory reservation to restock",
            ));
        }
        for (lot_id, count) in quantities {
            let held = state
                .held
                .get_mut(&lot_id)
                .ok_or_else(|| denied("No remaining reservation for this lot"))?;
            if count > *held {
                return Err(denied("Restock exceeds remaining reservation"));
            }
            let lot = self.inventories.get_mut(&lot_id).ok_or_else(missing)?;
            lot.fields.quantity = quantity(
                lot.fields
                    .quantity
                    .checked_add(count)
                    .ok_or_else(|| invalid("Quantity overflow"))?,
            )?;
            *held -= count;
        }
        state.held.retain(|_, count| *count > 0);
        if !lines.is_empty() {
            state.restitution_started = true;
        }
        Ok(())
    }

    fn restitution(&mut self, id: u64, action: &str, body: Value) -> ApiResult {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Restitution {
            operation_id: String,
            #[serde(default)]
            items: Vec<OrderLine>,
            amount: Option<String>,
        }
        let input: Restitution =
            serde_json::from_value(body.clone()).map_err(|e| invalid(e.to_string()))?;
        if input.operation_id.is_empty() {
            return Err(invalid("operation_id must not be empty"));
        }
        let fingerprint = json!({"action": action, "body": body});
        if let Some((original, response)) = self.order_state[&id].receipts.get(&input.operation_id)
        {
            if original != &fingerprint {
                return Err(denied("operation_id already used with different input"));
            }
            return Ok((StatusCode::OK, response.clone()));
        }
        if action == "refund" {
            if !settled(&self.orders[&id]) {
                return Err(denied(
                    "Refund requires a settled payment/order in this simulator",
                ));
            }
            let amount = input
                .amount
                .ok_or_else(|| invalid("Refund amount is required"))?;
            let refund = Decimal::from_str_exact(&price(&amount)?).unwrap();
            if refund <= Decimal::ZERO {
                return Err(invalid("Refund must be positive"));
            }
            let total = decimal(&self.orders[&id]["cost"]["grand_total"], "grand_total")?;
            let state = self.order_state.get_mut(&id).unwrap();
            let refunded = state
                .refunded
                .checked_add(refund)
                .ok_or_else(|| invalid("Refund overflow"))?;
            if refunded > total {
                return Err(denied("Refund exceeds remaining order total"));
            }
            state.refunded = refunded;
            state.restitution_started = true;
            if refunded == total {
                self.orders.get_mut(&id).unwrap()["payment"]["status"] = json!("Returned");
            }
        } else if input.amount.is_some() || input.items.is_empty() {
            return Err(invalid("Restock requires items and does not accept amount"));
        }
        self.return_stock(id, &input.items)?;
        let response = self.order_effects(id)?.1;
        self.order_state
            .get_mut(&id)
            .unwrap()
            .receipts
            .insert(input.operation_id, (fingerprint, response.clone()));
        Ok((StatusCode::OK, response))
    }
}
