use axum::{
    Router,
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::Request,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use bricklink_sandbox::app_configured;
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha1::Sha1;
use std::collections::BTreeMap;
use tower::ServiceExt;
const NOW: &str = "1767225600";
const LOT: &str = "/api/store/v1/inventories/1000";
fn encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
        .replace('*', "%2A")
        .replace("%7E", "~")
}
fn params() -> BTreeMap<String, String> {
    [
        ("oauth_consumer_key", "dummy-consumer"),
        ("oauth_token", "dummy-token"),
        ("oauth_signature_method", "HMAC-SHA1"),
        ("oauth_timestamp", NOW),
        ("oauth_nonce", "unique+nonce /ß"),
        ("oauth_version", "1.0"),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v.into()))
    .collect()
}
fn sign(method: &str, path: &str, p: &mut BTreeMap<String, String>, body: &str, origin: &str) {
    let url = url::Url::parse(&format!("{origin}{path}")).unwrap();
    let mut fields: Vec<_> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    fields.extend(
        p.iter()
            .filter(|(k, _)| k.as_str() != "oauth_signature")
            .map(|(k, v)| (k.clone(), v.clone())),
    );
    if body.starts_with("data=") {
        fields.extend(url::form_urlencoded::parse(body.as_bytes()).into_owned());
    }
    let mut encoded: Vec<_> = fields.iter().map(|(k, v)| (encode(k), encode(v))).collect();
    encoded.sort();
    let normalized = encoded
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    let base = format!(
        "{method}&{}&{}",
        encode(&format!(
            "{}{}",
            url.origin().ascii_serialization(),
            url.path()
        )),
        encode(&normalized)
    );
    let mut mac =
        Hmac::<Sha1>::new_from_slice(b"dummy-consumer-secret&dummy-token-secret").unwrap();
    mac.update(base.as_bytes());
    p.insert(
        "oauth_signature".into(),
        STANDARD.encode(mac.finalize().into_bytes()),
    );
}
fn header(p: &BTreeMap<String, String>) -> String {
    format!(
        "OAuth {}",
        p.iter()
            .map(|(k, v)| format!("{}=\"{}\"", encode(k), encode(v)))
            .collect::<Vec<_>>()
            .join(", ")
    )
}
fn config() -> Value {
    serde_json::from_str(include_str!("../fixtures/validation.json")).unwrap()
}
fn app(config: Value) -> Router {
    app_configured(Some(r#"{"version":1,"clock":{"mode":"fixed","now":"2026-01-01T00:00:00Z"},"inventories":[],"orders":[]}"#),Some(include_str!("../fixtures/catalog.json")),Some(&config.to_string())).unwrap()
}
async fn send(
    app: &Router,
    method: &str,
    path: &str,
    header: Option<&str>,
    body: &str,
    form: bool,
) -> (u16, Value) {
    let mut request = Request::builder().method(method).uri(path).header(
        "content-type",
        if form {
            "application/x-www-form-urlencoded"
        } else {
            "application/json"
        },
    );
    if let Some(value) = header {
        request = request.header("authorization", value);
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
    (
        response.status().as_u16(),
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap(),
    )
}
#[tokio::test]
async fn valid_header_and_documented_json_query_authorization_work() {
    for query_mode in [false, true] {
        let app = app(config());
        let path = "/api/store/v1/items/PART/3001";
        let mut p = params();
        sign("GET", path, &mut p, "", "http://127.0.0.1:8000");
        let response = if query_mode {
            let escaped: BTreeMap<_, _> = p.iter().map(|(k, v)| (k.clone(), encode(v))).collect();
            send(
                &app,
                "GET",
                &format!(
                    "{path}?Authorization={}",
                    encode(&serde_json::to_string(&escaped).unwrap())
                ),
                None,
                "",
                false,
            )
            .await
        } else {
            send(&app, "GET", path, Some(&header(&p)), "", false).await
        };
        assert_eq!(response.0, 200);
        assert_eq!(response.1["data"]["no"], "3001");
        assert_eq!(
            send(&app, "GET", path, Some(&header(&p)), "", false)
                .await
                .0,
            401,
            "replay refused"
        );
    }
}
#[tokio::test]
async fn signing_normalizes_query_order_duplicates_encoding_and_local_origin() {
    let path = "/api/store/v1/inventories?item_type=P%41RT&status=Y&status=-S&color_id=5";
    let app = app(config());
    let mut p = params();
    sign("GET", path, &mut p, "", "http://127.0.0.1:8000");
    let reordered = "/api/store/v1/inventories?color_id=5&status=-S&item_type=PART&status=Y";
    assert_eq!(
        send(&app, "GET", reordered, Some(&header(&p)), "", false)
            .await
            .0,
        200
    );
    for origin in ["https://api.bricklink.com", "http://127.0.0.1:8001"] {
        let mut p = params();
        p.insert("oauth_nonce".into(), origin.into());
        sign("GET", LOT, &mut p, "", origin);
        assert_eq!(
            send(&app, "GET", LOT, Some(&header(&p)), "", false).await.0,
            401
        );
    }
    let mut p = params();
    p.insert("oauth_nonce".into(), "tampered".into());
    sign("GET", path, &mut p, "", "http://127.0.0.1:8000");
    assert_eq!(
        send(
            &app,
            "GET",
            "/api/store/v1/inventories?item_type=PART&status=Y&color_id=5",
            Some(&header(&p)),
            "",
            false
        )
        .await
        .0,
        401,
        "dropping duplicate changes signature"
    );
}
#[tokio::test]
async fn invalid_credentials_algorithms_timestamps_and_headers_fail_without_leaking_secrets() {
    let app = app(config());
    assert_eq!(send(&app, "GET", LOT, None, "", false).await.0, 401);
    for (key, value) in [
        ("oauth_consumer_key", "wrong"),
        ("oauth_token", "wrong"),
        ("oauth_signature_method", "PLAINTEXT"),
        ("oauth_version", "2.0"),
        ("oauth_timestamp", "0"),
        ("oauth_timestamp", "9999999999"),
        ("oauth_timestamp", "nope"),
        ("oauth_nonce", ""),
    ] {
        let mut p = params();
        p.insert(key.into(), value.into());
        sign("GET", LOT, &mut p, "", "http://127.0.0.1:8000");
        let response = send(&app, "GET", LOT, Some(&header(&p)), "", false).await;
        assert_eq!(response.0, 401, "{key}");
        assert!(!response.1.to_string().contains("dummy-consumer-secret"));
    }
    let mut p = params();
    sign("GET", LOT, &mut p, "", "http://127.0.0.1:8000");
    for bad in [
        format!("{}, oauth_nonce=\"duplicate\"", header(&p)),
        header(&p).replace("HMAC-SHA1", "%ZZ"),
        "Bearer secret".into(),
        "OAuth oauth_nonce=unquoted".into(),
    ] {
        assert_eq!(send(&app, "GET", LOT, Some(&bad), "", false).await.0, 401);
    }
    let diagnostic = send(&app, "GET", "/__mock/validation", None, "", false)
        .await
        .1
        .to_string();
    assert!(!diagnostic.contains("dummy-consumer"));
    assert!(!diagnostic.contains("dummy-token"));
}
#[tokio::test]
async fn authenticating_mutations_precedes_dispatch_and_form_extension_is_signed() {
    let app = app(config());
    let body = r#"{"item":{"no":"3001","type":"PART"},"color_id":5,"quantity":4,"unit_price":"0.1234","new_or_used":"N"}"#;
    assert_eq!(
        send(&app, "POST", "/api/store/v1/inventories", None, body, false)
            .await
            .0,
        401
    );
    for (index, (body, form)) in [
        (body.to_owned(), false),
        (format!("data={}", encode(body)), true),
        (encode(body), true),
    ]
    .into_iter()
    .enumerate()
    {
        let mut p = params();
        p.insert("oauth_nonce".into(), index.to_string());
        sign(
            "POST",
            "/api/store/v1/inventories",
            &mut p,
            &body,
            "http://127.0.0.1:8000",
        );
        assert_eq!(
            send(
                &app,
                "POST",
                "/api/store/v1/inventories",
                Some(&header(&p)),
                &body,
                form
            )
            .await
            .0,
            201
        );
    }
    let mut p = params();
    p.insert("oauth_nonce".into(), "final".into());
    sign(
        "GET",
        "/api/store/v1/inventories",
        &mut p,
        "",
        "http://127.0.0.1:8000",
    );
    assert_eq!(
        send(
            &app,
            "GET",
            "/api/store/v1/inventories",
            Some(&header(&p)),
            "",
            false
        )
        .await
        .1["data"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}
#[tokio::test]
async fn peer_ip_policy_uses_socket_not_forwarded_headers() {
    let mut cfg = config();
    cfg["auth"]["allowed_ips"] = json!(["127.0.0.1"]);
    let app = app(cfg);
    let path = "/api/store/v1/colors";
    let mut p = params();
    sign("GET", path, &mut p, "", "http://127.0.0.1:8000");
    assert_eq!(
        send(&app, "GET", path, Some(&header(&p)), "", false)
            .await
            .0,
        403
    );
    for (peer, expected) in [("127.0.0.2:4567", 403), ("127.0.0.1:4567", 200)] {
        let mut request = Request::builder()
            .uri(path)
            .header("authorization", header(&p))
            .header("x-forwarded-for", "127.0.0.1")
            .body(Body::empty())
            .unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo(peer.parse::<std::net::SocketAddr>().unwrap()));
        assert_eq!(
            app.clone()
                .oneshot(request)
                .await
                .unwrap()
                .status()
                .as_u16(),
            expected
        );
    }
}
#[tokio::test]
async fn replay_state_is_atomic_and_resettable_and_duplicate_json_auth_is_rejected() {
    let app = app(config());
    let path = "/api/store/v1/colors";
    let mut p = params();
    sign("GET", path, &mut p, "", "http://127.0.0.1:8000");
    let auth = header(&p);
    let (a, b) = tokio::join!(
        send(&app, "GET", path, Some(&auth), "", false),
        send(&app, "GET", path, Some(&auth), "", false)
    );
    let mut codes = [a.0, b.0];
    codes.sort();
    assert_eq!(codes, [200, 401]);
    send(&app, "POST", "/__mock/reset", None, "{}", false).await;
    assert_eq!(send(&app, "GET", path, Some(&auth), "", false).await.0, 200);
    let duplicate = r#"{"oauth_nonce":"one","oauth_nonce":"two"}"#;
    assert_eq!(
        send(
            &app,
            "GET",
            &format!("{path}?Authorization={}", encode(duplicate)),
            None,
            "",
            false
        )
        .await
        .0,
        401
    );
    assert_eq!(
        send(
            &app,
            "GET",
            &format!("{path}?Authorization=%7B%7D"),
            Some(&auth),
            "",
            false
        )
        .await
        .0,
        401
    );
}

#[tokio::test]
async fn advancing_clock_expires_signatures_without_extending_nonce_acceptance() {
    let app = app(config());
    let path = "/api/store/v1/colors";
    let mut p = params();
    sign("GET", path, &mut p, "", "http://127.0.0.1:8000");
    assert_eq!(
        send(&app, "GET", path, Some(&header(&p)), "", false)
            .await
            .0,
        200
    );
    send(
        &app,
        "POST",
        "/__mock/clock/advance",
        None,
        r#"{"seconds":301}"#,
        false,
    )
    .await;
    p.insert("oauth_nonce".into(), "new-but-expired".into());
    sign("GET", path, &mut p, "", "http://127.0.0.1:8000");
    assert_eq!(
        send(&app, "GET", path, Some(&header(&p)), "", false)
            .await
            .0,
        401
    );
    p.insert("oauth_timestamp".into(), "1767225901".into());
    sign("GET", path, &mut p, "", "http://127.0.0.1:8000");
    assert_eq!(
        send(&app, "GET", path, Some(&header(&p)), "", false)
            .await
            .0,
        200
    );
}

#[tokio::test]
async fn strict_catalog_read_runs_over_real_http_with_socket_identity() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let origin = format!("http://{address}");
    let mut cfg = config();
    cfg["auth"]["base_url"] = json!(origin);
    cfg["auth"]["allowed_ips"] = json!(["127.0.0.1"]);
    let app = app(cfg);
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap()
    });
    let path = "/api/store/v1/items/PART/3001";
    let mut p = params();
    sign("GET", path, &mut p, "", &origin);
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    socket.write_all(format!("GET {path} HTTP/1.1\r\nHost: {address}\r\nAuthorization: {}\r\nConnection: close\r\n\r\n",header(&p)).as_bytes()).await.unwrap();
    let mut received = String::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        socket.read_to_string(&mut received),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(received.starts_with("HTTP/1.1 200"), "{received}");
    let (_, body) = received.split_once("\r\n\r\n").unwrap();
    let data: Value = serde_json::from_str(body).unwrap();
    assert_eq!(data["data"]["no"], "3001");
    assert_eq!(data["data"]["test_extension"]["retained"], true);
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn authentication_precedes_fault_counting_and_catalog_dispatch() {
    let path = "/api/store/v1/colors/5";
    let mut settings = config();
    settings["faults"] = json!([{
        "id": "catalog-limit", "method": "GET", "path": path,
        "occurrences": [1], "phase": "before",
        "effect": {"kind": "rate_limit", "retry_after_seconds": 7}
    }]);
    let app = app(settings);
    assert_eq!(send(&app, "GET", path, None, "", false).await.0, 401);
    for (nonce, expected) in [("first", 429), ("second", 200)] {
        let mut credentials = params();
        credentials.insert("oauth_nonce".into(), nonce.into());
        sign("GET", path, &mut credentials, "", "http://127.0.0.1:8000");
        let response = send(&app, "GET", path, Some(&header(&credentials)), "", false).await;
        assert_eq!(response.0, expected);
        if expected == 200 {
            assert_eq!(response.1["data"]["color_id"], 5);
        }
    }
    let diagnostics = send(&app, "GET", "/__mock/validation", None, "", false)
        .await
        .1;
    assert_eq!(diagnostics["data"]["counters"]["catalog-limit"], 2);
    assert_eq!(diagnostics["data"]["events"].as_array().unwrap().len(), 1);
    assert_eq!(diagnostics["data"]["events"][0]["occurrence"], 1);
}
