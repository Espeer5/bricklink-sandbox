# Order data and lifecycle simulation

Start with `cargo run --locked -- --fixture fixtures/lifecycle.json`. This fixture has an incoming EUR shipment, a filed outgoing GBP order, and a deterministic script covering purchase, edits, payment, tracking, partial refund, cancellation, and refund of the balance. Run `POST /__mock/replay` to execute it. All names, addresses, tracking IDs, and payments are fictional.

## Configurable order details

Historical fixture orders and `POST /__mock/orders` accept an optional `details` object. Script `create_order` actions accept the same object. Historical legacy fields remain supported; a value in `details` takes precedence.

```json
{
  "items": [{"inventory_id":1000,"quantity":10}],
  "details": {
    "direction":"in",
    "buyer_name":"synthetic_buyer",
    "buyer_email":"buyer@example.test",
    "currency_code":"EUR",
    "shipping": {
      "method":"Synthetic parcel",
      "address": {
        "name":{"full":"Example Buyer","first":"Example","last":"Buyer"},
        "address1":"1 Fictional Street",
        "city":"Example City",
        "postal_code":"00000",
        "country_code":"DE"
      }
    },
    "payment":{"method":"Synthetic transfer","status":"None"},
    "cost":{"shipping":"2.50","insurance":"0.20","etc1":"0.30","credit":"0.10"}
  }
}
```

Optional details:

| Field | Accepted values / default |
| --- | --- |
| `buyer_name`, `buyer_email`, `seller_name`, `store_name` | Nonempty strings; existing mock defaults |
| `remarks` | String; empty by default |
| `direction` | `in` (default, seller view) or `out` (buyer view) |
| `is_filed` | Boolean, default false |
| `status` | `PENDING` by default; supported labels below |
| `date_ordered`, `date_status_changed` | RFC3339, normalized to UTC milliseconds; sandbox clock by default. Custom order date also initializes status date unless supplied separately |
| `currency_code` | Three uppercase ASCII letters, default USD; syntactic validation only |
| `payment` | `method`, `status`, optional `date_paid` timestamp; currency comes from `currency_code` |
| `shipping` | `method`, `method_id` (string or unsigned integer), `tracking_no`, `tracking_link`, `date_shipped`, `address`; absent fields remain absent |
| `shipping.address` | String `full`, `address1`, `address2`, `city`, `state`, `postal_code`, `country_code`, `phone_number`; `name` object with string `full`, `first`, `last` |
| `cost` | Decimal-string `shipping`, `insurance`, `etc1`, `etc2`, `credit`, `coupon` |
| `inventory_effects` | Historical orders: `none` (default) or `reserved`; outgoing orders must use `none`. New incoming purchases always reserve stock |

Unknown details fields, malformed dates, negative charges, and totals below zero fail atomically. Initial statuses and payment fields are independently authored: seeding `PAID` does not infer a payment timestamp or a real transfer. This permits intentionally unusual read scenarios. Route-driven changes follow the policies below.

## Documented update endpoints

These routes use `/api/store/v1` and accept the same JSON/URL-encoded JSON request formats as other marketplace routes.

| Method and path | Body | Success data |
| --- | --- | --- |
| PUT `/orders/{id}` | Any subset of `remarks`, `is_filed`, `shipping.{date_shipped,tracking_no,tracking_link,method_id}`, `cost.{shipping,insurance,etc1,etc2,credit}` | Updated order |
| PUT `/orders/{id}/status` | `{"field":"status","value":"SHIPPED"}` | `null` |
| PUT `/orders/{id}/payment_status` | `{"field":"payment_status","value":"Received"}` | `null` |

The [update manual](https://www.bricklink.com/v3/api.page?page=update-order) says other fields are ignored and costs only change while unpaid. The two patch manuals specify empty data; this implementation chooses a 200 envelope with `data:null`. Exact empty-body encoding remains unverified. The [source manifest](contract-sources.json) records the audit and synthetic contract cases.

The [status help](https://www.bricklink.com/help.asp?helpID=41) identifies ownership and a one-week completed-order change window, but does not give a complete API transition graph. We apply those UI restrictions as a documented simulator interpretation:

- Incoming status writes accept `PROCESSING`, `READY`, `PAID`, `PACKED`, `SHIPPED`, `COMPLETED`; outgoing writes accept `RECEIVED`, `COMPLETED`.
- Fixtures additionally accept `PENDING`, `UPDATED`, `OCR`, `NPB`, `NPX`, `NRS`, `NSS`, `CANCELLED`. System/dispute workflows are not recreated. Mock edits and cancellation produce `UPDATED` and `CANCELLED` respectively.
- An unchanged status is a no-op. Cancelled orders cannot reopen. Completed orders can change through exactly seven elapsed days from `date_status_changed`; later changes fail. There is no invented mandatory linear progression between allowed statuses.

Payment labels are `None`, `Sent`, `Received`, `Paid`, `Clearing`, `Returned`, `Bounced`, `Completed`. Outgoing payment writes only permit `Sent` while the current value is `None`/`Sent`; incoming writes exclude `Sent`. See [payment help](https://www.bricklink.com/help.asp?helpID=121). Account-specific payment/order separation and PayPal pending indicators are not simulated.

Local payment policy: setting order status `PAID` sets payment `Paid` and initializes a missing payment timestamp; setting payment `Paid`/`Completed` initializes a missing payment timestamp. Other payment updates leave order status alone. No money moves. For cost locking and refunds, payment `Received`, `Clearing`, `Paid`, `Completed`, `Returned`, or order `PAID`, `PACKED`, `SHIPPED`, `RECEIVED`, `COMPLETED` counts as settled. This conservative predicate is a simulator choice, not a verified upstream unpaid test. Costs stay locked after any refund. Status/payment writes never deduct or restore stock. Invalid patch shapes return 400; disallowed state changes return 422; missing orders return 404.

## Mock-only controls and stock effects

These operations have **no production endpoint claim**. Every mutation is transactional under the store lock: all affected lots, totals, receipts, and order fields commit together or remain unchanged.

| Control | Body and behavior |
| --- | --- |
| POST `/__mock/orders` | Required nonempty `items` plus optional `details`; creates a purchase and reserves/deducts incoming stock once. Outgoing orders copy referenced lots without changing local stock |
| POST `/__mock/orders/{id}/items` | `{"items":[{"inventory_id":1000,"quantity":5}]}` replaces the entire item selection with absolute quantities; duplicates aggregate. Added/increased lines deduct the difference; removed/decreased lines restore it |
| POST `/__mock/orders/{id}/cancel` | `{"restock":true}` (default true); cancels and restores the remaining reservation once. False cancels without returning stock; a later true request returns the remainder. Does not refund money |
| POST `/__mock/orders/{id}/refund` | `{"operation_id":"refund-1","amount":"0.30","items":[{"inventory_id":1000,"quantity":2}]}`; positive amount required, items optional. Requires settlement; accumulated refunds cannot exceed original grand total. Full refund sets payment `Returned`; order status and historical totals remain unchanged |
| POST `/__mock/orders/{id}/restock` | `{"operation_id":"return-1","items":[{"inventory_id":1000,"quantity":2}]}`; required nonempty items, no amount; returns reserved items without a monetary refund |
| GET `/__mock/orders/{id}/state` | Direction, whether inventory is managed, remaining reservations, cumulative refunded amount, and currency. These simulator fields are kept out of marketplace responses |

Item changes require an unpaid `PENDING`, `UPDATED`, or `PROCESSING` order before any refund/restock. They set status `UPDATED` and payment `None`. An identical selection is a no-op; empty selections must use cancellation. Existing item snapshots retain price and metadata when quantity changes. New lines snapshot their lot at the time of addition. Removing and later re-adding a line constitutes a new snapshot. Edits use one batch; production batch history is not modeled. Unmanaged historical/outgoing edits do not change stock.

Refund/restock operation IDs are required, nonempty, and scoped to an order. The same action and JSON body replay the original response without effects, even after subsequent actions; the same ID with different input returns 422. JSON object key ordering is irrelevant, but equivalent numeric strings or reordered arrays are distinct input. New IDs cannot return more than the shared remaining reservation or refund more than the total. Cancellation and absolute edits are intrinsically idempotent and need no key. Purchase retries create new orders; purchase idempotency is not provided.

Historical inventory quantities are **opening available stock**, already net of historical sales. Historical `inventory_effects:"reserved"` initializes the returnable reservation without deducting that stock again. Use it only for orders whose units are excluded from opening stock. Default `none` avoids accidental inflation when historical inventory provenance is unknown. Restocking such unmanaged orders is rejected. Cancellation of unmanaged orders is allowed without stock effects.

Deleting a reserved lot makes subsequent stock restitution fail atomically with 404; the simulator does not recreate deleted lots. Stock cannot exceed the one-billion-unit cap. Restocking while an order is active is an explicit simulator action and freezes later edits; it does not infer delivery or cancellation. Filed state affects list filtering only. Reset/replay restore reservation and idempotency state along with orders and inventory.

## Amounts, snapshots, and limits

`subtotal = sum(snapshot unit_price_final × quantity)`.

`grand_total = subtotal + shipping + insurance + etc1 + etc2 − credit − coupon`.

All inputs use nonnegative decimal strings, rounded upward to four places, capped individually at one billion. Checked decimal arithmetic computes totals; negative totals fail. `etc1` may carry an explicitly supplied tax amount and `etc2` handling. No tax jurisdiction/rate engine, tax recomputation, carrier rates, tier pricing, or automatic discounts are implemented. Unlike BrickLink's documented behavior for collected taxes, updating a charge does not recalculate tax. Supply the desired tax amount explicitly.

Currencies are labels on nominal values: EUR/GBP/USD orders can coexist, but no FX conversion, ISO registry validation, currency-specific minor-unit rounding, or display conversion occurs. A lot's numeric price is interpreted in the order currency. Display and ordinary item prices are equal. Historical item prices originate from fixture lot prices; arbitrary separate historical unit-price overrides are not supported.

Later inventory edits/deletion never rewrite existing order items. List, detail, and nested item endpoints derive counts and totals from the same order state; cancellation/refunds preserve the purchased item record and original total. The mock state endpoint separately reports refunded value. These are deterministic integration-test scenarios, not a complete accounting ledger, payment processor, or production-equivalence guarantee.

## Scripted lifecycle actions

```json
{"action":"order_action","order_id":10000,"operation":"status","body":{"field":"status","value":"SHIPPED"}}
```

Supported operations: `update`, `status`, `payment_status`, `items`, `cancel`, `refund`, `restock`. Bodies and policies match their HTTP operations. Invalid actions reject fixture startup during dry-run validation. Scripts execute on disposable state and commit only when complete. `fixtures/lifecycle.json` finishes with original available stock restored, order 10002 filed/cancelled, refund total USD 2.5100, and empty remaining reservations. Its duplicate refund/cancellation steps demonstrate retry safety.
