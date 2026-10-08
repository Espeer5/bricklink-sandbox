//! Regenerate the committed large synthetic fixture without external data.
use serde_json::{Value, json};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let destination = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "fixtures/large.json".into());
    let inventories: Vec<Value> = (0..3000).map(|i| {
        json!({"inventory_id": 1000 + i, "inventory": {
            "item": {"no": (["3001", "3002", "3003"][i % 3]), "type": "PART", "name": format!("Synthetic lot {i}"), "category_id": 5},
            "color_id": ([1, 4, 5, 6, 11][i % 5]), "quantity": if i % 13 == 0 { 0 } else { 100 + i % 50 },
            "unit_price": format!("0.{:04}", 100 + i % 900), "new_or_used": if i % 2 == 0 { "N" } else { "U" },
            "remarks": format!("BIN-{:03}-{:02}", i / 20, i % 20),
            "is_stock_room": i % 17 == 0, "stock_room_id": (["A", "B", "C"][i % 3])
        }})
    }).collect();
    let orders: Vec<Value> = (0..200).map(|i| {
        let items: Vec<Value> = (0..1 + i % 4).map(|j| json!({"inventory_id": 1000 + (i * 7 + j) % 3000, "quantity": 1 + (i + j) % 10})).collect();
        json!({"order_id": 10000 + i, "items": items, "status": (["PENDING", "PAID", "SHIPPED", "COMPLETED"][i % 4]),
            "is_filed": i % 4 == 3, "buyer_name": format!("synthetic_buyer_{i:03}"),
            "date_ordered": format!("2025-12-{:02}T09:00:00.000Z", 1 + i % 28)})
    }).collect();
    let fixture = json!({"version": 1, "clock": {"mode": "fixed", "now": "2026-01-01T12:00:00.000Z"},
    "inventories": inventories, "orders": orders, "steps": [
        {"action": "advance_clock", "seconds": 60},
        {"action": "create_order", "items": [{"inventory_id": 1001, "quantity": 3}, {"inventory_id": 1002, "quantity": 4}]}
    ]});
    let contents = serde_json::to_string_pretty(&fixture)? + "\n";
    let _validated = bricklink_sandbox::app_from_fixture(&contents)?;
    std::fs::write(destination, contents)?;
    Ok(())
}
