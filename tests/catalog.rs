use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use bricklink_sandbox::{app_configured, app_from_fixture};
use serde_json::{Value, json};
use tower::ServiceExt;
async fn call(app: &Router, method: &str, path: &str, body: Value) -> (u16, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .body(Body::from(if body.is_null() {
                    String::new()
                } else {
                    body.to_string()
                }))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
const CATALOG: &str = include_str!("../fixtures/catalog.json");
#[tokio::test]
async fn catalog_resources_preserve_full_synthetic_metadata_and_relationships() {
    let app = app_configured(None, Some(CATALOG), None).unwrap();
    let fixture: Value = serde_json::from_str(CATALOG).unwrap();
    for (path, expected) in [
        ("/items/part/3001", fixture["items"][0]["item"].clone()),
        (
            "/items/PART/3001/colors",
            fixture["items"][0]["known_colors"].clone(),
        ),
        (
            "/items/PART/3001/images/5",
            fixture["items"][0]["images"]["5"].clone(),
        ),
        (
            "/items/SET/7784-1/subsets",
            fixture["items"][2]["subsets"].clone(),
        ),
        (
            "/items/PART/3001/supersets",
            fixture["items"][0]["supersets"].clone(),
        ),
        ("/colors", fixture["colors"].clone()),
        ("/categories", fixture["categories"].clone()),
        ("/colors/5", fixture["colors"][1].clone()),
        ("/categories/10", fixture["categories"][2].clone()),
        ("/items/MINIFIG/sh0001", fixture["items"][3]["item"].clone()),
    ] {
        let (status, response) =
            call(&app, "GET", &format!("/api/store/v1{path}"), Value::Null).await;
        assert_eq!(status, 200, "{path}");
        assert_eq!(response["data"], expected, "{path}");
    }
    let mappings = call(
        &app,
        "GET",
        "/api/store/v1/item_mapping/PART/3001?color_id=5",
        Value::Null,
    )
    .await;
    assert_eq!(mappings.1["data"].as_array().unwrap().len(), 1);
    let reverse = call(
        &app,
        "GET",
        "/api/store/v1/item_mapping/synthetic-shared-element",
        Value::Null,
    )
    .await;
    assert_eq!(
        reverse.1["data"].as_array().unwrap().len(),
        3,
        "ambiguous mappings remain multiple"
    );
}
#[tokio::test]
async fn exact_id_color_and_method_failures_do_not_invent_catalog_values() {
    let app = app_configured(None, Some(CATALOG), None).unwrap();
    for path in [
        "items/PART/unknown",
        "items/SET/3001",
        "items/PART/3001/images/99",
        "colors/99",
        "categories/99",
        "items/PART/3001/price",
        "item_mapping/missing",
    ] {
        assert_eq!(
            call(&app, "GET", &format!("/api/store/v1/{path}"), Value::Null)
                .await
                .0,
            404,
            "{path}"
        );
    }
    assert_eq!(
        call(&app, "PUT", "/api/store/v1/items/PART/3001", json!({}))
            .await
            .0,
        405
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/store/v1/items/PART/3001/subsets?break_minifigs=true",
            Value::Null
        )
        .await
        .0,
        400
    );
    let old = call(&app, "GET", "/api/store/v1/items/PART/3001old", Value::Null).await;
    assert_eq!(old.1["data"]["no"], "3001old");
    assert_eq!(old.1["data"]["is_obsolete"], true);
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/store/v1/items/PART/3001alternate",
            Value::Null
        )
        .await
        .0,
        404
    );
}
#[tokio::test]
async fn strict_inventory_validates_membership_without_overwriting_merchant_facts() {
    let app = app_configured(None, Some(CATALOG), None).unwrap();
    let valid = json!({"item":{"no":"3001","type":"PART"},"color_id":11,"quantity":7,"unit_price":"0.1234","new_or_used":"U","remarks":"  A & shelf/2  "});
    let (code, response) = call(&app, "POST", "/api/store/v1/inventories", valid.clone()).await;
    assert_eq!(code, 201);
    assert_eq!(response["data"]["remarks"], valid["remarks"]);
    assert_eq!(response["data"]["unit_price"], "0.1234");
    assert_eq!(response["data"]["quantity"], 7);
    for (field, value) in [
        ("color_id", json!(99)),
        ("item", json!({"no":"missing","type":"PART"})),
        ("item", json!({"no":"3001old","type":"PART"})),
        ("item", json!({"no":"3001","type":"PART","category_id":10})),
    ] {
        let mut body = valid.clone();
        body[field] = value;
        assert_eq!(
            call(&app, "POST", "/api/store/v1/inventories", body)
                .await
                .0,
            400
        );
    }
    assert_eq!(
        call(&app, "GET", "/api/store/v1/inventories", Value::Null)
            .await
            .1["data"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let permissive = bricklink_sandbox::app();
    let mut unknown = valid;
    unknown["item"]["no"] = json!("missing");
    assert_eq!(
        call(&permissive, "POST", "/api/store/v1/inventories", unknown)
            .await
            .0,
        201
    );
}
#[test]
fn embedded_and_sidecar_catalogs_validate_before_startup() {
    let catalog: Value = serde_json::from_str(CATALOG).unwrap();
    let fixture = json!({"version":1,"clock":{"mode":"fixed","now":"2026-01-01T00:00:00Z"},"inventories":[],"orders":[],"catalog":catalog});
    assert!(app_from_fixture(&fixture.to_string()).is_ok());
    for change in 0..5 {
        let mut bad = catalog.clone();
        match change {
            0 => bad["categories"][0]["parent_id"] = json!(5),
            1 => bad["items"][0]["known_colors"][0]["color_id"] = json!(999),
            2 => bad["items"][0]["item"]["category_id"] = json!(999),
            3 => bad["items"][0]["images"]["5"]["no"] = json!("wrong"),
            _ => bad["colors"][1]["color_id"] = json!(0),
        };
        assert!(app_configured(None, Some(&bad.to_string()), None).is_err());
    }
    let mut bad = catalog;
    bad["items"].as_array_mut().unwrap().remove(0);
    assert!(
        app_configured(None, Some(&bad.to_string()), None).is_err(),
        "seeded inventory must exist in strict catalog"
    );
}
