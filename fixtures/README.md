# Fixtures and deterministic scenarios (format version 1)

Fixtures are authored initial conditions plus optional scripts. They are **not runtime snapshots**: loading a file builds a new store, calculates order item batches and totals, and initializes ID counters. No changes are written back to the file. State export/persistence is a separate feature tracked in [issue #8](https://github.com/Espeer5/bricklink-sandbox/issues/8).

## Start a custom store

```sh
cargo run --locked -- --fixture fixtures/small.json
# Or load the larger store:
cargo run --locked -- --fixture fixtures/large.json --bind 127.0.0.1:9000
```

The file is parsed and fully validated **before** the listener is bound. Errors include the source filename and a field path, for example `$.inventories[0].inventory.unit_price: Invalid decimal price`. Type errors, missing/unknown fields, duplicate IDs, invalid references, timestamps, and script failures are rejected. For missing fields, the error identifies the containing object and names the missing field. Fix the file and restart; there is no hot reload.

| Included file | Initial data | Script |
| --- | --- | --- |
| `small.json` | 6 lots, 4 historical orders; three colors, new/used pieces, storage locations, a depleted lot, all three stockrooms, pending/paid/shipped/completed orders, one filed order | Advance clock, purchase, restock/change location, create a lot, advance clock, purchase from new lot |
| `large.json` | 3,000 lots, 200 historical orders; five colors, new/used pieces, unique locations, depleted and stockroom lots, 1–4 lines per order, four statuses, 50 filed orders | Advance clock and make a two-lot purchase |

These are wholly synthetic stores; item labels and colors are not a validated catalog. Regenerate the large file reproducibly with:

```sh
cargo run --locked --example generate_large_fixture -- fixtures/large.json
```

## Root format

```json
{
  "version": 1,
  "clock": {"mode": "fixed", "now": "2026-01-01T12:00:00.000Z"},
  "inventories": [],
  "orders": [],
  "steps": []
}
```

`version`, `clock`, `inventories`, and `orders` are required. Arrays may be empty. `steps` defaults to an empty array. Unknown fields are rejected. Version 1 is the only accepted version.

Clock choices:

- `{"mode":"fixed","now":"<RFC3339 timestamp>"}`: time changes only when explicitly advanced. All new lot/order timestamps come from this clock.
- `{"mode":"system"}`: use wall time. Scripts must be empty; replay and clock advancement are unavailable. Fixtures still support reset, but future operation timestamps are not deterministic.

Timestamps accept UTC or an explicit offset and normalize to UTC with three fractional digits. Precision beyond milliseconds is rejected. Dates on historical records may precede the clock. Fixed-clock scenarios should use dates appropriate for their own timeline.

## Inventory entries

```json
{
  "inventory_id": 1000,
  "date_created": "2025-12-01T00:00:00Z",
  "inventory": {
    "item": {"no": "3001", "type": "PART", "name": "Brick 2 x 4", "category_id": 5},
    "color_id": 5,
    "quantity": 100,
    "unit_price": "0.1500",
    "new_or_used": "N",
    "remarks": "BIN-A01"
  }
}
```

`inventory_id` is required, positive, unique within inventory, and less than `u64::MAX`. `date_created` is optional and defaults to the fixture clock at load time. `inventory` uses the supported inventory-create payload:

| Field | Type / constraints | Required / default |
| --- | --- | --- |
| `item.no` | Nonempty string | Required |
| `item.type` | `PART`, `SET`, `MINIFIG`, `BOOK`, `GEAR`, `CATALOG`, `INSTRUCTION`, `UNSORTED_LOT`, `ORIGINAL_BOX` | Required |
| `item.name` | String | Empty string |
| `item.category_id` | Unsigned integer | `0` |
| `color_id` | Unsigned integer | Required |
| `quantity` | Integer, 0 through 1,000,000,000 | Required |
| `unit_price` | Decimal string, 0 through 1,000,000,000, rounded upward to four decimal places | Required |
| `new_or_used` | `N` or `U` | Required |
| `description`, `remarks` | Strings | Empty strings |
| `is_stock_room` | Boolean | `false` |
| `stock_room_id` | `A`, `B`, or `C` | `A` |

This is the existing simulator inventory subset, not the full BrickLink schema. Prices normalize upward to four decimal places under the documented simulator rounding interpretation. Lot quantities are **current opening stock**, not stock before historical orders.

## Historical order entries

```json
{
  "order_id": 10000,
  "items": [{"inventory_id": 1000, "quantity": 3}],
  "date_ordered": "2025-12-31T09:00:00Z",
  "status": "SHIPPED",
  "is_filed": false,
  "buyer_name": "synthetic_buyer"
}
```

Required: positive unique `order_id` below `u64::MAX`, and nonempty `items`. Every line requires an existing `inventory_id` and positive integer `quantity` no greater than one billion. Duplicate lot references are combined. All referenced inventory must be present in the fixture, regardless of array order.

Optional fields:

- `date_ordered`: defaults to the fixture clock; also initializes `date_status_changed`.
- `status`: `PENDING` (default), `PAID`, `SHIPPED`, or `COMPLETED`. These are fixture labels, not an implementation of status transitions.
- `is_filed`: boolean, defaults to `false`.
- `buyer_name`: nonempty string, defaults to `mock_buyer`.

Historical orders **do not deduct stock**. They may reference depleted or stockroom lots, and historical quantity may exceed opening stock. Order items copy the fixture lot's details and price; totals are calculated in USD. The rest of the order uses the simulator's existing synthetic defaults, including empty shipping and placeholder payment data. A status label does not generate payment/shipping transitions. Richer lifecycle behavior remains [issue #3](https://github.com/Espeer5/bricklink-sandbox/issues/3).

New inventory IDs start at `max(1000, highest fixture inventory ID + 1)`. New order IDs start at `max(10000, highest fixture order ID + 1)`. These rules do not depend on fixture array ordering. Exhausted ID space returns an error without committing a new record.

## Script actions

Scripts require a fixed clock and execute sequentially. Supported actions:

```json
[
  {"action":"advance_clock","seconds":60},
  {"action":"create_order","items":[{"inventory_id":1000,"quantity":3}]},
  {"action":"update_inventory","inventory_id":1000,"update":{"quantity":"+10","remarks":"BIN-B02"}},
  {"action":"create_inventory","inventory":{"item":{"no":"3002","type":"PART"},"color_id":1,"quantity":20,"unit_price":"0.0800","new_or_used":"U"}}
]
```

- `advance_clock`: nonnegative integer seconds; overflow is rejected. Zero is allowed.
- `create_order`: uses normal mock purchase rules and **deducts stock**. Empty/invalid lines or insufficient available stock reject the script.
- `update_inventory`: existing lot ID plus the supported update payload: signed-string quantity delta, decimal-string unit price, string description and/or remarks.
- `create_inventory`: the inventory payload described above; the server assigns its ID. Later steps may reference that ID using the documented counter rules.

Startup validates the complete script on disposable state. Startup itself does **not** commit script actions. Invalid scripts cannot start a server. Reset returns to the pre-script fixture.

## Clock controls, reset, and replay

```sh
curl http://127.0.0.1:8000/__mock/clock
curl -X POST http://127.0.0.1:8000/__mock/clock/advance \
  -H 'Content-Type: application/json' -d '{"seconds":60}'
curl -X POST http://127.0.0.1:8000/__mock/reset
curl -X POST http://127.0.0.1:8000/__mock/replay
```

`reset` restores the exact selected initial inventory, historical orders, ID counters, and initial fixed clock. It does not clear the store to the built-in seed and does not run scripts. In system mode, initial record timestamps are restored while subsequent operations use current wall time.

`replay` starts from that same initial fixture every time, runs all steps on temporary state, then atomically replaces the active store. It discards any intervening manual changes. Requests cannot observe a partially executed script. The response contains `data.steps`, each with a zero-based `step`, HTTP `code`, and operation `data`, plus the final `data.clock`. It returns only after all actions finish; this is not a background event scheduler.

For `small.json`, replay produces order IDs `10004` and `10005`, creates lot `1006`, and finishes at `2026-01-01T12:03:00.000Z`. Lots `1000`, `1001`, and `1006` finish with quantities `97`, `48`, and `16`.

To verify deterministic results:

```sh
curl -sS -X POST http://127.0.0.1:8000/__mock/replay > /tmp/replay-first.json
curl -sS -X POST http://127.0.0.1:8000/__mock/replay > /tmp/replay-second.json
cmp /tmp/replay-first.json /tmp/replay-second.json
```

A fixed clock also supports repeatable manually issued requests after reset. Concurrent requests are serialized but their arrival order is not deterministic; use script steps for prescribed ordering.

All these controls are simulator-only. They do not correspond to BrickLink endpoints. Fixed-clock controls do not implement order transitions, webhooks, failures, or persistence.

## Library use

`bricklink_sandbox::app_from_fixture(json_text)` returns `Result<axum::Router, FixtureError>`. Errors expose `path` and `message`. Every invocation creates independent initial and live state; cloning a returned router intentionally shares that one store. Existing `app()` keeps the built-in fixture and system clock.
