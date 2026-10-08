//! Authored fixtures and scripts, deliberately separate from runtime snapshots.

use super::*;
use chrono::{DateTime, TimeDelta};
use std::fmt;

/// A fixture error with a JSON field path and a human-readable explanation.
#[derive(Debug)]
pub struct FixtureError {
    pub path: String,
    pub message: String,
}

impl fmt::Display for FixtureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

impl std::error::Error for FixtureError {}

fn error(path: impl Into<String>, message: impl Into<String>) -> FixtureError {
    FixtureError {
        path: path.into(),
        message: message.into(),
    }
}

fn parse<T: serde::de::DeserializeOwned>(json: &str) -> Result<T, FixtureError> {
    let mut deserializer = serde_json::Deserializer::from_str(json);
    let value = serde_path_to_error::deserialize(&mut deserializer).map_err(|e| {
        let path = e.path().to_string();
        error(
            if path == "." {
                "$".into()
            } else {
                format!("$.{path}")
            },
            e.inner().to_string(),
        )
    })?;
    deserializer.end().map_err(|e| error("$", e.to_string()))?;
    Ok(value)
}

fn instant(value: &str, path: &str) -> Result<DateTime<Utc>, FixtureError> {
    let parsed = DateTime::parse_from_rfc3339(value)
        .map_err(|e| error(path, format!("Expected an RFC3339 timestamp: {e}")))?
        .with_timezone(&Utc);
    if parsed.timestamp_subsec_nanos() % 1_000_000 != 0 {
        return Err(error(
            path,
            "Timestamp precision must not exceed milliseconds",
        ));
    }
    Ok(parsed)
}

#[derive(Clone)]
pub(super) enum Clock {
    System,
    Fixed(DateTime<Utc>),
}

impl Clock {
    pub(super) fn now(&self) -> String {
        match self {
            Self::System => timestamp(),
            Self::Fixed(time) => time.to_rfc3339_opts(SecondsFormat::Millis, true),
        }
    }

    fn value(&self) -> Value {
        json!({"mode": if matches!(self, Self::System) { "system" } else { "fixed" }, "now": self.now()})
    }

    fn advance(&mut self, seconds: u64) -> Result<Value, ApiError> {
        let Self::Fixed(time) = self else {
            return Err(invalid("Clock advancement requires a fixed clock"));
        };
        let delta = i64::try_from(seconds)
            .ok()
            .and_then(TimeDelta::try_seconds)
            .ok_or_else(|| invalid("seconds exceeds the supported clock range"))?;
        *time = time
            .checked_add_signed(delta)
            .ok_or_else(|| invalid("Clock advancement overflows timestamp"))?;
        Ok(self.value())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClockInput {
    mode: String,
    now: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    version: u32,
    clock: ClockInput,
    inventories: Vec<FixtureInventory>,
    orders: Vec<FixtureOrder>,
    #[serde(default)]
    steps: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureInventory {
    inventory_id: u64,
    inventory: InventoryInput,
    date_created: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureOrder {
    order_id: u64,
    items: Vec<OrderLine>,
    date_ordered: Option<String>,
    #[serde(default = "pending")]
    status: String,
    #[serde(default)]
    is_filed: bool,
    #[serde(default = "buyer")]
    buyer_name: String,
}

fn pending() -> String {
    "PENDING".into()
}
fn buyer() -> String {
    "mock_buyer".into()
}

#[derive(Clone)]
enum Step {
    AdvanceClock {
        seconds: u64,
    },
    CreateInventory {
        inventory: InventoryInput,
    },
    UpdateInventory {
        inventory_id: u64,
        update: InventoryUpdate,
    },
    CreateOrder {
        items: Vec<OrderLine>,
    },
}

impl Step {
    fn parse(mut value: Value, index: usize) -> Result<Self, FixtureError> {
        let path = format!("$.steps[{index}]");
        let object = value
            .as_object_mut()
            .ok_or_else(|| error(&path, "Expected an action object"))?;
        let action = object
            .remove("action")
            .ok_or_else(|| error(format!("{path}.action"), "Missing action"))?;
        // Decode each action's fields directly so errors retain nested field paths.
        fn fields<T: serde::de::DeserializeOwned>(
            value: Value,
            path: &str,
        ) -> Result<T, FixtureError> {
            parse(&value.to_string()).map_err(|e| {
                error(
                    format!("{path}{}", e.path.strip_prefix('$').unwrap_or(&e.path)),
                    e.message,
                )
            })
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Advance {
            seconds: u64,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Create {
            inventory: InventoryInput,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Update {
            inventory_id: u64,
            update: InventoryUpdate,
        }
        match action.as_str() {
            Some("advance_clock") => {
                let input: Advance = fields(value, &path)?;
                Ok(Self::AdvanceClock {
                    seconds: input.seconds,
                })
            }
            Some("create_inventory") => {
                let input: Create = fields(value, &path)?;
                Ok(Self::CreateInventory {
                    inventory: input.inventory,
                })
            }
            Some("update_inventory") => {
                let input: Update = fields(value, &path)?;
                Ok(Self::UpdateInventory {
                    inventory_id: input.inventory_id,
                    update: input.update,
                })
            }
            Some("create_order") => {
                let input: OrderInput = fields(value, &path)?;
                Ok(Self::CreateOrder { items: input.items })
            }
            _ => Err(error(
                format!("{path}.action"),
                "Expected advance_clock, create_inventory, update_inventory, or create_order",
            )),
        }
    }

    fn run(&self, store: &mut Store) -> ApiResult {
        match self {
            Self::AdvanceClock { seconds } => Ok((StatusCode::OK, store.clock.advance(*seconds)?)),
            Self::CreateInventory { inventory } => store.create_inventory(json!(inventory)),
            Self::UpdateInventory {
                inventory_id,
                update,
            } => store.update_inventory(*inventory_id, json!(update)),
            Self::CreateOrder { items } => store.create_order(json!({"items": items})),
        }
    }
}

fn validate_inventory(input: &InventoryInput, path: &str) -> Result<(), FixtureError> {
    quantity(input.quantity).map_err(|e| error(format!("{path}.quantity"), e.2))?;
    price(&input.unit_price).map_err(|e| error(format!("{path}.unit_price"), e.2))?;
    if input.item.no.is_empty() {
        return Err(error(format!("{path}.item.no"), "Must not be empty"));
    }
    if ![
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
        return Err(error(format!("{path}.item.type"), "Unsupported item type"));
    }
    if !["N", "U"].contains(&input.new_or_used.as_str()) {
        return Err(error(format!("{path}.new_or_used"), "Expected N or U"));
    }
    if !["A", "B", "C"].contains(&input.stock_room_id.as_str()) {
        return Err(error(
            format!("{path}.stock_room_id"),
            "Expected A, B, or C",
        ));
    }
    Ok(())
}

fn validate_lines(
    store: &Store,
    lines: &[OrderLine],
    path: &str,
    consume: bool,
) -> Result<(), FixtureError> {
    if lines.is_empty() {
        return Err(error(path, "Must not be empty"));
    }
    let mut totals = BTreeMap::<u64, i64>::new();
    for (i, line) in lines.iter().enumerate() {
        let lot = store
            .inventories
            .get(&line.inventory_id)
            .ok_or_else(|| error(format!("{path}[{i}].inventory_id"), "Unknown inventory ID"))?;
        if !(1..=1_000_000_000).contains(&line.quantity) {
            return Err(error(
                format!("{path}[{i}].quantity"),
                "Expected 1..1000000000",
            ));
        }
        let total = totals.entry(line.inventory_id).or_default();
        *total = total
            .checked_add(line.quantity)
            .ok_or_else(|| error(format!("{path}[{i}].quantity"), "Quantity overflow"))?;
        if consume && (lot.fields.is_stock_room || *total > lot.fields.quantity) {
            return Err(error(
                format!("{path}[{i}].quantity"),
                "Insufficient available stock",
            ));
        }
    }
    Ok(())
}

fn validate_id(id: u64, path: &str) -> Result<(), FixtureError> {
    if id == 0 || id == u64::MAX {
        return Err(error(path, "ID must be between 1 and 18446744073709551614"));
    }
    Ok(())
}

pub(super) struct Sandbox {
    store: Store,
    initial: Store,
    steps: Vec<Step>,
}

impl Sandbox {
    pub(super) fn seeded() -> Self {
        let initial = Store::seeded();
        Self {
            store: initial.clone(),
            initial,
            steps: vec![],
        }
    }

    fn from_fixture(text: &str) -> Result<Self, FixtureError> {
        let fixture: Fixture = parse(text)?;
        if fixture.version != 1 {
            return Err(error(
                "$.version",
                "Unsupported fixture version; expected 1",
            ));
        }
        let clock = match fixture.clock.mode.as_str() {
            "system" if fixture.clock.now.is_none() => Clock::System,
            "system" => return Err(error("$.clock.now", "System clock must not specify now")),
            "fixed" => Clock::Fixed(instant(
                fixture
                    .clock
                    .now
                    .as_deref()
                    .ok_or_else(|| error("$.clock.now", "Fixed clock requires now"))?,
                "$.clock.now",
            )?),
            _ => return Err(error("$.clock.mode", "Expected system or fixed")),
        };
        if !fixture.steps.is_empty() && matches!(clock, Clock::System) {
            return Err(error(
                "$.clock.mode",
                "Scripted scenarios require a fixed clock",
            ));
        }
        let mut store = Store {
            clock,
            inventories: BTreeMap::new(),
            orders: BTreeMap::new(),
            order_items: BTreeMap::new(),
            next_inventory: 1000,
            next_order: 10000,
        };
        let mut next_inventory = 1000;
        for (i, lot) in fixture.inventories.into_iter().enumerate() {
            let path = format!("$.inventories[{i}]");
            validate_id(lot.inventory_id, &format!("{path}.inventory_id"))?;
            if store.inventories.contains_key(&lot.inventory_id) {
                return Err(error(
                    format!("{path}.inventory_id"),
                    "Duplicate inventory ID",
                ));
            }
            validate_inventory(&lot.inventory, &format!("{path}.inventory"))?;
            let created = lot
                .date_created
                .map(|s| instant(&s, &format!("{path}.date_created")))
                .transpose()?;
            store.next_inventory = lot.inventory_id;
            store
                .create_inventory(json!(lot.inventory))
                .map_err(|e| error(&path, e.2))?;
            if let Some(created) = created {
                store
                    .inventories
                    .get_mut(&lot.inventory_id)
                    .expect("inserted lot")
                    .date_created = created.to_rfc3339_opts(SecondsFormat::Millis, true);
            }
            next_inventory = next_inventory.max(store.next_inventory);
        }
        store.next_inventory = next_inventory;
        let mut next_order = 10000;
        for (i, order) in fixture.orders.into_iter().enumerate() {
            let path = format!("$.orders[{i}]");
            validate_id(order.order_id, &format!("{path}.order_id"))?;
            if store.orders.contains_key(&order.order_id) {
                return Err(error(format!("{path}.order_id"), "Duplicate order ID"));
            }
            validate_lines(&store, &order.items, &format!("{path}.items"), false)?;
            if !["PENDING", "PAID", "SHIPPED", "COMPLETED"].contains(&order.status.as_str()) {
                return Err(error(
                    format!("{path}.status"),
                    "Supported fixture statuses: PENDING, PAID, SHIPPED, COMPLETED",
                ));
            }
            if order.buyer_name.is_empty() {
                return Err(error(format!("{path}.buyer_name"), "Must not be empty"));
            }
            let ordered = order
                .date_ordered
                .map(|s| instant(&s, &format!("{path}.date_ordered")))
                .transpose()?;
            store.next_order = order.order_id;
            store
                .insert_order(json!({"items": order.items}), false)
                .map_err(|e| error(&path, e.2))?;
            let saved = store
                .orders
                .get_mut(&order.order_id)
                .expect("inserted order");
            saved["status"] = json!(order.status);
            saved["is_filed"] = json!(order.is_filed);
            saved["buyer_name"] = json!(order.buyer_name);
            if let Some(ordered) = ordered {
                let date = ordered.to_rfc3339_opts(SecondsFormat::Millis, true);
                saved["date_ordered"] = json!(date);
                saved["date_status_changed"] = json!(date);
            }
            next_order = next_order.max(store.next_order);
        }
        store.next_order = next_order;
        let steps = fixture
            .steps
            .into_iter()
            .enumerate()
            .map(|(i, value)| Step::parse(value, i))
            .collect::<Result<_, _>>()?;
        let sandbox = Self {
            initial: store.clone(),
            store,
            steps,
        };
        // Validate the whole script on disposable state, without advancing startup state.
        sandbox.replayed()?;
        Ok(sandbox)
    }

    fn replayed(&self) -> Result<(Store, Value), FixtureError> {
        let mut store = self.initial.clone();
        let mut results = vec![];
        for (i, step) in self.steps.iter().enumerate() {
            let path = format!("$.steps[{i}]");
            match step {
                Step::CreateInventory { inventory } => {
                    validate_inventory(inventory, &format!("{path}.inventory"))?
                }
                Step::CreateOrder { items } => {
                    validate_lines(&store, items, &format!("{path}.items"), true)?
                }
                Step::UpdateInventory {
                    inventory_id,
                    update,
                } => {
                    if !store.inventories.contains_key(inventory_id) {
                        return Err(error(
                            format!("{path}.inventory_id"),
                            "Unknown inventory ID",
                        ));
                    }
                    if let Some(value) = &update.unit_price {
                        price(value)
                            .map_err(|e| error(format!("{path}.update.unit_price"), e.2))?;
                    }
                }
                _ => {}
            }
            let (status, data) = step.run(&mut store).map_err(|e| {
                error(
                    match step {
                        Step::AdvanceClock { .. } => format!("{path}.seconds"),
                        Step::UpdateInventory { .. } => format!("{path}.update.quantity"),
                        _ => path.clone(),
                    },
                    e.2,
                )
            })?;
            results.push(json!({"step": i, "code": status.as_u16(), "data": data}));
        }
        Ok((store, json!(results)))
    }

    pub(super) fn dispatch(
        &mut self,
        method: &str,
        path: &str,
        query: &BTreeMap<String, String>,
        body: Value,
    ) -> ApiResult {
        match (method, path) {
            ("POST", "/__mock/reset") => {
                self.store = self.initial.clone();
                Ok((StatusCode::OK, json!({"reset": true})))
            }
            ("GET", "/__mock/clock") => Ok((StatusCode::OK, self.store.clock.value())),
            ("POST", "/__mock/clock/advance") => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Advance {
                    seconds: u64,
                }
                let advance: Advance =
                    serde_json::from_value(body).map_err(|e| invalid(e.to_string()))?;
                Ok((StatusCode::OK, self.store.clock.advance(advance.seconds)?))
            }
            ("POST", "/__mock/replay") => {
                if matches!(self.initial.clock, Clock::System) {
                    return Err(invalid("Replay requires a fixed clock fixture"));
                }
                let (store, results) = self.replayed().map_err(|e| invalid(e.to_string()))?;
                self.store = store;
                Ok((
                    StatusCode::OK,
                    json!({"steps": results, "clock": self.store.clock.value()}),
                ))
            }
            _ => self.store.dispatch(method, path, query, body),
        }
    }
}

/// Validate a version-1 JSON fixture and build an isolated router.
/// Loading historical orders does not deduct inventory. Script actions run only
/// when `POST /__mock/replay` is called; startup validates them without committing.
pub fn app_from_fixture(text: &str) -> Result<Router, FixtureError> {
    let sandbox = Sandbox::from_fixture(text)?;
    Ok(Router::new()
        .fallback(handle)
        .with_state(Arc::new(Mutex::new(sandbox))))
}
