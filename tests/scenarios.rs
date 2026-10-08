use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use bricklink_sandbox::{app, app_from_fixture};
use serde_json::{Value, json};
use tower::ServiceExt;

const SMALL: &str = include_str!("../fixtures/small.json");
const LARGE: &str = include_str!("../fixtures/large.json");
const INVENTORIES: &str = "/api/store/v1/inventories";
const ORDERS: &str = "/api/store/v1/orders";

async fn call(app: &Router, method: &str, path: &str, body: Value) -> (u16, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let code = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        code,
        if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        },
    )
}

async fn get(app: &Router, path: &str) -> Value {
    let (code, response) = call(app, "GET", path, json!({})).await;
    assert_eq!(code, 200, "{response}");
    response["data"].clone()
}

async fn post(app: &Router, path: &str, body: Value) -> Value {
    let (code, response) = call(app, "POST", path, body).await;
    assert!((200..300).contains(&code), "{response}");
    response["data"].clone()
}

async fn snapshot(app: &Router) -> Value {
    let lots = get(app, INVENTORIES).await;
    let orders = get(app, ORDERS).await;
    let filed = get(app, &format!("{ORDERS}?filed=true")).await;
    let mut items = vec![];
    for order in orders
        .as_array()
        .unwrap()
        .iter()
        .chain(filed.as_array().unwrap())
    {
        items.push(get(app, &format!("{ORDERS}/{}/items", order["order_id"])).await);
    }
    json!({"lots": lots, "orders": orders, "filed": filed, "items": items, "clock": get(app, "/__mock/clock").await})
}

fn fixture() -> Value {
    serde_json::from_str(SMALL).unwrap()
}

fn assert_invalid(value: Value, path: &str) {
    let error = app_from_fixture(&value.to_string()).expect_err("must reject invalid fixture");
    assert!(error.to_string().contains(path), "expected {path}: {error}");
}

#[tokio::test]
async fn custom_startup_loads_historical_orders_without_consuming_stock() {
    let app = app_from_fixture(SMALL).unwrap();
    let lots = get(&app, INVENTORIES).await;
    assert_eq!(lots.as_array().unwrap().len(), 6);
    assert_eq!(lots[0]["quantity"], 100);
    assert_eq!(lots[2]["quantity"], 0);
    assert_eq!(lots[0]["date_created"], "2026-01-01T12:00:00.000Z");
    let orders = get(&app, ORDERS).await;
    assert_eq!(orders.as_array().unwrap().len(), 3);
    assert_eq!(orders[1]["status"], "PAID");
    assert_eq!(orders[1]["buyer_name"], "synthetic_buyer_2");
    assert_eq!(orders[1]["cost"]["grand_total"], "1.0800");
    assert_eq!(
        get(&app, &format!("{ORDERS}?filed=true")).await[0]["status"],
        "COMPLETED"
    );
    // Startup validates the script, but must not execute its clock or mutations.
    assert_eq!(
        get(&app, "/__mock/clock").await["now"],
        "2026-01-01T12:00:00.000Z"
    );
}

#[tokio::test]
async fn replay_is_identical_across_resets_mutations_and_instances() {
    let first = app_from_fixture(SMALL).unwrap();
    let initial = snapshot(&first).await;
    let replay = post(&first, "/__mock/replay", json!({})).await;
    let result = snapshot(&first).await;
    assert_eq!(replay["steps"][1]["data"]["order_id"], 10004);
    assert_eq!(replay["steps"][3]["data"]["inventory_id"], 1006);
    assert_eq!(
        replay["steps"][5]["data"]["date_ordered"],
        "2026-01-01T12:03:00.000Z"
    );
    assert_eq!(
        get(&first, &format!("{INVENTORIES}/1000")).await["quantity"],
        97
    );
    assert_eq!(
        get(&first, &format!("{INVENTORIES}/1001")).await["quantity"],
        48
    );
    assert_eq!(
        get(&first, &format!("{INVENTORIES}/1006")).await["quantity"],
        16
    );
    post(&first, "/__mock/clock/advance", json!({"seconds": 999})).await;
    call(&first, "DELETE", &format!("{INVENTORIES}/1000"), json!({})).await;
    assert_eq!(post(&first, "/__mock/replay", json!({})).await, replay);
    assert_eq!(snapshot(&first).await, result);
    post(&first, "/__mock/reset", json!({})).await;
    assert_eq!(snapshot(&first).await, initial);
    assert_eq!(post(&first, "/__mock/replay", json!({})).await, replay);
    let second = app_from_fixture(SMALL).unwrap();
    assert_eq!(snapshot(&second).await, initial);
    assert_eq!(post(&second, "/__mock/replay", json!({})).await, replay);
    assert_eq!(snapshot(&second).await, result);
}

#[tokio::test]
async fn fixed_clock_advances_only_explicitly_and_rejects_bad_changes() {
    let app = app_from_fixture(SMALL).unwrap();
    let before = get(&app, "/__mock/clock").await;
    for body in [
        json!({"seconds": -1}),
        json!({"seconds": 1.5}),
        json!({"seconds": "2"}),
        json!({"seconds": u64::MAX}),
        json!({"seconds": 1, "extra": true}),
    ] {
        assert_eq!(
            call(&app, "POST", "/__mock/clock/advance", body).await.0,
            400
        );
        assert_eq!(get(&app, "/__mock/clock").await, before);
    }
    let advanced = post(&app, "/__mock/clock/advance", json!({"seconds": 3600})).await;
    assert_eq!(advanced["now"], "2026-01-01T13:00:00.000Z");
    let order = post(
        &app,
        "/__mock/orders",
        json!({"items": [{"inventory_id": 1000, "quantity": 1}]}),
    )
    .await;
    assert_eq!(order["date_ordered"], advanced["now"]);
    assert_eq!(get(&app, "/__mock/clock").await, advanced);
}

#[tokio::test]
async fn system_clock_mode_remains_available_but_is_not_replayable() {
    let default = app();
    assert_eq!(get(&default, "/__mock/clock").await["mode"], "system");
    assert_eq!(
        call(
            &default,
            "POST",
            "/__mock/clock/advance",
            json!({"seconds": 1})
        )
        .await
        .0,
        400
    );
    assert_eq!(
        call(&default, "POST", "/__mock/replay", json!({})).await.0,
        400
    );
    let mut fixture = fixture();
    fixture["clock"] = json!({"mode": "system"});
    fixture["steps"] = json!([]);
    let custom = app_from_fixture(&fixture.to_string()).unwrap();
    assert_eq!(get(&custom, INVENTORIES).await.as_array().unwrap().len(), 6);
}

#[tokio::test]
async fn separate_scenarios_have_no_shared_state() {
    let first = app_from_fixture(SMALL).unwrap();
    let mut different = fixture();
    different["inventories"][0]["inventory"]["quantity"] = json!(250);
    different["clock"]["now"] = json!("2020-01-01T00:00:00Z");
    let second = app_from_fixture(&different.to_string()).unwrap();
    let before = snapshot(&second).await;
    post(&first, "/__mock/replay", json!({})).await;
    post(&first, "/__mock/reset", json!({})).await;
    assert_eq!(snapshot(&second).await, before);
    assert_eq!(
        get(&first, &format!("{INVENTORIES}/1000")).await["quantity"],
        100
    );
    assert_eq!(
        get(&second, &format!("{INVENTORIES}/1000")).await["quantity"],
        250
    );
}

#[test]
fn invalid_fields_have_actionable_paths() {
    for (pointer, value, path) in [
        (
            "/steps/1/items/0/quantity",
            json!("many"),
            "$.steps[1].items[0].quantity",
        ),
        (
            "/steps/3/inventory/quantity",
            json!("many"),
            "$.steps[3].inventory.quantity",
        ),
        ("/clock/now", json!(42), "$.clock.now"),
        ("/version", json!(2), "$.version"),
        ("/clock/now", json!("tomorrow"), "$.clock.now"),
        (
            "/clock/now",
            json!("2026-01-01T00:00:00.0001Z"),
            "$.clock.now",
        ),
        (
            "/inventories/0/inventory/quantity",
            json!(-1),
            "$.inventories[0].inventory.quantity",
        ),
        (
            "/inventories/0/inventory/quantity",
            json!("many"),
            "inventories[0].inventory.quantity",
        ),
        (
            "/inventories/0/inventory/unit_price",
            json!("NaN"),
            "$.inventories[0].inventory.unit_price",
        ),
        (
            "/inventories/0/inventory/item/no",
            json!(""),
            "$.inventories[0].inventory.item.no",
        ),
        (
            "/inventories/0/inventory/item/type",
            json!("INVALID"),
            "$.inventories[0].inventory.item.type",
        ),
        (
            "/inventories/0/inventory/new_or_used",
            json!("X"),
            "$.inventories[0].inventory.new_or_used",
        ),
        (
            "/inventories/0/inventory/stock_room_id",
            json!("D"),
            "$.inventories[0].inventory.stock_room_id",
        ),
        (
            "/inventories/0/inventory_id",
            json!(0),
            "$.inventories[0].inventory_id",
        ),
        (
            "/inventories/0/inventory_id",
            json!(u64::MAX),
            "$.inventories[0].inventory_id",
        ),
        (
            "/inventories/1/inventory_id",
            json!(1000),
            "$.inventories[1].inventory_id",
        ),
        ("/orders/1/order_id", json!(10000), "$.orders[1].order_id"),
        (
            "/orders/0/items/0/inventory_id",
            json!(99),
            "$.orders[0].items[0].inventory_id",
        ),
        (
            "/orders/0/items/0/quantity",
            json!(0),
            "$.orders[0].items[0].quantity",
        ),
        ("/orders/0/status", json!("BOGUS"), "$.orders[0].status"),
        (
            "/orders/0/date_ordered",
            json!("yesterday"),
            "$.orders[0].date_ordered",
        ),
        (
            "/steps/1/items/0/quantity",
            json!(101),
            "$.steps[1].items[0].quantity",
        ),
        (
            "/steps/2/update/quantity",
            json!("5"),
            "$.steps[2].update.quantity",
        ),
        (
            "/steps/2/update/unit_price",
            json!("-1"),
            "$.steps[2].update.unit_price",
        ),
        ("/steps/0/seconds", json!(u64::MAX), "$.steps[0].seconds"),
    ] {
        let mut fixture = fixture();
        let (parent, key) = pointer.rsplit_once('/').unwrap();
        fixture
            .pointer_mut(parent)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert(key.into(), value);
        assert_invalid(fixture, path);
    }
    let mut missing = fixture();
    missing["inventories"][0]["inventory"]
        .as_object_mut()
        .unwrap()
        .remove("color_id");
    let error = app_from_fixture(&missing.to_string())
        .err()
        .unwrap()
        .to_string();
    assert!(
        error.contains("inventories[0].inventory") && error.contains("color_id"),
        "{error}"
    );
    let mut unknown = fixture();
    unknown["inventories"][0]["inventory"]["unexpected"] = json!(true);
    assert_invalid(unknown, "unexpected");
    let mut system = fixture();
    system["clock"] = json!({"mode": "system"});
    assert_invalid(system, "$.clock.mode");
    assert!(app_from_fixture("{").is_err());
    assert!(app_from_fixture(&(SMALL.to_owned() + "{}")).is_err());
}

#[test]
fn cli_rejects_invalid_fixture_before_serving() {
    let path = std::env::temp_dir().join(format!(
        "bricklink-invalid-fixture-{}.json",
        std::process::id()
    ));
    let mut invalid = fixture();
    invalid["version"] = json!(999);
    std::fs::write(&path, invalid.to_string()).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_bricklink-sandbox"))
        .arg("--fixture")
        .arg(&path)
        .args(["--bind", "127.0.0.1:0"])
        .output()
        .unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("$.version")
            && stderr.contains(path.file_name().unwrap().to_str().unwrap()),
        "{stderr}"
    );
    assert!(output.stdout.is_empty());
}

#[tokio::test]
async fn large_fixture_is_diverse_and_replayable() {
    let app = app_from_fixture(LARGE).unwrap();
    let lots = get(&app, INVENTORIES).await;
    assert_eq!(lots.as_array().unwrap().len(), 3000);
    assert!(
        lots.as_array()
            .unwrap()
            .iter()
            .any(|lot| lot["quantity"] == 0)
    );
    assert!(
        lots.as_array()
            .unwrap()
            .iter()
            .any(|lot| lot["is_stock_room"] == true)
    );
    for field in ["color_id", "new_or_used", "remarks"] {
        let values: std::collections::BTreeSet<_> = lots
            .as_array()
            .unwrap()
            .iter()
            .map(|lot| lot[field].to_string())
            .collect();
        assert!(values.len() > 1);
    }
    let orders = get(&app, ORDERS).await;
    assert_eq!(orders.as_array().unwrap().len(), 150);
    assert_eq!(
        get(&app, &format!("{ORDERS}?filed=true"))
            .await
            .as_array()
            .unwrap()
            .len(),
        50
    );
    let replay = post(&app, "/__mock/replay", json!({})).await;
    assert_eq!(replay["steps"][1]["data"]["order_id"], 10200);
    assert_eq!(post(&app, "/__mock/replay", json!({})).await, replay);
}

#[tokio::test]
async fn empty_fixtures_and_unsorted_ids_work() {
    let mut custom = fixture();
    custom["steps"] = json!([]);
    custom["inventories"].as_array_mut().unwrap().reverse();
    custom["orders"].as_array_mut().unwrap().reverse();
    let app = app_from_fixture(&custom.to_string()).unwrap();
    let created = post(
        &app,
        "/__mock/orders",
        json!({"items": [{"inventory_id": 1000, "quantity": 1}]}),
    )
    .await;
    assert_eq!(created["order_id"], 10004);
    let created = post(
        &app,
        INVENTORIES,
        custom["inventories"][0]["inventory"].clone(),
    )
    .await;
    assert_eq!(created["inventory_id"], 1006);
    custom["inventories"] = json!([]);
    custom["orders"] = json!([]);
    let empty = app_from_fixture(&custom.to_string()).unwrap();
    assert_eq!(get(&empty, INVENTORIES).await, json!([]));
    assert_eq!(
        post(&empty, "/__mock/replay", json!({})).await["steps"],
        json!([])
    );
}
