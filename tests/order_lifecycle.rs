use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use bricklink_sandbox::app_from_fixture;
use serde_json::{Value, json};
use tower::ServiceExt;

const BASE: &str = "/api/store/v1/orders/10004";
async fn call(app: &Router, method: &str, path: &str, body: Value, expected: u16) -> Value {
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
    let value: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    assert_eq!(code, expected, "{method} {path}: {value}");
    value["data"].clone()
}
async fn get(app: &Router, path: &str) -> Value {
    call(app, "GET", path, json!({}), 200).await
}
async fn action(app: &Router, op: &str, body: Value, code: u16) -> Value {
    call(
        app,
        "POST",
        &format!("/__mock/orders/10004/{op}"),
        body,
        code,
    )
    .await
}
async fn patch(app: &Router, field: &str, value: &str, code: u16) -> Value {
    call(
        app,
        "PUT",
        &format!("{BASE}/{field}"),
        json!({"field":field,"value":value}),
        code,
    )
    .await
}
async fn stock(app: &Router, id: u64) -> Value {
    get(app, &format!("/api/store/v1/inventories/{id}")).await["quantity"].clone()
}
fn app() -> Router {
    app_from_fixture(include_str!("../fixtures/small.json")).unwrap()
}
async fn purchase(app: &Router, details: Value) -> Value {
    call(
        app,
        "POST",
        "/__mock/orders",
        json!({"items":[{"inventory_id":1000,"quantity":10}],"details":details}),
        201,
    )
    .await
}

#[tokio::test]
async fn rich_metadata_totals_and_direction_are_consistent() {
    let app = app();
    let order=purchase(&app,json!({"buyer_name":"synthetic_eu_buyer","buyer_email":"buyer@example.test","currency_code":"EUR",
        "shipping":{"method":"Synthetic post","address":{"name":{"full":"Example Buyer"},"country_code":"DE","city":"Example City","address1":"1 Test Street"}},
        "payment":{"method":"Synthetic transfer"},"cost":{"shipping":"2.50","insurance":"0.2","etc1":"0.12341","etc2":"0.1","credit":"0.2","coupon":"0.1"}})).await;
    assert_eq!(order["cost"]["subtotal"], "1.5000");
    assert_eq!(order["cost"]["grand_total"], "4.1235");
    assert_eq!(order["shipping"]["address"]["country_code"], "DE");
    assert_eq!(stock(&app, 1000).await, 90);
    let items = get(&app, &format!("{BASE}/items")).await;
    assert_eq!(items[0][0]["currency_code"], "EUR");
    assert_eq!(items[0][0]["unit_price_final"], "0.1500");
    let listed = get(&app, "/api/store/v1/orders").await;
    assert_eq!(
        listed[3]["cost"]["grand_total"],
        order["cost"]["grand_total"]
    );
    assert!(listed[3].get("shipping").is_none());
    call(&app, "PUT", BASE, json!({"is_filed":true}), 200).await;
    assert_eq!(
        get(&app, "/api/store/v1/orders")
            .await
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        get(&app, "/api/store/v1/orders?filed=true")
            .await
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let out = purchase(
        &app,
        json!({"direction":"out","currency_code":"GBP","seller_name":"synthetic_other_seller"}),
    )
    .await;
    assert_eq!(out["order_id"], 10005);
    assert_eq!(stock(&app, 1000).await, 90);
    assert_eq!(
        get(&app, "/api/store/v1/orders?direction=out").await[0]["cost"]["currency_code"],
        "GBP"
    );
}

#[tokio::test]
async fn documented_updates_roles_and_completed_window() {
    let app = app();
    purchase(&app, json!({})).await;
    let changed=call(&app,"PUT",BASE,json!({"buyer_name":"ignored","status":"CANCELLED","cost":{"shipping":"1.2","coupon":"99"},"shipping":{"tracking_no":"TEST123","tracking_link":"https://example.test/track","date_shipped":"2026-01-01T13:00:00+01:00","method_id":42,"address":"ignored"},"remarks":"packed"}),200).await;
    assert_eq!(changed["status"], "PENDING");
    assert_eq!(changed["buyer_name"], "mock_buyer");
    assert_eq!(changed["cost"]["grand_total"], "2.7000");
    assert_eq!(
        changed["shipping"]["date_shipped"],
        "2026-01-01T12:00:00.000Z"
    );
    patch(&app, "status", "RECEIVED", 422).await;
    patch(&app, "status", "OCR", 422).await;
    patch(&app, "payment_status", "Sent", 422).await;
    call(
        &app,
        "PUT",
        &format!("{BASE}/status"),
        json!({"field":"wrong","value":"PAID"}),
        400,
    )
    .await;
    assert_eq!(patch(&app, "status", "PAID", 200).await, Value::Null);
    let paid = get(&app, BASE).await;
    assert_eq!(paid["payment"]["date_paid"], "2026-01-01T12:00:00.000Z");
    let updated = call(
        &app,
        "PUT",
        BASE,
        json!({"cost":"ignored while paid","remarks":"paid"}),
        200,
    )
    .await;
    assert_eq!(updated["cost"], paid["cost"]);
    patch(&app, "status", "COMPLETED", 200).await;
    call(
        &app,
        "POST",
        "/__mock/clock/advance",
        json!({"seconds":604800}),
        200,
    )
    .await;
    patch(&app, "status", "SHIPPED", 200).await;
    patch(&app, "status", "COMPLETED", 200).await;
    call(
        &app,
        "POST",
        "/__mock/clock/advance",
        json!({"seconds":604801}),
        200,
    )
    .await;
    patch(&app, "status", "SHIPPED", 422).await;
    assert_eq!(stock(&app, 1000).await, 90);
}

#[tokio::test]
async fn outgoing_buyer_payment_and_status_restrictions() {
    let app = app();
    purchase(&app, json!({"direction":"out"})).await;
    patch(&app, "payment_status", "Sent", 200).await;
    patch(&app, "payment_status", "Received", 422).await;
    patch(&app, "status", "SHIPPED", 422).await;
    patch(&app, "status", "RECEIVED", 200).await;
    patch(&app, "status", "COMPLETED", 200).await;
    assert_eq!(stock(&app, 1000).await, 100);
    let other = app_from_fixture(&{
        let mut f: Value = serde_json::from_str(include_str!("../fixtures/small.json")).unwrap();
        f["orders"][0]["details"] = json!({"direction":"out","payment":{"status":"Received"}});
        f.to_string()
    })
    .unwrap();
    call(
        &other,
        "PUT",
        "/api/store/v1/orders/10000/payment_status",
        json!({"field":"payment_status","value":"Sent"}),
        422,
    )
    .await;
}

#[tokio::test]
async fn edits_preserve_prices_and_apply_only_stock_differences_atomically() {
    let app = app();
    purchase(&app, json!({})).await;
    call(
        &app,
        "PUT",
        "/api/store/v1/inventories/1000",
        json!({"unit_price":"9","remarks":"changed"}),
        200,
    )
    .await;
    let edit = json!({"items":[{"inventory_id":1000,"quantity":3},{"inventory_id":1000,"quantity":2},{"inventory_id":1001,"quantity":2}]});
    let changed = action(&app, "items", edit.clone(), 200).await;
    assert_eq!(changed["cost"]["subtotal"], "0.9100");
    assert_eq!(changed["status"], "UPDATED");
    assert_eq!(stock(&app, 1000).await, 95);
    assert_eq!(stock(&app, 1001).await, 38);
    action(&app, "items", edit, 200).await;
    assert_eq!(stock(&app, 1000).await, 95);
    let before = get(&app, &format!("{BASE}/items")).await;
    assert_eq!(before[0][0]["remarks"], "BIN-A01");
    action(
        &app,
        "items",
        json!({"items":[{"inventory_id":1000,"quantity":6},{"inventory_id":1001,"quantity":1000}]}),
        422,
    )
    .await;
    assert_eq!(stock(&app, 1000).await, 95);
    assert_eq!(get(&app, &format!("{BASE}/items")).await, before);
    action(
        &app,
        "items",
        json!({"items":[{"inventory_id":1001,"quantity":2}]}),
        200,
    )
    .await;
    assert_eq!(stock(&app, 1000).await, 100);
    action(&app, "items", json!({"items":[]}), 400).await;
    patch(&app, "status", "PAID", 200).await;
    action(
        &app,
        "items",
        json!({"items":[{"inventory_id":1001,"quantity":3}]}),
        422,
    )
    .await;
}

#[tokio::test]
async fn refunds_retries_cancellation_and_restock_share_one_reservation() {
    let app = app();
    purchase(&app, json!({})).await;
    action(
        &app,
        "refund",
        json!({"operation_id":"early","amount":"0.1"}),
        422,
    )
    .await;
    patch(&app, "payment_status", "Received", 200).await;
    let refund =
        json!({"operation_id":"r1","amount":"0.3000","items":[{"inventory_id":1000,"quantity":2}]});
    let (a, b) = tokio::join!(
        action(&app, "refund", refund.clone(), 200),
        action(&app, "refund", refund.clone(), 200)
    );
    assert_eq!(a, b);
    assert_eq!(stock(&app, 1000).await, 92);
    assert_eq!(a["refunded"], "0.3000");
    action(
        &app,
        "refund",
        json!({"operation_id":"r1","amount":"0.4"}),
        422,
    )
    .await;
    action(
        &app,
        "refund",
        json!({"operation_id":"too-much","amount":"2"}),
        422,
    )
    .await;
    action(&app,"refund",json!({"operation_id":"bad-stock","amount":"0.1","items":[{"inventory_id":1000,"quantity":9}]}),422).await;
    assert_eq!(
        get(&app, "/__mock/orders/10004/state").await["refunded"],
        "0.3000"
    );
    action(&app, "cancel", json!({"restock":false}), 200).await;
    assert_eq!(stock(&app, 1000).await, 92);
    let restock = json!({"operation_id":"s1","items":[{"inventory_id":1000,"quantity":3}]});
    action(&app, "restock", restock.clone(), 200).await;
    action(&app, "restock", restock, 200).await;
    assert_eq!(stock(&app, 1000).await, 95);
    action(&app, "cancel", json!({}), 200).await;
    action(&app, "cancel", json!({}), 200).await;
    assert_eq!(stock(&app, 1000).await, 100);
    action(
        &app,
        "restock",
        json!({"operation_id":"s2","items":[{"inventory_id":1000,"quantity":1}]}),
        422,
    )
    .await;
    action(
        &app,
        "refund",
        json!({"operation_id":"r2","amount":"1.2"}),
        200,
    )
    .await;
    assert_eq!(get(&app, BASE).await["payment"]["status"], "Returned");
    assert_eq!(action(&app, "refund", refund, 200).await, a); // original receipt, not current totals
    patch(&app, "status", "PROCESSING", 422).await;
    patch(&app, "payment_status", "None", 200).await;
    call(&app, "PUT", BASE, json!({"cost":{"credit":"1"}}), 200).await;
    assert_eq!(get(&app, BASE).await["cost"]["grand_total"], "1.5000");
}

#[tokio::test]
async fn failed_creation_and_missing_restock_lot_leave_state_unchanged() {
    let app = app();
    for details in [
        json!({"currency_code":"eur"}),
        json!({"cost":{"credit":"2"}}),
        json!({"shipping":{"date_shipped":"bad"}}),
    ] {
        call(
            &app,
            "POST",
            "/__mock/orders",
            json!({"items":[{"inventory_id":1000,"quantity":10}],"details":details}),
            400,
        )
        .await;
        assert_eq!(stock(&app, 1000).await, 100);
    }
    assert_eq!(purchase(&app, json!({})).await["order_id"], 10004);
    call(
        &app,
        "DELETE",
        "/api/store/v1/inventories/1000",
        json!({}),
        204,
    )
    .await;
    let before = get(&app, BASE).await;
    action(&app, "cancel", json!({}), 404).await;
    assert_eq!(get(&app, BASE).await, before);
    assert_eq!(
        get(&app, &format!("{BASE}/items")).await[0][0]["quantity"],
        10
    );
}

#[tokio::test]
async fn historical_reservations_are_opt_in_and_do_not_deduct_opening_stock() {
    let mut fixture: Value = serde_json::from_str(include_str!("../fixtures/small.json")).unwrap();
    fixture["orders"][0]["details"] =
        json!({"inventory_effects":"reserved","currency_code":"CAD","payment":{"method":"Test"}});
    let app = app_from_fixture(&fixture.to_string()).unwrap();
    assert_eq!(stock(&app, 1000).await, 100);
    call(&app, "POST", "/__mock/orders/10000/cancel", json!({}), 200).await;
    assert_eq!(stock(&app, 1000).await, 102);
    call(&app, "POST", "/__mock/orders/10000/cancel", json!({}), 200).await;
    call(&app, "POST", "/__mock/orders/10001/cancel", json!({}), 200).await;
    assert_eq!(stock(&app, 1000).await, 102);
    call(&app, "POST", "/__mock/reset", json!({}), 200).await;
    assert_eq!(stock(&app, 1000).await, 100);
    assert_eq!(
        get(&app, "/__mock/orders/10000/state").await["remaining_reserved"][0]["quantity"],
        2
    );
}

#[tokio::test]
async fn lifecycle_fixture_replays_receipts_and_reservations_deterministically() {
    let fixture = include_str!("../fixtures/lifecycle.json");
    let app = app_from_fixture(fixture).unwrap();
    assert_eq!(
        get(&app, "/api/store/v1/orders?direction=out&filed=true").await[0]["order_id"],
        10001
    );
    let first = call(&app, "POST", "/__mock/replay", json!({}), 200).await;
    assert_eq!(stock(&app, 1000).await, 100);
    assert_eq!(stock(&app, 1001).await, 40);
    let state = get(&app, "/__mock/orders/10002/state").await;
    assert_eq!(state["refunded"], "2.5100");
    assert_eq!(state["remaining_reserved"], json!([]));
    let order = get(&app, "/api/store/v1/orders/10002").await;
    assert_eq!(order["status"], "CANCELLED");
    assert_eq!(order["payment"]["status"], "Returned");
    assert_eq!(order["total_count"], 10);
    assert_eq!(order["is_filed"], true);
    assert_eq!(order["cost"]["grand_total"], "2.5100");
    assert_eq!(order["shipping"]["tracking_no"], "TEST-US-003");
    call(&app, "POST", "/__mock/reset", json!({}), 200).await;
    call(&app, "GET", "/__mock/orders/10002/state", json!({}), 404).await;
    assert_eq!(
        call(&app, "POST", "/__mock/replay", json!({}), 200).await,
        first
    );
    assert_eq!(
        call(
            &app_from_fixture(fixture).unwrap(),
            "POST",
            "/__mock/replay",
            json!({}),
            200
        )
        .await,
        first
    );
}

#[tokio::test]
async fn monetary_and_restock_failures_roll_back_all_affected_fields() {
    let app = app();
    purchase(
        &app,
        json!({"cost":{"credit":"1.00"},"payment":{"date_paid":"2025-12-31T12:00:00Z"}}),
    )
    .await;
    let before = get(&app, BASE).await;
    action(
        &app,
        "items",
        json!({"items":[{"inventory_id":1000,"quantity":1}]}),
        400,
    )
    .await;
    assert_eq!(get(&app, BASE).await, before);
    assert_eq!(stock(&app, 1000).await, 90);
    call(
        &app,
        "PUT",
        BASE,
        json!({"remarks":"must rollback","cost":{"credit":"2"}}),
        400,
    )
    .await;
    assert_eq!(get(&app, BASE).await, before);
    patch(&app, "status", "PAID", 200).await;
    assert_eq!(
        get(&app, BASE).await["payment"]["date_paid"],
        "2025-12-31T12:00:00.000Z"
    );
    call(
        &app,
        "PUT",
        "/api/store/v1/inventories/1000",
        json!({"quantity":"+999999910"}),
        200,
    )
    .await;
    action(&app,"refund",json!({"operation_id":"overflow","amount":"0.1","items":[{"inventory_id":1000,"quantity":1}]}),400).await;
    assert_eq!(
        get(&app, "/__mock/orders/10004/state").await["refunded"],
        "0.0000"
    );
    assert_eq!(stock(&app, 1000).await, 1000000000);
}

#[tokio::test]
async fn completed_window_uses_millisecond_precision() {
    let app = app();
    purchase(
        &app,
        json!({"status":"COMPLETED","date_status_changed":"2025-12-25T11:59:59.999Z"}),
    )
    .await;
    patch(&app, "status", "SHIPPED", 422).await;
    call(&app, "GET", &format!("{BASE}/status"), json!({}), 405).await;
    call(
        &app,
        "PUT",
        "/api/store/v1/orders/999/status",
        json!({"field":"status","value":"SHIPPED"}),
        404,
    )
    .await;
}
