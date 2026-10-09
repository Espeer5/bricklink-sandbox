use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use bricklink_sandbox::app_configured;
use serde_json::{Value, json};
use tower::ServiceExt;
async fn raw(app: &Router, method: &str, path: &str, body: Value) -> axum::response::Response {
    app.clone()
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
        .unwrap()
}
async fn call(app: &Router, method: &str, path: &str, body: Value) -> (u16, Value) {
    let r = raw(app, method, path, body).await;
    let status = r.status().as_u16();
    (
        status,
        serde_json::from_slice(&to_bytes(r.into_body(), 1024 * 1024).await.unwrap()).unwrap(),
    )
}
fn rule(id: &str, method: &str, path: &str, phase: &str, effect: Value) -> Value {
    json!({"id":id,"method":method,"path":path,"phase":phase,"occurrences":[1],"effect":effect})
}
const LOT: &str = "/api/store/v1/inventories/1000";
async fn qty(app: &Router) -> i64 {
    call(app, "GET", LOT, Value::Null).await.1["data"]["quantity"]
        .as_i64()
        .unwrap()
}
#[tokio::test]
async fn before_faults_are_atomic_resettable_and_exactly_scoped() {
    let config =
        json!({"faults":[rule("reject","PUT",LOT,"before",json!({"kind":"error","status":503}))]});
    let app = app_configured(None, None, Some(&config.to_string())).unwrap();
    assert_eq!(qty(&app).await, 100);
    assert_eq!(
        call(&app, "PUT", LOT, json!({"quantity":"-3"})).await.0,
        503
    );
    assert_eq!(qty(&app).await, 100);
    assert_eq!(
        call(&app, "PUT", LOT, json!({"quantity":"-3"})).await.0,
        200
    );
    assert_eq!(qty(&app).await, 97);
    let diagnostic = call(&app, "GET", "/__mock/validation", Value::Null).await.1;
    assert_eq!(diagnostic["data"]["counters"]["reject"], 2);
    assert_eq!(diagnostic["data"]["events"][0]["occurrence"], 1);
    assert_eq!(diagnostic["data"]["events"][0]["dispatch_succeeded"], false);
    call(&app, "POST", "/__mock/faults/reset", json!({})).await;
    assert_eq!(
        call(&app, "PUT", LOT, json!({"quantity":"-3"})).await.0,
        503
    );
    assert_eq!(qty(&app).await, 97);
    call(&app, "POST", "/__mock/faults", json!([])).await;
    assert_eq!(
        call(&app, "PUT", LOT, json!({"quantity":"-3"})).await.0,
        200
    );
    assert_eq!(qty(&app).await, 94);
}
#[tokio::test]
async fn committed_update_with_lost_response_exposes_unsafe_retry() {
    let config = json!({"faults":[rule("lost","PUT",LOT,"after",json!({"kind":"disconnect"}))]});
    let app = app_configured(None, None, Some(&config.to_string())).unwrap();
    let response = raw(&app, "PUT", LOT, json!({"quantity":"-3"})).await;
    assert!(
        to_bytes(response.into_body(), 1024).await.is_err(),
        "response is lost, not a fabricated success body"
    );
    assert_eq!(qty(&app).await, 97, "update already committed");
    assert_eq!(
        call(&app, "PUT", LOT, json!({"quantity":"-3"})).await.0,
        200
    );
    assert_eq!(qty(&app).await, 94, "no invented idempotency guarantee");
    let data = call(&app, "GET", "/__mock/validation", Value::Null).await.1;
    assert_eq!(data["data"]["events"][0]["dispatch_succeeded"], true);
    call(&app, "POST", "/__mock/reset", json!({})).await;
    assert_eq!(qty(&app).await, 100);
    assert!(
        to_bytes(
            raw(&app, "PUT", LOT, json!({"quantity":"-3"}))
                .await
                .into_body(),
            1024
        )
        .await
        .is_err()
    );
}
#[tokio::test]
async fn rate_limits_malformed_content_and_delays_are_reproducible() {
    let app = bricklink_sandbox::app();
    let mut first = rule(
        "limit",
        "GET",
        LOT,
        "before",
        json!({"kind":"rate_limit","retry_after_seconds":7}),
    );
    first["occurrences"] = json!([1, 3]);
    call(&app, "POST", "/__mock/faults", json!([first])).await;
    for expected in [429, 200, 429, 200] {
        let response = raw(&app, "GET", LOT, Value::Null).await;
        assert_eq!(response.status().as_u16(), expected);
        if expected == 429 {
            assert_eq!(response.headers()["retry-after"], "7");
        }
    }
    call(
        &app,
        "POST",
        "/__mock/faults",
        json!([rule(
            "malformed",
            "GET",
            LOT,
            "before",
            json!({"kind":"malformed"})
        )]),
    )
    .await;
    let response = raw(&app, "GET", LOT, Value::Null).await;
    assert_eq!(response.status(), 200);
    assert!(
        serde_json::from_slice::<Value>(&to_bytes(response.into_body(), 1024).await.unwrap())
            .is_err()
    );
    for phase in ["before", "after"] {
        call(&app, "POST", "/__mock/reset", json!({})).await;
        call(
            &app,
            "POST",
            "/__mock/faults",
            json!([rule(
                "slow",
                "PUT",
                LOT,
                phase,
                json!({"kind":"delay","milliseconds":200})
            )]),
        )
        .await;
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(30),
                raw(&app, "PUT", LOT, json!({"quantity":"-3"}))
            )
            .await
            .is_err()
        );
        assert_eq!(qty(&app).await, if phase == "after" { 97 } else { 100 });
    }
}
#[tokio::test]
async fn after_fault_does_not_hide_validation_failure_and_configuration_is_atomic() {
    let app = bricklink_sandbox::app();
    let rules = json!([rule(
        "after",
        "PUT",
        LOT,
        "after",
        json!({"kind":"error","status":500})
    )]);
    assert_eq!(call(&app, "POST", "/__mock/faults", rules).await.0, 200);
    assert_eq!(
        call(&app, "POST", "/__mock/faults", json!([{"bad":"config"}]))
            .await
            .0,
        400
    );
    assert_eq!(
        call(&app, "PUT", LOT, json!({"quantity":"-1000"})).await.0,
        400
    );
    assert_eq!(qty(&app).await, 100);
    let events = call(&app, "GET", "/__mock/validation", Value::Null).await.1;
    assert_eq!(events["data"]["events"][0]["dispatch_succeeded"], false);
}
#[tokio::test]
async fn concurrent_rules_count_requests_once_without_blocking_other_routes() {
    let app = bricklink_sandbox::app();
    call(
        &app,
        "POST",
        "/__mock/faults",
        json!([rule(
            "slow",
            "PUT",
            LOT,
            "before",
            json!({"kind":"delay","milliseconds":100})
        )]),
    )
    .await;
    let first = raw(&app, "PUT", LOT, json!({"quantity":"-3"}));
    let second = raw(&app, "PUT", LOT, json!({"quantity":"-2"}));
    let (a, b) = tokio::join!(first, second);
    assert_eq!(a.status(), 200);
    assert_eq!(b.status(), 200);
    assert_eq!(qty(&app).await, 95);
    let data = call(&app, "GET", "/__mock/validation", Value::Null).await.1;
    assert_eq!(data["data"]["counters"]["slow"], 2);
    assert_eq!(data["data"]["events"].as_array().unwrap().len(), 1);
    assert_eq!(data["data"]["events"][0]["occurrence"], 1);
}

#[tokio::test]
async fn lost_response_is_observable_over_a_real_local_http_connection() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let config =
        json!({"faults":[rule("wire-loss","PUT",LOT,"after",json!({"kind":"disconnect"}))]});
    let app = app_configured(None, None, Some(&config.to_string())).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let serving = app.clone();
    let task = tokio::spawn(async move { axum::serve(listener, serving).await.unwrap() });
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    let body = r#"{"quantity":"-3"}"#;
    socket.write_all(format!("PUT {LOT} HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
    let mut bytes = Vec::new();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        socket.read_to_end(&mut bytes),
    )
    .await
    .expect("connection should close rather than hang");
    assert!(
        result.is_err() || !String::from_utf8_lossy(&bytes).contains("\"quantity\":97"),
        "client must not receive the successful resource"
    );
    assert_eq!(qty(&app).await, 97);
    assert_eq!(
        call(&app, "GET", "/__mock/validation", Value::Null).await.1["data"]["events"][0]["dispatch_succeeded"],
        true
    );
    task.abort();
    let _ = task.await;
}
