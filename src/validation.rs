//! Resettable, deterministic faults. These are simulator controls, not upstream guarantees.
use super::*;
use axum::body::{Body, Bytes};
use std::collections::BTreeSet;

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Effect {
    Error { status: u16 },
    RateLimit { retry_after_seconds: u32 },
    Delay { milliseconds: u64 },
    Disconnect,
    Malformed,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Rule {
    #[serde(skip)]
    occurrence: u64,
    pub id: String,
    pub method: String,
    pub path: String,
    pub occurrences: Vec<u64>,
    pub phase: String,
    pub effect: Effect,
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    auth: Option<auth::Config>,
    #[serde(default)]
    faults: Vec<Rule>,
}
#[derive(Default)]
pub(super) struct Validation {
    auth: Option<auth::Config>,
    nonces: BTreeMap<String, i64>,
    rules: Vec<Rule>,
    counters: BTreeMap<String, u64>,
    events: Vec<Value>,
}
impl Validation {
    pub(super) fn parse(text: &str) -> Result<Self, ApiError> {
        let config: Config = serde_json::from_str(text).map_err(|_| {
            invalid("Invalid validation configuration; credential values are not echoed")
        })?;
        if let Some(auth) = &config.auth {
            auth.validate()?;
        }
        Self::validate_rules(&config.faults)?;
        Ok(Self {
            auth: config.auth,
            rules: config.faults,
            ..Self::default()
        })
    }
    fn validate_rules(rules: &[Rule]) -> Result<(), ApiError> {
        if rules.len() > 100 {
            return Err(invalid("At most 100 fault rules"));
        }
        let mut ids = BTreeSet::new();
        for rule in rules {
            if rule.id.is_empty()
                || rule.id.len() > 100
                || !ids.insert(&rule.id)
                || !["GET", "POST", "PUT", "DELETE"].contains(&rule.method.as_str())
                || !rule.path.starts_with(&format!("{BASE}/"))
                || rule.path.contains(['?', '#'])
                || !["before", "after"].contains(&rule.phase.as_str())
                || rule.occurrences.is_empty()
                || rule.occurrences.len() > 1000
                || rule.occurrences.contains(&0)
            {
                return Err(invalid(
                    "Invalid fault ID, method, exact API path, occurrence list or phase",
                ));
            }
            match rule.effect {
                Effect::Error { status } if !(400..=599).contains(&status) => {
                    return Err(invalid("Fault HTTP status must be 400..599"));
                }
                Effect::Delay { milliseconds } if milliseconds > 60000 => {
                    return Err(invalid("Fault delay must be <= 60000 ms"));
                }
                _ => {}
            }
        }
        Ok(())
    }
    pub(super) fn authenticate(
        &mut self,
        parts: &axum::http::request::Parts,
        bytes: &[u8],
        now: i64,
    ) -> Result<(), ApiError> {
        if let Some(auth) = &self.auth {
            auth.check(parts, bytes, now, &mut self.nonces)?;
        }
        Ok(())
    }
    pub(super) fn select(&mut self, method: &str, path: &str) -> Option<Rule> {
        let mut selected = None;
        for rule in &self.rules {
            if rule.method == method && rule.path == path {
                let count = self.counters.entry(rule.id.clone()).or_default();
                *count = count.saturating_add(1);
                if selected.is_none() && rule.occurrences.contains(count) {
                    let mut chosen = rule.clone();
                    chosen.occurrence = *count;
                    selected = Some(chosen);
                }
            }
        }
        selected
    }
    pub(super) fn event(&mut self, rule: &Rule, committed: bool) {
        if self.events.len() == 1000 {
            self.events.remove(0);
        }
        self.events.push(json!({"rule_id":rule.id,"method":rule.method,"path":rule.path,"phase":rule.phase,"occurrence":rule.occurrence,"effect":rule.effect,"dispatch_succeeded":committed}));
    }
    pub(super) fn reset(&mut self) {
        self.counters.clear();
        self.events.clear();
        self.nonces.clear();
    }
    pub(super) fn dispatch(&mut self, method: &str, path: &str, body: Value) -> Option<ApiResult> {
        match (method, path) {
            ("GET", "/__mock/validation") => Some(Ok((
                StatusCode::OK,
                json!({"authentication":if self.auth.is_some(){"strict"}else{"permissive"},"rules":self.rules,"counters":self.counters,"events":self.events,"nonce_count":self.nonces.len()}),
            ))),
            ("POST", "/__mock/faults") => Some((|| {
                let rules: Vec<Rule> = serde_json::from_value(body)
                    .map_err(|_| invalid("Expected fault rule array"))?;
                Self::validate_rules(&rules)?;
                self.rules = rules;
                self.counters.clear();
                self.events.clear();
                Ok((StatusCode::OK, json!({"configured":self.rules.len()})))
            })()),
            ("POST", "/__mock/faults/reset") => {
                self.counters.clear();
                self.events.clear();
                Some(Ok((StatusCode::OK, json!({"reset":true}))))
            }
            _ => None,
        }
    }
}
pub(super) async fn effect(rule: &Rule) -> Option<Response> {
    let response = match rule.effect {
        Effect::Delay { milliseconds } => {
            tokio::time::sleep(std::time::Duration::from_millis(milliseconds)).await;
            return None;
        }
        Effect::Error { status } => ApiError(
            StatusCode::from_u16(status).expect("validated status"),
            match status {
                401 => "BAD_OAUTH_REQUEST",
                403 => "PERMISSION_DENIED",
                404 => "RESOURCE_NOT_FOUND",
                500 => "INTERNAL_SERVER_ERROR",
                _ => "SIMULATED_FAILURE",
            },
            "Configured simulator fault".into(),
        )
        .into_response(),
        Effect::RateLimit {
            retry_after_seconds,
        } => {
            let mut response = ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "SIMULATED_RATE_LIMIT",
                "Generic rate-limit scenario; not a verified BrickLink quota policy".into(),
            )
            .into_response();
            response.headers_mut().insert(
                "retry-after",
                retry_after_seconds
                    .to_string()
                    .parse()
                    .expect("integer header"),
            );
            response
        }
        Effect::Malformed => (
            StatusCode::OK,
            [("content-type", "application/json")],
            "{invalid",
        )
            .into_response(),
        Effect::Disconnect => {
            let stream = futures_util::stream::once(async {
                Err::<Bytes, _>(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "synthetic lost response",
                ))
            });
            Response::new(Body::from_stream(stream))
        }
    };
    Some(response)
}
