# bricklink-sandbox

An **unofficial local simulator** for a subset of the BrickLink Store API, written in Rust.
Create fictional inventory and orders to test inventory import, order ingestion, and cross-marketplace synchronization without touching a live store.

Early version: this is a stateful testing tool, not a complete or certified BrickLink API implementation. It makes no outbound requests to BrickLink. All included store and customer data is synthetic. Not affiliated with or endorsed by BrickLink or the LEGO Group.

## Run

Install the current stable Rust toolchain, then:

```sh
git clone https://github.com/Espeer5/bricklink-sandbox.git
cd bricklink-sandbox
cargo run --locked
```

The server listens on `127.0.0.1:8000`. Configure your application's BrickLink base URL as:

```text
http://127.0.0.1:8000/api/store/v1
```

By default authentication is permissive and OAuth headers are ignored. Optional strict validation checks dummy OAuth credentials, signatures, timestamps, nonces and configured peer IPs. Use only dummy credentials locally. The service is intended for local development and CI, not public hosting. To change the bind address, use `cargo run --locked -- --bind 127.0.0.1:9000`.

By default, each startup seeds lot `1000` with 100 new red 2×4 bricks at USD 0.1500 each, in location `BIN-A01`. State lives in memory and is discarded when the server stops. IDs reset with the store; the default clock is the system clock.

For configurable stores and deterministic tests, run `cargo run --locked -- --fixture fixtures/small.json`. Versioned fixtures load inventories and historical orders, support a fixed clock, and optionally define replayable scripts. A 3,000-lot/200-order example is included. See the [fixture format and scenario guide](fixtures/README.md).

## Validate a catalog client

```sh
# Synthetic catalog with strict inventory membership; permissive authentication:
cargo run --locked -- --catalog fixtures/catalog.json
# Add OAuth signature validation using the documented dummy credentials:
cargo run --locked -- --catalog fixtures/catalog.json --validation fixtures/validation.json
# Reproduce an update that commits but loses its response:
cargo run --locked -- --catalog fixtures/catalog.json --validation fixtures/faults.json
```

The sample OAuth configuration signs `http://127.0.0.1:8000`; update `auth.base_url` if changing the bind address or port. Combine `auth` and `faults` in one validation file to test both. No live BrickLink account, listings, credentials or outbound requests are needed. [Configuration, supported catalog endpoints, failure scenarios and verification limits](docs/validation.md).

## Try an order

Read inventory:

```sh
curl http://127.0.0.1:8000/api/store/v1/inventories
```

Simulate a customer purchasing three pieces:

```sh
curl -X POST http://127.0.0.1:8000/__mock/orders \
  -H 'Content-Type: application/json' \
  -d '{"items":[{"inventory_id":1000,"quantity":3}]}'
```

The response contains order `10000`; lot `1000` now has 97 pieces. Fetch the order and its nested item batches using the API paths your integration would use:

```sh
curl http://127.0.0.1:8000/api/store/v1/orders
curl http://127.0.0.1:8000/api/store/v1/orders/10000/items
```

Restore the selected initial fixture (the default seed has no orders):

```sh
curl -X POST http://127.0.0.1:8000/__mock/reset
```

`/__mock/*` endpoints belong to this simulator. **They are not BrickLink API endpoints.** In particular, this project does not imply that BrickLink offers a create-order API.

## Supported API subset

| Method | Path | Behavior |
| --- | --- | --- |
| GET | `/api/store/v1/inventories` | List lots; filter by `item_type`, `color_id`, `category_id`, `status` |
| POST | `/api/store/v1/inventories` | Create one lot |
| GET | `/api/store/v1/inventories/{id}` | Read a lot |
| PUT | `/api/store/v1/inventories/{id}` | Update quantity delta, price, description, or remarks |
| DELETE | `/api/store/v1/inventories/{id}` | Delete a lot; HTTP 204 with no body |
| GET | `/api/store/v1/orders` | List orders; filter by `direction`, `status`, `filed` |
| GET | `/api/store/v1/orders/{id}` | Read an order |
| PUT | `/api/store/v1/orders/{id}` | Update permitted charges, tracking, remarks, and filed state |
| PUT | `/api/store/v1/orders/{id}/status` | Update order status |
| PUT | `/api/store/v1/orders/{id}/payment_status` | Update payment status |
| GET | `/api/store/v1/orders/{id}/items` | Read order items as nested batches |
| GET | `/api/store/v1/items/{type}/{no}` | Exact fixture catalog item |
| GET | `/api/store/v1/items/{type}/{no}/colors` | Known colors |
| GET | `/api/store/v1/items/{type}/{no}/images/{color_id}` | Authored image reference |
| GET | `/api/store/v1/items/{type}/{no}/subsets` or `/supersets` | Authored default relationship groups |
| GET | `/api/store/v1/colors` or `/colors/{id}` | Color list/detail |
| GET | `/api/store/v1/categories` or `/categories/{id}` | Category list/detail and parent hierarchy |
| GET | `/api/store/v1/item_mapping/PART/{no}` or `/item_mapping/{element_id}` | Authored element mappings, including ambiguous reverse matches |
| GET | `/health` | Server health |
| GET | `/__mock/validation` | Redacted mode, fault rules/counters/events |
| POST | `/__mock/faults` | Replace fault rules atomically |
| POST | `/__mock/faults/reset` | Reset fault counters and diagnostics |
| POST | `/__mock/orders` | Create a configurable mock order; incoming purchases deduct stock atomically |
| POST | `/__mock/orders/{id}/{action}` | Edit items, cancel, refund, or restock; see lifecycle guide |
| GET | `/__mock/orders/{id}/state` | Inspect reservations and cumulative refunds |
| POST | `/__mock/reset` | Restore selected initial fixture and counters |
| GET | `/__mock/clock` | Inspect system or fixed clock |
| POST | `/__mock/clock/advance` | Advance a fixed clock by nonnegative integer seconds |
| POST | `/__mock/replay` | Restore a fixed-clock fixture and execute its script atomically |

Responses use a `meta`/`data` JSON envelope. Unknown routes return HTTP 404; unsupported methods on recognized marketplace routes return 405. Unsupported list filters and unknown inventory create/update fields are rejected. Order updates ignore non-writable fields as the current manual specifies. Request bodies support the documented raw URL-encoded JSON form. Plain JSON and a form-encoded `data` wrapper are also accepted as local extensions. Malformed JSON/encoding returns `INVALID_REQUEST_BODY`; unsupported media types return 415.

Inventory creation requires `item.no`, `item.type`, `color_id`, `quantity`, `unit_price` (a decimal string), and `new_or_used` (`N` or `U`). Optional fields: `item.name`, `item.category_id`, `description`, `remarks`, `is_stock_room`, and `stock_room_id` (`A`, `B`, or `C`). Catalog membership is permissive by default. An optional strict catalog fixture validates the exact item/color and category on creation, without overwriting supplied names or merchant facts.

Quantity updates use signed delta strings, not absolute values:

```sh
curl -X PUT http://127.0.0.1:8000/api/store/v1/inventories/1000 \
  -H 'Content-Type: application/json' -d '{"quantity":"+10"}'
```

Inventory filters support comma-separated inclusion values and `-` exclusions, case-insensitively. Implemented inventory statuses: `Y` (positive available quantity), `N` (zero available quantity), `S` (stockroom A), `B`, and `C`. BrickLink’s reserved-inventory listing status is not modeled; the mock lifecycle tracks returnable order units separately. Order listing defaults to `direction=in&filed=false` and returns summaries; fetch individual orders for detail. Both incoming and outgoing orders can be authored using `details.direction`.

Mock orders aggregate duplicate lot IDs before checking availability. Invalid or oversold orders leave all stock unchanged. A shared lock serializes stock mutations; concurrent orders cannot consume the same remaining quantity. Order items preserve their purchase-time prices and details. Zero-quantity lots remain in the store.

For rich buyers, addresses, shipping, payments, multi-currency amounts, and retry-safe lifecycle controls, see the [order lifecycle guide](docs/order-lifecycle.md). Try `cargo run --locked -- --fixture fixtures/lifecycle.json`, then `POST /__mock/replay`.

## Compatibility limits and roadmap

The initial scope prioritizes inventory and order ingestion. The following are **not implemented**:

- Live credential registration, token issuance, TLS serving, or verified production OAuth window/quota policy. Optional strict dummy OAuth and exact peer-IP validation are supported.
- Bulk inventory creation, consolidation, retain behavior, tier pricing, or the full inventory schema.
- Automatic shipping rates, jurisdictional tax calculations, FX conversion, account-specific payment settings, and complete production lifecycle parity. Explicit charges/tax amounts, currency labels, documented updates, and mock cancellation/refund/restocking are supported.
- Price guides, exploded/filtered subset representations, feedback and notifications/webhooks. Fixture-backed catalog reads and deterministic fault injection are supported; see [validation guide](docs/validation.md).
- Runtime state persistence/export or production-identical validation/status transitions. Authored startup fixtures and fixed-clock replay are supported; these are not runtime snapshots.

Prices normalize upward to four decimal places, interpreting the manual's rounding-up instruction as ceiling for nonnegative values. Precise live rounding edge cases remain unverified. Unit prices and inventory quantities are capped at one billion for bounded simulation. Request bodies are limited to 1 MiB.

The [compatibility matrix and audit](docs/compatibility.md) record current official sources, field types/defaults, verified shapes, deliberate deviations, and unresolved documentation conflicts. The source-labeled contract suite runs offline with synthetic data. Passing it does not prove full production equivalence.

The [BrickLink Store API entry point](https://www.bricklink.com/v2/api/welcome.page), [current manual](https://www.bricklink.com/v3/api.page), and [older static reference](https://static.bricklink.com/alpha/default/api_wiki.html) provide background. The audit uses the current manual's publicly served content bundle; the static reference is historical and is not the contract suite's verification source. Future endpoint work should verify current documentation and add contract examples with synthetic data.

## Development

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

The library exposes `bricklink_sandbox::app()` for an independent default Axum router and `app_from_fixture(json_text)` for a validated custom store in integration tests. CI runs formatting, linting, and tests. See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT. See [LICENSE](LICENSE).
