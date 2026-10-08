# BrickLink Store API compatibility audit

**Verification date: 2026-10-08 UTC. Scope: the currently implemented endpoint subset.**

This audit verifies what the **current public documentation says**, not what a live account does. No authenticated requests, production writes, copied customer data, or real credentials were used. The current manual is client-rendered, so its text was inspected in the public React bundle referenced by the live page. [Source manifest](contract-sources.json) records the source URLs, retrieval time, component identifiers, findings, and bundle SHA-256. The historical static manual is background only; it is not the source of the assertions below.

Status terminology:

- **Implemented:** the named behavior is represented and tested within the stated subset.
- **Partial:** a documented operation exists, but fields or behaviors are omitted or deliberately simplified.
- **Unsupported:** not implemented; another roadmap issue may own it.
- **Unverified:** documentation is absent, ambiguous, or inconsistent, or precise live behavior would require further evidence. A local policy test is not upstream verification.

The verification date above applies to every matrix and source in this document. Documentation can change independently of the simulator. Recheck it before expanding a compatibility claim.

## Endpoint matrix

Paths below are relative to `/api/store/v1`. Marketplace GET and DELETE methods are documented without request bodies. POST/PUT use URL-encoded JSON; plain JSON and a `data=` form wrapper are supported local extensions. Success payloads use `meta`/`data`, except the selected empty-body DELETE behavior. HTTP codes are drawn from the general result-code table; per-method pages do not always specify one unique code.

| Method / path | Request, fields, defaults, filters | Response / HTTP policy | Status and limits | Sources |
| --- | --- | --- | --- | --- |
| GET `/inventories` | No body. Optional `item_type`, `status`, `category_id`, `color_id`. Omitted filters include all. Comma-separated values and `-` exclusions. | 200; `data` array of inventory objects in the supported resource subset. | **Partial:** no reserved stock; zero quantity → N is local policy. The manual says summaries but does not enumerate a separate summary projection. | [List inventories][list-inventory], [Inventory schema][inventory] |
| GET `/inventories/{inventory_id}` | Integer path ID; no body. | 200; one inventory object; missing ID 404. | **Partial:** resource fields listed below only; no catalog enrichment. | [Get inventory][get-inventory], [Inventory schema][inventory], [Errors][errors] |
| POST `/inventories` | Inventory object. Both `/inventories` and `/inventories/` accepted because current table/example use different forms. Supported create fields below. | 201; created inventory object. | **Partial:** no bulk arrays, consolidation, retain, bulk/tier pricing, complete SET schema, or other omitted writable fields. | [Create inventory][create], [Inventory schema][inventory], [Errors][errors] |
| PUT `/inventories/{inventory_id}` | Integer ID; optional signed-string `quantity`, string `unit_price`, `description`, `remarks`. Quantity is a delta, not an absolute count. | 200; updated inventory object; missing ID 404. | **Partial:** other documented update fields unsupported. `color_id` is not writable in the current list; historical docs conflict. Null/unknown handling is a local policy. | [Update inventory][update], [Inventory schema][inventory], [Errors][errors] |
| DELETE `/inventories/{inventory_id}` | Integer ID; no body. | 204; empty HTTP body; later GET returns 404. | **Implemented deletion**, **unverified exact success wire choice:** method page says empty data; general notes allow no body. We select 204 rather than claiming it is the only upstream result. | [Delete inventory][delete], [General notes][general], [Errors][errors] |
| GET `/orders` | No body. `direction=in` by default (`out` also accepted); `filed=false` by default; optional `status` inclusion/exclusion. | 200; array of summaries, not full order details. Fields below. | **Partial:** incoming/outgoing and filed/unfiled scenarios supported. Summary `grandtotal` spelling conflicts with resource `grand_total`; see ambiguities. | [List orders][list-orders], [Order schema][order] |
| GET `/orders/{order_id}` | Integer path parameter; no body. | 200; order detail object; missing ID 404. | **Partial:** configurable metadata and nominal currencies; see the [lifecycle guide](order-lifecycle.md). | [Get order][get-order], [Order schema][order], [Errors][errors] |
| PUT `/orders/{order_id}` | Permitted charges, tracking, remarks, filed state | 200; updated order | Other fields ignored; costs locked after settlement. Automatic upstream tax recomputation unsupported | [Update order](https://www.bricklink.com/v3/api.page?page=update-order) |
| PUT `/orders/{order_id}/status` | `field:"status"`, `value` | 200; null data | Help-derived role/window restrictions are simulator interpretation, not a complete API transition graph | [Update status](https://www.bricklink.com/v3/api.page?page=update-order-status) |
| PUT `/orders/{order_id}/payment_status` | `field:"payment_status"`, `value` | 200; null data | Account-specific settings unsupported | [Update payment](https://www.bricklink.com/v3/api.page?page=update-payment-status) |
| GET `/orders/{order_id}/items` | Integer path parameter; no body. | 200; nested arrays, one inner array per batch; missing order 404. | **Partial:** currently one synthetic batch only; item schema below. No real multi-batch edits, tier prices, or currency conversion. | [Get order items][items], [Order schema][order] |

Only exact base-path segments route to marketplace methods. A lookalike path such as `/api/store/v1inventories` is not accepted. Unsupported methods on these recognized routes return 405; unknown routes return 404. **Caveat:** a method absent from this simulator may exist upstream (for example order invoice operations). A local 405 is not evidence that BrickLink lacks that operation.

## Supported fields and local defaults

### Inventory resources

Types are based on the [current inventory table][inventory]. Except where indicated, the docs do **not** establish creation defaults, requiredness, or null behavior precisely. The following defaults and strictness are simulator policies and are tested as such.

| Field | Wire type / supported values | Local create behavior |
| --- | --- | --- |
| `inventory_id` | Integer | Server-assigned; not accepted in create payload |
| `item` | Object | Required |
| `item.no` | Nonempty string | Required; no catalog existence check |
| `item.type` | String: MINIFIG, PART, SET, BOOK, GEAR, CATALOG, INSTRUCTION, UNSORTED_LOT, ORIGINAL_BOX | Required; uppercase input |
| `item.name` | String | Defaults to empty; not populated from a catalog |
| `item.category_id` | Integer | Defaults to 0; canonical schema spelling |
| `color_id` | Integer | Required; zero allowed; not writable by PUT |
| `quantity` | Integer | Required; local bound 0..1,000,000,000 |
| `new_or_used` | String: N/U | Required |
| `unit_price` | Fixed-point decimal, serialized as a string as in official examples | Required string input, nonnegative and ≤ 1,000,000,000; rounded upward to four places |
| `description`, `remarks` | Strings | Default empty |
| `is_stock_room` | Boolean | Default false |
| `stock_room_id` | String: A/B/C | Defaults A, including when not in stockroom |
| `date_created` | ISO8601 string | Generated from the sandbox clock; UTC milliseconds |

Unsupported documented inventory fields include `color_name`, `completeness`, `bind_id`, `bulk`, `is_retain`, `my_cost`, `sale_rate`, all tier quantity/price fields, and `my_weight`. Their omission makes resource compatibility partial. [Issue #6](https://github.com/Espeer5/bricklink-sandbox/issues/6) owns broader inventory behavior; [issue #13](https://github.com/Espeer5/bricklink-sandbox/issues/13) owns catalog fixtures.

PUT supports only `quantity`, `unit_price`, `description`, and `remarks`. Quantity input must be a string beginning with `+` or `-`; its output is an integer. Omitted update fields remain unchanged. Explicit null is treated as omitted for these optional update fields. Create fields with null are rejected, including optional string fields. Unknown fields in create/update are rejected; a rejected update leaves the lot unchanged. **Upstream null/unknown semantics are unverified.**

### Order resources and summaries

Order detail currently emits integer `order_id`; timestamp strings `date_ordered` and `date_status_changed`; strings `seller_name`, `store_name`, `buyer_name`, `buyer_email`, `status`, `remarks`; boolean `is_filed`; integer `total_count`/`unique_count`; and objects `payment`, `shipping`, `cost`.

Local payment/shipping metadata, explicit charges, currency labels, and lifecycle policies are specified in the [order lifecycle guide](order-lifecycle.md). Default orders remain USD with empty shipping, zero shipping charge, and no `date_paid`; omission versus null for unpaid dates remains unverified. Automatic tax, FX conversion, display costs, invoice flags, and multi-batch edits remain unsupported.

GET orders now projects the documented summary fields: `order_id`, `date_ordered`, `seller_name`, `store_name`, `buyer_name`, `total_count`, `unique_count`, `status`; payment method/status/currency and date_paid when present; cost subtotal/grand_total/currency. Full-detail-only fields such as buyer email, remarks, shipping, and date_status_changed are not returned in summaries. Filed filtering uses internal state even though `is_filed` is not projected into summaries.

Order-item responses contain only the implemented order-item fields: integer `inventory_id`, item object, integer `color_id` and `quantity`, string `new_or_used`, decimal strings `unit_price`, `unit_price_final`, `disp_unit_price`, `disp_unit_price_final`, string `currency_code`/`disp_currency_code`, and string `remarks`/`description`. Inventory-only fields such as `date_created`, `is_stock_room`, and `stock_room_id` are no longer leaked into order items. Prices are preserved when the order is created. Display prices equal original prices because the simulator has no currency conversion or tier discounts. SET completeness, weights, and color names remain unsupported.

## Cross-cutting behavior matrix

| Behavior | Status | Source-backed finding and simulator decision |
| --- | --- | --- |
| URL-encoded JSON PUT/POST | **Implemented** | [General notes][general] specify encoding the resource JSON. The whole JSON document is percent-decoded once; percent-encoded plus signs, UTF-8, ampersands, equals signs, and literal percent strings are tested. |
| Plain JSON and `data=` wrapper | **Implemented local extensions; upstream unverified** | Neither alternative is explicitly promised by general notes. Both remain supported for existing users. Wrapper accepts one `data` field; form `+` means space, `%2B` means plus. Raw percent-encoded JSON preserves literal `+`. |
| Request media types | **Unverified upstream set** | Manual does not list accepted Content-Type values. Locally accept JSON/form types, optional parameters, or no header; unsupported nonempty-body media types return documented error 415. |
| UTF-8 | **Implemented** | [General notes][general] require UTF-8. Invalid JSON/UTF-8/percent escapes fail before mutation; exact upstream error precedence is unverified. |
| Quantity delta | **Implemented** | [Update inventory][update] explicitly requires a signed delta. Absolute integer/string updates fail. Local bounds and exact error messages are simulator policy. |
| Four-place decimal rounding | **Implemented interpretation** | [General notes][general] say excess precision rounds up. Locally use ceiling for nonnegative prices: 0.12341 → 0.1235; 0.12340 → 0.1234. No binary-float arithmetic. Precise live edge/tie behavior and accepted numeric JSON types remain **unverified**; inputs beyond the exact Rust decimal parser range are rejected instead of silently losing precision. |
| Sold-out / zero quantity | **Partial; deliberate deviation** | [Inventory schema][inventory] describes `is_retain` as controlling retention after sellout. Simulator does not implement that flag and always retains depleted lots. Direct zero-quantity PUT/POST deletion semantics and retain defaults are **unverified**, not inferred from a sale. |
| Inventory statuses | **Partial** | Y/S/B/C/N/R filter vocabulary is documented. Stockrooms S/B/C are implemented; reserved R is not. Local available/unavailable mapping uses positive/zero quantity. |
| Filters | **Implemented subset** | Comma-separated inclusion/exclusion and lowercase type/status examples are documented. Combining inclusion and exclusion uses local include-then-exclude precedence; duplicate query keys use last value and unknown filters fail. Those edge rules are **unverified**. |
| Optional / null / unknown fields | **Unverified upstream policy** | Resource tables specify types but no uniform null/default/unknown policy. Local rules above are explicitly tested, not promoted to verified upstream behavior. |
| Nested order batches | **Partial** | [Get items][items] specifies an array of arrays. Shape implemented and tested; multi-batch orders not yet modeled. |
| Authentication | **Unsupported validation** | [Auth][auth] documents header or Authorization query parameter. Both are ignored in permissive mode; query auth no longer causes an unknown-filter error. No signing, permissions, or nonce verification ([issue #7](https://github.com/Espeer5/bricklink-sandbox/issues/7)). |
| SSL | **Deliberate deviation** | [General notes][general] require upstream HTTPS. This local service uses HTTP. |
| Financial totals | **Partial** | Decimal unit-price × quantity sums; no shipping/tax/discount/currency rules beyond synthetic defaults. |

### Error and response envelope

For nonempty responses, `meta` is an object with integer `code`, string `message`, string `description`; `data` is the result object/array or null on local errors. Descriptions are explanatory local strings, not copied upstream error messages. The [result-code table][errors] supplies these code/name pairs:

| HTTP | `meta.message` | Local use |
| --- | --- | --- |
| 200 | OK | Successful reads/updates |
| 201 | OK_CREATED | Created resource |
| 204 | No envelope | Deletion, selected policy; documented name would be OK_NO_CONTENT |
| 400 | INVALID_URI | Malformed integer path (local mapping) |
| 400 | INVALID_REQUEST_BODY | Malformed JSON/encoding or local request-body limit |
| 400 | PARAMETER_MISSING_OR_INVALID | Parsed but invalid/missing supported fields |
| 404 | RESOURCE_NOT_FOUND | Missing resource or unknown route |
| 405 | METHOD_NOT_ALLOWED | Unsupported method on recognized local route |
| 415 | UNSUPPORTED_MEDIA_TYPE | Unsupported nonempty-body Content-Type |
| 422 | RESOURCE_UPDATE_NOT_ALLOWED | Mock purchase exceeds available stock (mock-only policy) |

401 BAD_OAUTH_REQUEST, 403 PERMISSION_DENIED, and 500 INTERNAL_SERVER_ERROR are documented upstream but not deliberately simulated. Future authentication/fault scenarios belong to issues #7/#4. Error selection when multiple inputs are wrong and null versus omitted error `data` are not verified against production. Request bodies have a local 1 MiB limit; loading fixture files is separate.

## Documentation conflicts and unresolved points

These remain **unverified**, even though the tests choose stable local behavior:

1. `meta.code` is integer in the schema but quoted in an example. Emit integer, following the schema.
2. `item.category_id` in tables conflicts with `categoryID` in examples. Use `category_id`.
3. Order resource table calls `order_id` a string; examples use integers and routes specify integer IDs. Keep integer IDs locally.
4. Order list names `cost.grandtotal`; detail schema/example uses `cost.grand_total`. Use `grand_total` consistently, without inventing duplicate aliases.
5. DELETE's empty `data` description does not uniquely specify HTTP 204/no body. Keep the general-notes-compatible no-body choice.
6. General notes specify URL-encoded JSON but not the media type, plus/space algorithm, or a `data` wrapper. Implement one-pass percent decoding; mark wrapper/plain JSON as extensions.
7. Missing/null/default fields, mixed include/exclude precedence, and direct zero-write behavior lack sufficient detail for a production-equivalence claim.

Resolving these needs stronger official clarification or separately authorized observation of appropriate live read behavior. A green simulator test must never be used as the evidence that resolves an upstream ambiguity.

## Simulator-only endpoints

These have **no BrickLink contract**. They are documented and covered by local integration tests, not upstream contract claims.

| Endpoint | Request / defaults | Response |
| --- | --- | --- |
| GET `/health` | No body | 200; data status string |
| POST `/__mock/orders` | Required nonempty items array of integer inventory_id/quantity; optional details | 201; configurable order, atomic incoming stock deduction; invalid input 400, missing lot 404, unavailable stock 422 |
| POST `/__mock/orders/{id}/{action}` | items/cancel/refund/restock; [payloads and policies](order-lifecycle.md) | 200; order or effect state; atomic mutations, bounded restitution |
| GET `/__mock/orders/{id}/state` | No body | 200; reservation and refund state |
| POST `/__mock/reset` | No body | 200; reset boolean; restores selected fixture |
| GET `/__mock/clock` | No body | 200; mode/now strings |
| POST `/__mock/clock/advance` | Required nonnegative integer seconds; fixed clock only | 200; mode/now; invalid mode/input/overflow 400 |
| POST `/__mock/replay` | No body; fixed clock only | 200; steps array and clock; invalid mode 400 |

Fixture versions, deterministic IDs, fixed time, synthetic buyers, and local reset/replay semantics are simulator controls. [Fixture documentation](../fixtures/README.md) remains their specification.

## Contract suite and maintenance

`tests/contracts/cases.json` contains authored synthetic requests and expected full `data` responses. No production response dumps or generated snapshots of the implementation are used. Each case records source IDs, a scope note, and one of:

- `documented-shape-with-synthetic-values`: checks a documented shape/operation; exact fake values and omitted unsupported fields are local.
- `documentation-interpretation`: chooses one interpretation of incomplete or conflicting documentation.
- `simulator-policy`: protects an explicit extension or deviation without claiming upstream compatibility.

`tests/contracts.rs` checks each source reference against the manifest, creates independent fixed-clock state for each case, then compares status, media type, envelope types/result message, and exact synthetic response data. DELETE bodies must be empty under our chosen policy. Multiple steps verify mutation and failure atomicity. The suite is offline and never needs BrickLink credentials. Existing scenario/API tests provide separate simulation coverage.

```sh
cargo test --locked --test contracts
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

For a contract change, re-read the relevant **current** official pages, update the manifest/date and matrix, classify uncertain behavior honestly, and author a synthetic regression case before adjusting implementation. Keep fixture expectations independent of the response-building code. Full API parity, bulk/retention behavior, catalog operations, full lifecycle parity, notifications, and strict OAuth remain separately tracked work.

[general]: https://www.bricklink.com/v3/api.page?page=general-notes
[errors]: https://www.bricklink.com/v3/api.page?page=error-handling
[auth]: https://www.bricklink.com/v3/api.page?page=auth
[inventory]: https://www.bricklink.com/v3/api.page?page=resource-representations-inventory
[order]: https://www.bricklink.com/v3/api.page?page=resource-representations-order
[list-inventory]: https://www.bricklink.com/v3/api.page?page=get-inventories
[get-inventory]: https://www.bricklink.com/v3/api.page?page=get-inventory
[create]: https://www.bricklink.com/v3/api.page?page=create-inventory
[update]: https://www.bricklink.com/v3/api.page?page=update-inventory
[delete]: https://www.bricklink.com/v3/api.page?page=delete-inventory
[list-orders]: https://www.bricklink.com/v3/api.page?page=get-orders
[get-order]: https://www.bricklink.com/v3/api.page?page=get-order
[items]: https://www.bricklink.com/v3/api.page?page=get-order-items
