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

Authentication is not enforced; OAuth headers are ignored. Use dummy credentials locally. The service is intended for local development and CI, not public hosting. To change the bind address, use `cargo run --locked -- --bind 127.0.0.1:9000`.

By default, each startup seeds lot `1000` with 100 new red 2×4 bricks at USD 0.1500 each, in location `BIN-A01`. State lives in memory and is discarded when the server stops. IDs reset with the store; the default clock is the system clock.

For configurable stores and deterministic tests, run `cargo run --locked -- --fixture fixtures/small.json`. Versioned fixtures load inventories and historical orders, support a fixed clock, and optionally define replayable scripts. A 3,000-lot/200-order example is included. See the [fixture format and scenario guide](fixtures/README.md).

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
| GET | `/api/store/v1/orders/{id}/items` | Read order items as nested batches |
| GET | `/health` | Server health |
| POST | `/__mock/orders` | Create an incoming mock order and deduct stock atomically |
| POST | `/__mock/reset` | Restore selected initial fixture and counters |
| GET | `/__mock/clock` | Inspect system or fixed clock |
| POST | `/__mock/clock/advance` | Advance a fixed clock by nonnegative integer seconds |
| POST | `/__mock/replay` | Restore a fixed-clock fixture and execute its script atomically |

Responses use a `meta`/`data` JSON envelope. Unknown routes return HTTP 404; unsupported methods on recognized marketplace routes return 405. Unsupported list filters and unknown create/update fields are rejected rather than silently accepted. Request bodies support the documented raw URL-encoded JSON form. Plain JSON and a form-encoded `data` wrapper are also accepted as local extensions. Malformed JSON/encoding returns `INVALID_REQUEST_BODY`; unsupported media types return 415.

Inventory creation requires `item.no`, `item.type`, `color_id`, `quantity`, `unit_price` (a decimal string), and `new_or_used` (`N` or `U`). Optional fields: `item.name`, `item.category_id`, `description`, `remarks`, `is_stock_room`, and `stock_room_id` (`A`, `B`, or `C`). No catalog lookup is performed.

Quantity updates use signed delta strings, not absolute values:

```sh
curl -X PUT http://127.0.0.1:8000/api/store/v1/inventories/1000 \
  -H 'Content-Type: application/json' -d '{"quantity":"+10"}'
```

Inventory filters support comma-separated inclusion values and `-` exclusions, case-insensitively. Implemented inventory statuses: `Y` (positive available quantity), `N` (zero available quantity), `S` (stockroom A), `B`, and `C`. Reserved inventory is not modeled. Order listing defaults to `direction=in&filed=false` and returns summaries; fetch individual orders for detail. Outgoing orders are always empty.

Mock orders aggregate duplicate lot IDs before checking availability. Invalid or oversold orders leave all stock unchanged. A shared lock serializes stock mutations; concurrent orders cannot consume the same remaining quantity. Order items preserve their purchase-time prices and details. Zero-quantity lots remain in the store.

## Compatibility limits and roadmap

The initial scope prioritizes inventory and order ingestion. The following are **not implemented**:

- OAuth signature validation, IP restrictions, or credential failure simulation.
- Bulk inventory creation, consolidation, retain behavior, tier pricing, or the full inventory schema.
- Order status/payment updates, cancellations, refunds, shipping calculations, tax, and multiple currencies. New mock purchases are unfiled, incoming, USD, and `PENDING`, with empty shipping details. Fixtures may seed other supported status labels and filed orders, without simulating lifecycle transitions.
- Catalog endpoints, price guides, feedback, notifications/webhooks, or fault injection.
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
