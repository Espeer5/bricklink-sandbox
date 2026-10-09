# Catalog, fault and OAuth validation

This implements sandbox issues #13, #4 and #7 using synthetic data and documented API shapes. All features work locally without a BrickLink account. It is an unofficial simulator: passing these tests proves client behavior against the declared contract, not live provider compatibility, account access or data-use rights.

## Catalog fixtures

Use `--catalog fixtures/catalog.json`, embed the same object under `catalog` in a version-1 store fixture, or use `app_configured(fixture_json, catalog_json, validation_json)` from Rust. Each argument is `Option<&str>`. Sidecar catalog data replaces any embedded catalog. Existing `app()` and `app_from_fixture()` remain supported.

The catalog object has `validate_inventory` (default false), `items`, `colors`, and `categories`. Every item entry has:

- `item`: full catalog resource, including `no`, `type`, `name`, `category_id`; optional description, alternate number, obsolete flag, language, measurements, release year, images and unknown extensions are preserved.
- `known_colors`: array of `{color_id, quantity}`. These catalog quantities never become merchant stock.
- `images`: optional map from color ID strings to image resources (`no`, `type`, `thumbnail_url` and any additional fields). URLs are references; the sandbox fetches no image data.
- `subsets`, `supersets`: optional authored default response arrays, including matching groups, item identity, quantities and alternate/counterpart flags. Absent means unsupported/unavailable (404); an explicit empty array means known empty (200).
- `element_mappings`: optional PART-only mapping array with item identity, color ID/name and element ID. Multiple mappings are retained, including ambiguous reverse lookups.

The bundled fixture is wholly authored synthetic data licensed under this repository's MIT license. IDs resemble real catalog identifiers for client integration, but labels, memberships, quantities, measurements, years and URLs are not verified BrickLink records. No external catalog content or personal data was imported. Custom external fixtures require their own provenance and permitted-use assessment.

| GET path, relative to `/api/store/v1` | Behavior |
| --- | --- |
| `/items/{type}/{no}` | Exact ID lookup, case-insensitive type; full authored resource. No alias rewrite or variant substitution. |
| `/items/{type}/{no}/colors` | Full known-color list. |
| `/items/{type}/{no}/images/{color_id}` | Exact authored item/color image reference. |
| `/items/{type}/{no}/subsets`, `/supersets` | Default authored relationship groups only, no recursion or inferred themes. Unsupported query options return 400. |
| `/colors`, `/colors/{id}` | Full color list or exact detail. |
| `/categories`, `/categories/{id}` | Full category list or exact detail; parent ID 0 denotes root. |
| `/item_mapping/PART/{no}` | All authored mappings, optionally filtered by integer `color_id`. |
| `/item_mapping/{element_id}` | All authored reverse matches, including multiple candidates. |

Unknown items, colors, categories, images and reverse mappings return 404. Unsupported methods on implemented catalog routes return 405. `price` is not implemented; optional exploded/filtered subset/superset representations remain unsupported. The compatibility matrix does not imply website/API parity or complete catalog coverage.

Startup rejects duplicate catalog identities, missing category parents, category cycles, unknown/duplicate known colors, mismatched images/mappings and malformed relationship shapes. Relationship inventories are authored separately; they are not calculated from other catalog entries. Optional metadata fields and extensions remain as supplied rather than being synthesized.

When `validate_inventory=true`, startup inventories and newly created lots must match an exact catalog item not marked obsolete and a known color. Supplied nonzero category IDs must agree; omitted category remains unknown/default 0. Replay inventory creation uses the same check. This is a **local strict policy**, not a claim about BrickLink's exact validation. Names, quantity, condition, prices and remarks remain merchant fields. Default/permissive mode accepts identifiers without membership checks. Existing inventory price normalization remains unchanged.

## Fault configuration

Pass `--validation fixtures/faults.json` or POST a rule array to `/__mock/faults`:

```json
[
  {
    "id": "lost-update",
    "method": "PUT",
    "path": "/api/store/v1/inventories/1000",
    "occurrences": [1],
    "phase": "after",
    "effect": {"kind": "disconnect"}
  }
]
```

Rules match an exact marketplace path and uppercase method; query parameters are not part of matching. One-based occurrence counters advance for matching requests after authentication, method and body decoding. Failed authentication cannot trigger a fault or mutate stock. At most one rule fires per request: the first matching occurrence in configured order; all matching rules' counters still advance. Concurrent admission is serialized; arrival order is not prescribed, so use sequential requests for reproducible scripts. A selected rule keeps its occurrence number even if another request arrives during a delay.

| Effect | Result |
| --- | --- |
| `{"kind":"error","status":503}` | Configured 400–599 HTTP error with JSON envelope. Recognized 401/403/404/500 use documented names; others use a simulator label. |
| `{"kind":"rate_limit","retry_after_seconds":7}` | HTTP 429, simulator-labeled envelope and Retry-After seconds. Generic resilience scenario, not a verified BrickLink quota/header policy. |
| `{"kind":"delay","milliseconds":200}` | Wait before or after dispatch, then continue normally. Set a shorter client deadline to exercise timeout handling. Maximum 60 seconds; no store lock is held during the delay. |
| `{"kind":"disconnect"}` | Fail the response body transport. In a real HTTP/1 connection this yields a lost/truncated response or connection closure; exact wire manifestation is server/client dependent. No fabricated success/error JSON. |
| `{"kind":"malformed"}` | HTTP 200 application/json with malformed content, to exercise response validation. |

A terminal `before` fault prevents dispatch and leaves inventory unchanged. A `before` delay continues to dispatch if the request remains alive; cancellation before dispatch leaves state unchanged. An `after` fault is applied only when normal dispatch succeeds, so validation errors are preserved. For writes, changes already committed remain committed even if the response is lost or a client times out. The sandbox does **not** deduplicate a retried quantity delta: stock 100 → 97 with a lost response → 94 after retry. Clients must reconcile ambiguous outcomes instead of assuming retry safety. Reads also support after-response faults.

`GET /__mock/validation` exposes mode, configured rules, per-rule counters and the latest 1,000 fault events. Events carry rule ID, method/path, selected occurrence, phase/effect and whether dispatch had succeeded at injection time. A before-delay event has not dispatched yet; it is not a claim that the eventual request failed. Credentials, signatures and nonce values are excluded. Fault configuration is atomic: invalid replacements leave prior rules/counters intact. Empty rule arrays disable faults.

`POST /__mock/faults/reset` clears counters/events only. `POST /__mock/reset` restores store/clock and clears counters/events and nonce history; rules/credentials remain configured. `POST /__mock/replay` also resets validation histories but its internal actions do not pass through HTTP faults. In-flight requests already admitted before a reset keep their selected behavior; quiesce traffic before deterministic reset/replay. Fault configuration is limited to 100 rules and 1,000 positive occurrence numbers per rule. Rules and diagnostics are local test controls, not upstream API features.

## Strict OAuth mode

`--validation fixtures/validation.json` enables strict authentication for marketplace paths. Omitting `auth` keeps the prior permissive behavior. The sample uses public dummy credentials:

```json
{
  "auth": {
    "base_url": "http://127.0.0.1:8000",
    "consumer_key": "dummy-consumer",
    "consumer_secret": "dummy-consumer-secret",
    "token": "dummy-token",
    "token_secret": "dummy-token-secret",
    "timestamp_window_seconds": 300,
    "allowed_ips": ["127.0.0.1"]
  },
  "faults": []
}
```

Only dummy values belong in these files. Credential values are not exposed by diagnostics or validation errors. Strict mode is for integration tests, not production access control. Mock controls and health remain unauthenticated; bind to loopback and do not publish this server.

Sign the configured `base_url` plus request path using OAuth 1.0 HMAC-SHA1. Set the origin to the actual local address and port used by the client; switching from `localhost` to `127.0.0.1`, changing the port, or signing BrickLink's production URL will fail unless that exact origin was deliberately configured. The server uses this configured origin, not Host or forwarded headers, to construct the signature base. It normalizes URL scheme/host/default port through URL parsing; path escaping and duplicate query values remain significant.

Supported credential placement follows the current BrickLink manual:

1. `Authorization: OAuth oauth_consumer_key="...", ...` with RFC3986-escaped names/values.
2. A single `Authorization` query parameter containing URL-encoded JSON whose OAuth string values are themselves RFC3986-escaped, as in the manual's example.

Supplying both locations, duplicate fields, malformed escapes, wrong/missing credentials, bad signatures, an unsupported algorithm/version, invalid timestamps or reused nonces yields 401 `BAD_OAUTH_REQUEST`. Ordinary query parameters—including duplicates—are decoded, RFC3986-encoded, sorted by encoded key/value and signed. The Authorization wrapper and signature itself are excluded. JSON and raw URL-encoded JSON bodies are not form parameter lists and are not included in the OAuth signature. The local `data=` form extension uses normal form-parameter signing. This distinction and the manual's query-wrapper interpretation are tested but still require separate live-read validation before asserting production parity.

Timestamps use the store clock (system by default, fixture time when fixed). The configurable symmetric acceptance window is a **simulator policy**; BrickLink does not document an exact window here. Nonces are remembered until their timestamp plus window expires. At most 10,000 active nonces are admitted; excess requests fail rather than evict replay protection. Advance a fixed clock explicitly for expiry tests. Atomic validation prevents two concurrent copies of one signed request from both succeeding. Reset clears local replay history; no tokens are issued or refreshed.

`allowed_ips` is optional (empty means no IP restriction). When set, compare the real socket peer, never X-Forwarded-For. The CLI supplies socket connection information; a Rust in-process caller must attach `ConnectInfo<SocketAddr>` or gets 403. This models a useful exact-IP restriction, not BrickLink's registration workflow or per-token production policy. Wrong credentials and fault rules can produce deterministic 401/403 cases. No TLS server, OAuth account registration or live secrets are required.

## Verification and remaining limits

Run:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

The suite includes independent static OAuth signature vectors, header/query forms, encoding/order/duplicates, local origin mismatch, wrong credentials, timestamps, concurrent replay rejection, peer-IP handling and safe diagnostics. Catalog tests compare full authored payloads and strict/permissive stock membership. Fault tests cover reset, occurrence counting, rate-limit headers, malformed content, cancellation before/after dispatch, failed validation, concurrency, and an actual local TCP connection losing the response after an inventory update. The existing store/order/fixture tests still run.

[Source manifest](contract-sources.json) and [source-labeled cases](../tests/contracts/cases.json) identify official documentation, interpretations and simulator policies. Expected fixtures are authored independently of response-building code. Remaining live unknowns include actual provider optional/null fields, response sizes, alias behavior, auth windows, quota resets and failure-wire details. The simulator cannot establish account eligibility, licensing, permitted retention, or real catalog search accuracy. It makes those live checks smaller; it does not replace them.
