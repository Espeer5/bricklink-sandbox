//! An intentionally limited, stateful BrickLink Store API simulator.
//! Each call to [`app`] or [`app_from_fixture`] creates an independent store.

mod auth;
mod catalog;
mod fixture;
mod orders;
mod validation;
mod wire;
pub use fixture::{FixtureError, app_configured, app_from_fixture};

use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{SecondsFormat, Utc};
use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, str::FromStr, sync::Arc};
use tokio::sync::Mutex;

pub const BASE: &str = "/api/store/v1";
type SharedStore = Arc<Mutex<fixture::Sandbox>>;
type ApiResult = Result<(StatusCode, Value), ApiError>;

#[derive(Debug)]
struct ApiError(StatusCode, &'static str, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.0,
            Json(json!({"meta": {"code": self.0.as_u16(), "message": self.1,
                                    "description": self.2}, "data": null})),
        )
            .into_response()
    }
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError(
        StatusCode::BAD_REQUEST,
        "PARAMETER_MISSING_OR_INVALID",
        message.into(),
    )
}

fn missing() -> ApiError {
    ApiError(
        StatusCode::NOT_FOUND,
        "RESOURCE_NOT_FOUND",
        "Resource or route is not implemented".into(),
    )
}

fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn price(value: &str) -> Result<String, ApiError> {
    let price = Decimal::from_str_exact(value).map_err(|_| invalid("Invalid decimal price"))?;
    if price < Decimal::ZERO || price > Decimal::from(1_000_000_000u64) {
        return Err(invalid("Price must be nonnegative and <= 1000000000"));
    }
    let price = price.round_dp_with_strategy(4, RoundingStrategy::ToPositiveInfinity);
    Ok(format!("{price:.4}"))
}

fn quantity(value: i64) -> Result<i64, ApiError> {
    if !(0..=1_000_000_000).contains(&value) {
        return Err(invalid("Quantity must be 0..1000000000"));
    }
    Ok(value)
}

fn matches(value: impl ToString, expression: &str) -> bool {
    let tokens: Vec<_> = expression
        .split(',')
        .filter(|s| !s.is_empty())
        .map(str::to_uppercase)
        .collect();
    let value = value.to_string().to_uppercase();
    let included: Vec<_> = tokens.iter().filter(|s| !s.starts_with('-')).collect();
    (included.is_empty() || included.iter().any(|s| **s == value))
        && !tokens
            .iter()
            .any(|s| s.strip_prefix('-') == Some(value.as_str()))
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Item {
    no: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    category_id: u64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct InventoryInput {
    item: Item,
    color_id: u64,
    quantity: i64,
    unit_price: String,
    new_or_used: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    remarks: String,
    #[serde(default)]
    is_stock_room: bool,
    #[serde(default = "default_room")]
    stock_room_id: String,
}

fn default_room() -> String {
    "A".into()
}

#[derive(Clone, Serialize)]
struct Inventory {
    inventory_id: u64,
    #[serde(flatten)]
    fields: InventoryInput,
    date_created: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct InventoryUpdate {
    quantity: Option<String>,
    unit_price: Option<String>,
    description: Option<String>,
    remarks: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OrderInput {
    items: Vec<OrderLine>,
    #[serde(default = "orders::empty_details")]
    details: Value,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OrderLine {
    inventory_id: u64,
    quantity: i64,
}

#[derive(Clone)]
struct Store {
    catalog: catalog::Catalog,
    clock: fixture::Clock,
    inventories: BTreeMap<u64, Inventory>,
    orders: BTreeMap<u64, Value>,
    order_items: BTreeMap<u64, Value>,
    order_state: BTreeMap<u64, orders::OrderState>,
    next_inventory: u64,
    next_order: u64,
}

impl Store {
    fn seeded() -> Self {
        let mut store = Self {
            catalog: catalog::Catalog::default(),
            clock: fixture::Clock::System,
            inventories: BTreeMap::new(),
            orders: BTreeMap::new(),
            order_items: BTreeMap::new(),
            order_state: BTreeMap::new(),
            next_inventory: 1000,
            next_order: 10000,
        };
        store
            .create_inventory(
                json!({"item": {"no": "3001", "type": "PART", "name": "Brick 2 x 4"},
            "color_id": 5, "quantity": 100, "unit_price": "0.1500", "new_or_used": "N",
            "remarks": "BIN-A01"}),
            )
            .expect("built-in seed is valid");
        store
    }

    fn create_inventory(&mut self, body: Value) -> ApiResult {
        let mut input: InventoryInput =
            serde_json::from_value(body).map_err(|e| invalid(e.to_string()))?;
        quantity(input.quantity)?;
        input.unit_price = price(&input.unit_price)?;
        if input.item.no.is_empty()
            || ![
                "PART",
                "SET",
                "MINIFIG",
                "BOOK",
                "GEAR",
                "CATALOG",
                "INSTRUCTION",
                "UNSORTED_LOT",
                "ORIGINAL_BOX",
            ]
            .contains(&input.item.kind.as_str())
        {
            return Err(invalid("Supply item.no and a valid uppercase item.type"));
        }
        if !["N", "U"].contains(&input.new_or_used.as_str())
            || !["A", "B", "C"].contains(&input.stock_room_id.as_str())
        {
            return Err(invalid(
                "new_or_used must be N/U; stock_room_id must be A/B/C",
            ));
        }
        self.catalog.check_inventory(&input)?;
        let next_id = self
            .next_inventory
            .checked_add(1)
            .ok_or_else(|| invalid("Inventory IDs exhausted"))?;
        let lot = Inventory {
            inventory_id: self.next_inventory,
            fields: input,
            date_created: self.clock.now(),
        };
        let data = json!(lot);
        self.inventories.insert(self.next_inventory, lot);
        self.next_inventory = next_id;
        Ok((StatusCode::CREATED, data))
    }

    fn update_inventory(&mut self, id: u64, body: Value) -> ApiResult {
        let update: InventoryUpdate =
            serde_json::from_value(body).map_err(|e| invalid(e.to_string()))?;
        let mut lot = self.inventories.get(&id).ok_or_else(missing)?.clone();
        if let Some(delta) = update.quantity {
            if !(delta.starts_with('+') || delta.starts_with('-'))
                || delta.len() < 2
                || !delta[1..].bytes().all(|b| b.is_ascii_digit())
            {
                return Err(invalid(
                    "quantity must be a signed delta string, e.g. +10 or -3",
                ));
            }
            let delta: i64 = delta
                .parse()
                .map_err(|_| invalid("Invalid quantity delta"))?;
            lot.fields.quantity = quantity(
                lot.fields
                    .quantity
                    .checked_add(delta)
                    .ok_or_else(|| invalid("Quantity overflow"))?,
            )?;
        }
        if let Some(value) = update.unit_price {
            lot.fields.unit_price = price(&value)?;
        }
        if let Some(value) = update.description {
            lot.fields.description = value;
        }
        if let Some(value) = update.remarks {
            lot.fields.remarks = value;
        }
        let data = json!(lot);
        self.inventories.insert(id, lot);
        Ok((StatusCode::OK, data))
    }

    fn create_order(&mut self, body: Value) -> ApiResult {
        self.insert_order(body, true)
    }

    fn insert_order(&mut self, body: Value, consume_stock: bool) -> ApiResult {
        let mut staged = self.clone();
        let result = staged.insert_order_inner(body, consume_stock)?;
        *self = staged;
        Ok(result)
    }

    fn insert_order_inner(&mut self, body: Value, consume_stock: bool) -> ApiResult {
        let input: OrderInput = serde_json::from_value(body).map_err(|e| invalid(e.to_string()))?;
        let consume_stock =
            consume_stock && input.details.get("direction").and_then(Value::as_str) != Some("out");
        if input.items.is_empty() {
            return Err(invalid("items must not be empty"));
        }
        let mut quantities = BTreeMap::<u64, i64>::new();
        for line in input.items {
            quantity(line.quantity)?;
            if line.quantity == 0 {
                return Err(invalid("Order quantity must be positive"));
            }
            let total = quantities.entry(line.inventory_id).or_default();
            *total = total
                .checked_add(line.quantity)
                .ok_or_else(|| invalid("Quantity overflow"))?;
        }
        let mut items = Vec::new();
        let mut subtotal = Decimal::ZERO;
        let mut total_count = 0i64;
        // Build and validate the complete order before changing any stock.
        for (id, count) in &quantities {
            let lot = self.inventories.get(id).ok_or_else(missing)?;
            if consume_stock && (lot.fields.is_stock_room || lot.fields.quantity < *count) {
                return Err(ApiError(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "RESOURCE_UPDATE_NOT_ALLOWED",
                    "Insufficient available stock".into(),
                ));
            }
            let unit_price = Decimal::from_str(&lot.fields.unit_price)
                .map_err(|_| invalid("Invalid stored price"))?;
            subtotal = subtotal
                .checked_add(unit_price * Decimal::from(*count))
                .ok_or_else(|| invalid("Order total overflow"))?;
            total_count = total_count
                .checked_add(*count)
                .ok_or_else(|| invalid("Order quantity overflow"))?;
            items.push(wire::order_item(lot, *count));
        }
        let id = self.next_order;
        let next_id = id
            .checked_add(1)
            .ok_or_else(|| invalid("Order IDs exhausted"))?;
        let time = self.clock.now();
        let order = json!({"order_id": id, "date_ordered": time, "date_status_changed": time,
            "seller_name": "mock_seller", "store_name": "Mock Brick Store", "buyer_name": "mock_buyer",
            "buyer_email": "buyer@example.invalid", "status": "PENDING", "is_filed": false,
            "remarks": "", "total_count": total_count, "unique_count": items.len(),
            "payment": {"method": "Mock", "status": "None", "currency_code": "USD"},
            "shipping": {}, "cost": {"currency_code": "USD", "subtotal": format!("{subtotal:.4}"),
            "grand_total": format!("{subtotal:.4}"), "shipping": "0.0000"}});
        for (id, count) in quantities.into_iter().filter(|_| consume_stock) {
            self.inventories
                .get_mut(&id)
                .expect("validated lot")
                .fields
                .quantity -= count;
        }
        self.orders.insert(id, order.clone());
        self.order_items.insert(id, json!([items]));
        self.next_order = next_id;
        self.configure_order(id, input.details, consume_stock)?;
        Ok((StatusCode::CREATED, self.orders[&id].clone()))
    }

    fn dispatch(
        &mut self,
        method: &str,
        path: &str,
        query: &BTreeMap<String, String>,
        body: Value,
    ) -> ApiResult {
        match (method, path) {
            ("GET", "/health") => return Ok((StatusCode::OK, json!({"status": "ok"}))),
            ("POST", "/__mock/orders") => return self.create_order(body),
            _ => {}
        }
        if let Some(route) = path.strip_prefix("/__mock/orders/") {
            let parts: Vec<_> = route.split('/').collect();
            if let [id, action] = parts.as_slice() {
                let id = id.parse().map_err(|_| invalid("Invalid order ID"))?;
                if method == "GET" && *action == "state" {
                    return self.order_effects(id);
                }
                if method == "POST" {
                    return self.order_action(id, action, body);
                }
            }
        }
        let route = path
            .strip_prefix(BASE)
            .and_then(|p| p.strip_prefix('/'))
            .ok_or_else(missing)?;
        let parts: Vec<_> = route.trim_end_matches('/').split('/').collect();
        if method == "GET"
            && matches!(
                parts.first(),
                Some(&"items" | &"colors" | &"categories" | &"item_mapping")
            )
        {
            return self.catalog.dispatch(&parts, query);
        }
        match (method, parts.as_slice()) {
            ("POST", ["inventories"]) => self.create_inventory(body),
            ("GET", ["inventories"]) => {
                if query.keys().any(|k| {
                    !["item_type", "color_id", "category_id", "status"].contains(&k.as_str())
                }) {
                    return Err(invalid("Unsupported inventory filter"));
                }
                let lots: Vec<_> = self
                    .inventories
                    .values()
                    .filter(|lot| {
                        query.iter().all(|(key, value)| {
                            let field = match key.as_str() {
                                "item_type" => lot.fields.item.kind.clone(),
                                "color_id" => lot.fields.color_id.to_string(),
                                "category_id" => lot.fields.item.category_id.to_string(),
                                _ => {
                                    if lot.fields.is_stock_room {
                                        if lot.fields.stock_room_id == "A" {
                                            "S".into()
                                        } else {
                                            lot.fields.stock_room_id.clone()
                                        }
                                    } else if lot.fields.quantity > 0 {
                                        "Y".into()
                                    } else {
                                        "N".into()
                                    }
                                }
                            };
                            matches(field, value)
                        })
                    })
                    .collect();
                Ok((StatusCode::OK, json!(lots)))
            }
            (_, ["inventories", id]) => {
                let id: u64 = id.parse().map_err(|_| missing())?;
                let lot = self.inventories.get(&id).ok_or_else(missing)?;
                match method {
                    "GET" => Ok((StatusCode::OK, json!(lot))),
                    "PUT" => self.update_inventory(id, body),
                    "DELETE" => {
                        self.inventories.remove(&id);
                        Ok((StatusCode::NO_CONTENT, Value::Null))
                    }
                    _ => Err(missing()),
                }
            }
            ("GET", ["orders"]) => {
                if query
                    .keys()
                    .any(|k| !["direction", "status", "filed"].contains(&k.as_str()))
                {
                    return Err(invalid("Unsupported order filter"));
                }
                let direction = query.get("direction").map(String::as_str).unwrap_or("in");
                let filed = query.get("filed").map(String::as_str).unwrap_or("false");
                if !["in", "out"].contains(&direction) || !["true", "false"].contains(&filed) {
                    return Err(invalid("Invalid direction or filed filter"));
                }
                let orders: Vec<_> = self
                    .orders
                    .values()
                    .filter(|order| {
                        self.order_state
                            .get(&order["order_id"].as_u64().unwrap())
                            .is_some_and(|state| state.direction == direction)
                            && order["is_filed"] == json!(filed == "true")
                            && matches(
                                order["status"].as_str().unwrap_or(""),
                                query.get("status").map(String::as_str).unwrap_or(""),
                            )
                    })
                    .collect();
                Ok((
                    StatusCode::OK,
                    json!(
                        orders
                            .into_iter()
                            .map(wire::order_summary)
                            .collect::<Vec<_>>()
                    ),
                ))
            }
            ("PUT", ["orders", id]) => self.order_action(
                id.parse().map_err(|_| invalid("Invalid order ID"))?,
                "update",
                body,
            ),
            ("PUT", ["orders", id, action @ ("status" | "payment_status")]) => self.order_action(
                id.parse().map_err(|_| invalid("Invalid order ID"))?,
                action,
                body,
            ),
            ("GET", ["orders", id]) => Ok((
                StatusCode::OK,
                self.orders
                    .get(&id.parse().map_err(|_| missing())?)
                    .ok_or_else(missing)?
                    .clone(),
            )),
            ("GET", ["orders", id, "items"]) => Ok((
                StatusCode::OK,
                self.order_items
                    .get(&id.parse().map_err(|_| missing())?)
                    .ok_or_else(missing)?
                    .clone(),
            )),
            _ => Err(missing()),
        }
    }
}

fn response(result: ApiResult) -> Response {
    match result {
        Ok((StatusCode::NO_CONTENT, _)) => StatusCode::NO_CONTENT.into_response(),
        Ok((code, data)) => (code,Json(json!({"meta":{"code":code.as_u16(),"message":if code==StatusCode::CREATED{"OK_CREATED"}else{"OK"},"description":""},"data":data}))).into_response(),
        Err(error)=>error.into_response(),
    }
}
async fn handle(State(store): State<SharedStore>, request: Request) -> Response {
    match execute(store, request).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}
async fn execute(store: SharedStore, request: Request) -> Result<Response, ApiError> {
    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, 1024 * 1024).await.map_err(|_| {
        ApiError(
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST_BODY",
            "Body exceeds 1 MiB or cannot be read".into(),
        )
    })?;
    if parts.uri.path().starts_with(&format!("{BASE}/")) {
        store.lock().await.authenticate(&parts, &bytes)?;
    }
    wire::check_method(parts.method.as_str(), parts.uri.path())?;
    let query = url::form_urlencoded::parse(parts.uri.query().unwrap_or("").as_bytes())
        .into_owned()
        .filter(|(key, _)| key != "Authorization")
        .collect();
    let body = wire::decode_body(
        &bytes,
        parts
            .headers
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
    )?;
    let fault = store
        .lock()
        .await
        .validation
        .select(parts.method.as_str(), parts.uri.path());
    if let Some(rule) = fault.as_ref().filter(|r| r.phase == "before") {
        store.lock().await.validation.event(rule, false);
        if let Some(response) = validation::effect(rule).await {
            return Ok(response);
        }
    }
    let result = store
        .lock()
        .await
        .dispatch(parts.method.as_str(), parts.uri.path(), &query, body);
    if let Some(rule) = fault.as_ref().filter(|r| r.phase == "after") {
        let succeeded = result.is_ok();
        store.lock().await.validation.event(rule, succeeded);
        if succeeded {
            if let Some(response) = validation::effect(rule).await {
                return Ok(response);
            }
        }
    }
    Ok(response(result))
}

/// Build a router with isolated, seeded, in-memory state.
pub fn app() -> Router {
    Router::new()
        .fallback(handle)
        .with_state(Arc::new(Mutex::new(fixture::Sandbox::seeded())))
}
