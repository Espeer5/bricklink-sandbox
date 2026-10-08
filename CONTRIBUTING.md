# Contributing

Issues and pull requests are welcome. For a new endpoint, include its documented request/response contract, a link to the official source, and integration tests for successful and rejected operations. Keep mock-only scenario controls under `/__mock/`.

Use synthetic customer, order, and inventory data. Do not commit API credentials or real buyer details. Clearly distinguish observed BrickLink behavior from local simulation choices, and update the README compatibility table and limits when behavior changes.

Before submitting, run `cargo fmt --all -- --check`, `cargo clippy --locked --all-targets -- -D warnings`, and `cargo test --locked`. Contributions are licensed under the project's MIT license.
