use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use bricklink_sandbox::{BASE, app};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn raw(
    app: &Router,
    method: &str,
    path: &str,
    body: String,
    content_type: &str,
) -> (u16, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", content_type)
                .body(Body::from(body))
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

async fn call(app: &Router, method: &str, path: &str, body: Value) -> (u16, Value) {
    raw(
        app,
        method,
        path,
        if body.is_null() {
            String::new()
        } else {
            body.to_string()
        },
        "application/json",
    )
    .await
}

async fn quantity(app: &Router) -> i64 {
    call(app, "GET", &format!("{BASE}/inventories/1000"), Value::Null)
        .await
        .1["data"]["quantity"]
        .as_i64()
        .unwrap()
}

async fn order(app: &Router, count: i64) -> (u16, Value) {
    call(
        app,
        "POST",
        "/__mock/orders",
        json!({"items": [{"inventory_id": 1000, "quantity": count}]}),
    )
    .await
}

#[tokio::test]
async fn order_deducts_stock_and_preserves_prices() {
    let app = app();
    let (code, result) = order(&app, 3).await;
    assert_eq!(code, 201);
    assert_eq!(quantity(&app).await, 97);
    assert_eq!(result["data"]["cost"]["grand_total"], "0.4500");
    let orders = call(&app, "GET", &format!("{BASE}/orders"), Value::Null)
        .await
        .1;
    assert_eq!(orders["data"].as_array().unwrap().len(), 1);
    call(
        &app,
        "PUT",
        &format!("{BASE}/inventories/1000"),
        json!({"unit_price": "1.00"}),
    )
    .await;
    let items = call(
        &app,
        "GET",
        &format!("{BASE}/orders/10000/items"),
        Value::Null,
    )
    .await
    .1;
    assert_eq!(items["data"][0][0]["quantity"], 3);
    assert_eq!(items["data"][0][0]["unit_price"], "0.1500");
}

#[tokio::test]
async fn failed_multiline_order_is_atomic() {
    let app = app();
    let (code, _) = call(
        &app,
        "POST",
        "/__mock/orders",
        json!({"items": [
        {"inventory_id": 1000, "quantity": 3}, {"inventory_id": 9999, "quantity": 1}]}),
    )
    .await;
    assert_eq!(code, 404);
    assert_eq!(quantity(&app).await, 100);
    assert_eq!(
        call(&app, "GET", &format!("{BASE}/orders"), Value::Null)
            .await
            .1["data"],
        json!([])
    );
}

#[tokio::test]
async fn duplicate_lines_are_aggregated_before_stock_check() {
    let app = app();
    let (code, _) = call(
        &app,
        "POST",
        "/__mock/orders",
        json!({"items": [
        {"inventory_id": 1000, "quantity": 60}, {"inventory_id": 1000, "quantity": 60}]}),
    )
    .await;
    assert_eq!(code, 422);
    assert_eq!(quantity(&app).await, 100);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_orders_cannot_oversell() {
    let app = app();
    let other = app.clone();
    let first = tokio::spawn(async move { order(&other, 60).await });
    let second = order(&app, 60).await;
    let mut codes = vec![first.await.unwrap().0, second.0];
    codes.sort();
    assert_eq!(codes, [201, 422]);
    assert_eq!(quantity(&app).await, 40);
}

#[tokio::test]
async fn delta_updates_are_validated_atomically() {
    let app = app();
    let path = format!("{BASE}/inventories/1000");
    assert_eq!(
        call(&app, "PUT", &path, json!({"quantity": "-3"})).await.0,
        200
    );
    for body in [
        json!({"quantity": 3}),
        json!({"quantity": "3"}),
        json!({"quantity": "-98"}),
        json!({"quantity": "-2", "unit_price": "NaN"}),
        json!({"quantity": "+9223372036854775807"}),
    ] {
        assert_eq!(call(&app, "PUT", &path, body).await.0, 400);
    }
    assert_eq!(quantity(&app).await, 97);
}

#[tokio::test]
async fn inventory_create_filter_and_delete() {
    let app = app();
    let (code, result) = call(
        &app,
        "POST",
        &format!("{BASE}/inventories"),
        json!({
        "item": {"no": "3002", "type": "PART"}, "color_id": 1,
        "quantity": 5, "unit_price": "0.2", "new_or_used": "U"}),
    )
    .await;
    assert_eq!(code, 201);
    assert_eq!(result["data"]["unit_price"], "0.2000");
    let lots = call(
        &app,
        "GET",
        &format!("{BASE}/inventories?color_id=1&item_type=part"),
        Value::Null,
    )
    .await
    .1;
    assert_eq!(lots["data"].as_array().unwrap().len(), 1);
    assert_eq!(lots["data"][0]["inventory_id"], 1001);
    let path = format!("{BASE}/inventories/1001");
    assert_eq!(
        call(&app, "DELETE", &path, Value::Null).await,
        (204, Value::Null)
    );
    assert_eq!(call(&app, "GET", &path, Value::Null).await.0, 404);
}

#[tokio::test]
async fn filters_and_reset() {
    let app = app();
    order(&app, 3).await;
    for query in ["direction=out", "status=-PENDING", "filed=true"] {
        assert_eq!(
            call(&app, "GET", &format!("{BASE}/orders?{query}"), Value::Null)
                .await
                .1["data"],
            json!([])
        );
    }
    call(&app, "POST", "/__mock/reset", Value::Null).await;
    assert_eq!(quantity(&app).await, 100);
    assert_eq!(
        call(&app, "GET", &format!("{BASE}/orders"), Value::Null)
            .await
            .1["data"],
        json!([])
    );
}

#[tokio::test]
async fn malformed_and_unsupported_requests_have_json_errors() {
    let app = app();
    for body in ["{", "null", "[]", "{\"items\":[]}", "{\"items\":null}"] {
        let (code, result) = raw(
            &app,
            "POST",
            "/__mock/orders",
            body.into(),
            "application/json",
        )
        .await;
        assert_eq!(code, 400);
        assert_eq!(result["meta"]["code"], 400);
    }
    assert_eq!(
        call(&app, "POST", &format!("{BASE}/orders"), json!({}))
            .await
            .0,
        405
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("{BASE}/inventories?unknown=yes"),
            Value::Null
        )
        .await
        .0,
        400
    );
}

#[tokio::test]
async fn form_encoded_json_is_supported() {
    let app = app();
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("data", "{\"quantity\":\"+2\"}")
        .finish();
    assert_eq!(
        raw(
            &app,
            "PUT",
            &format!("{BASE}/inventories/1000"),
            body,
            "application/x-www-form-urlencoded"
        )
        .await
        .0,
        200
    );
    assert_eq!(quantity(&app).await, 102);
}

#[tokio::test]
async fn stockroom_inventory_is_not_purchasable() {
    let app = app();
    call(&app, "POST", &format!("{BASE}/inventories"), json!({"item": {"no": "3002", "type": "PART"},
        "color_id": 1, "quantity": 5, "unit_price": "0.2", "new_or_used": "U", "is_stock_room": true})).await;
    assert_eq!(
        call(
            &app,
            "POST",
            "/__mock/orders",
            json!({"items": [{"inventory_id": 1001, "quantity": 1}]})
        )
        .await
        .0,
        422
    );
    let lots = call(
        &app,
        "GET",
        &format!("{BASE}/inventories?status=S"),
        Value::Null,
    )
    .await
    .1;
    assert_eq!(lots["data"][0]["quantity"], 5);
}
