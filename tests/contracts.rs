//! Offline, source-labeled wire contracts. Expected responses are authored
//! synthetic fixtures, not snapshots captured from the implementation or a store.
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use bricklink_sandbox::{app_configured, app_from_fixture};
use serde_json::Value;
use std::collections::BTreeSet;
use tower::ServiceExt;

#[tokio::test]
async fn source_backed_contract_cases() {
    let suite: Value = serde_json::from_str(include_str!("contracts/cases.json")).unwrap();
    let manifest: Value =
        serde_json::from_str(include_str!("../docs/contract-sources.json")).unwrap();
    assert_eq!(suite["version"], 1);
    assert_eq!(suite["seed"], "fixtures/small.json");
    let mut ids = BTreeSet::new();
    let mut used_sources = BTreeSet::new();
    assert!(!suite["cases"].as_array().unwrap().is_empty());
    for case in suite["cases"].as_array().unwrap() {
        let name = case["id"].as_str().unwrap();
        assert!(ids.insert(name), "duplicate contract ID {name}");
        assert!(
            [
                "documented-shape-with-synthetic-values",
                "documentation-interpretation",
                "simulator-policy"
            ]
            .contains(&case["basis"].as_str().unwrap()),
            "{name}: missing evidence classification"
        );
        assert!(
            !case["note"].as_str().unwrap().is_empty(),
            "{name}: missing scope note"
        );
        assert!(
            !case["sources"].as_array().unwrap().is_empty(),
            "{name}: missing provenance"
        );
        for source in case["sources"].as_array().unwrap() {
            let key = source.as_str().unwrap();
            used_sources.insert(key.to_owned());
            assert!(
                manifest["sources"].get(key).is_some(),
                "{name}: unknown source {key}"
            );
        }
        // Every case starts with fresh state; steps within a case share that state.
        let app = if case["catalog"] == true {
            let fixture = r#"{"version":1,"clock":{"mode":"fixed","now":"2026-01-01T00:00:00Z"},"inventories":[],"orders":[]}"#;
            app_configured(
                Some(fixture),
                Some(include_str!("../fixtures/catalog.json")),
                if case["validation"] == true {
                    Some(include_str!("../fixtures/validation.json"))
                } else {
                    None
                },
            )
            .unwrap()
        } else {
            app_from_fixture(include_str!("../fixtures/small.json")).unwrap()
        };
        assert!(
            !case["steps"].as_array().unwrap().is_empty(),
            "{name}: empty contract"
        );
        for (index, step) in case["steps"].as_array().unwrap().iter().enumerate() {
            let request = &step["request"];
            let expected = &step["expect"];
            let mut builder = Request::builder()
                .method(request["method"].as_str().unwrap())
                .uri(request["path"].as_str().unwrap());
            for (key, value) in request["headers"].as_object().unwrap() {
                builder = builder.header(key, value.as_str().unwrap());
            }
            let response = app
                .clone()
                .oneshot(
                    builder
                        .body(Body::from(request["body"].as_str().unwrap().to_owned()))
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status().as_u16();
            assert_eq!(
                status as u64,
                expected["status"].as_u64().unwrap(),
                "{name} step {index}: status"
            );
            if status != 204 {
                assert_eq!(
                    response.headers().get("content-type").unwrap(),
                    "application/json",
                    "{name} step {index}"
                );
            }
            let body = to_bytes(response.into_body(), 4 * 1024 * 1024)
                .await
                .unwrap();
            if status == 204 {
                assert!(
                    body.is_empty(),
                    "{name}: DELETE must have no response body under the selected policy"
                );
                continue;
            }
            let body: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body.as_object().unwrap().len(), 2, "{name}: envelope keys");
            assert_eq!(body["meta"]["code"], status, "{name}: integer result code");
            assert_eq!(
                body["meta"]["message"], expected["meta_message"],
                "{name}: result message"
            );
            assert!(
                body["meta"]["description"].is_string(),
                "{name}: description type"
            );
            assert_eq!(
                body["data"], expected["data"],
                "{name} step {index}: full synthetic response data"
            );
        }
    }
    assert_eq!(
        used_sources,
        manifest["sources"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect(),
        "every audited source must have a contract case"
    );
}
