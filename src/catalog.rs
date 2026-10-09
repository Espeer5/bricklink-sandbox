//! Fixture-backed catalog data. No remote acquisition or inferred identifiers.
use super::*;
use std::collections::BTreeSet;

#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Catalog {
    #[serde(default)]
    pub validate_inventory: bool,
    #[serde(default)]
    pub items: Vec<CatalogItem>,
    #[serde(default)]
    pub colors: Vec<Value>,
    #[serde(default)]
    pub categories: Vec<Value>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CatalogItem {
    pub item: Value,
    pub known_colors: Vec<Value>,
    #[serde(default)]
    pub images: BTreeMap<u64, Value>,
    pub subsets: Option<Vec<Value>>,
    pub supersets: Option<Vec<Value>>,
    pub element_mappings: Option<Vec<Value>>,
}
const TYPES: &[&str] = &[
    "PART",
    "SET",
    "MINIFIG",
    "BOOK",
    "GEAR",
    "CATALOG",
    "INSTRUCTION",
    "UNSORTED_LOT",
    "ORIGINAL_BOX",
];
fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, ApiError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid(format!("Catalog {key} must be a nonempty string")))
}
fn integer(value: &Value, key: &str) -> Result<u64, ApiError> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid(format!("Catalog {key} must be an unsigned integer")))
}
impl Catalog {
    pub(super) fn parse(text: &str) -> Result<Self, ApiError> {
        let catalog: Self =
            serde_json::from_str(text).map_err(|_| invalid("Invalid catalog fixture shape"))?;
        catalog.validate()?;
        Ok(catalog)
    }
    pub(super) fn validate(&self) -> Result<(), ApiError> {
        let mut colors = BTreeSet::new();
        for color in &self.colors {
            if !colors.insert(integer(color, "color_id")?) {
                return Err(invalid("Duplicate catalog color"));
            }
            for field in ["color_name", "color_code", "color_type"] {
                string(color, field)?;
            }
        }
        let mut categories = BTreeMap::new();
        for category in &self.categories {
            let id = integer(category, "category_id")?;
            if id == 0
                || categories
                    .insert(id, integer(category, "parent_id")?)
                    .is_some()
            {
                return Err(invalid("Invalid or duplicate category ID"));
            }
            string(category, "category_name")?;
        }
        for id in categories.keys() {
            let mut seen = BTreeSet::new();
            let mut current = *id;
            while current != 0 {
                if !seen.insert(current) {
                    return Err(invalid("Cyclic category hierarchy"));
                }
                current = *categories
                    .get(&current)
                    .ok_or_else(|| invalid("Unknown category parent"))?;
            }
        }
        let mut identities = BTreeSet::new();
        for record in &self.items {
            let no = string(&record.item, "no")?;
            let kind = string(&record.item, "type")?;
            if !TYPES.contains(&kind) || no.contains('/') || !identities.insert((kind, no)) {
                return Err(invalid("Invalid or duplicate catalog identity"));
            }
            string(&record.item, "name")?;
            if !categories.contains_key(&integer(&record.item, "category_id")?) {
                return Err(invalid("Unknown item category"));
            }
            if record
                .item
                .get("is_obsolete")
                .is_some_and(|v| !v.is_boolean())
            {
                return Err(invalid("is_obsolete must be boolean"));
            }
            let mut known = BTreeSet::new();
            for color in &record.known_colors {
                let id = integer(color, "color_id")?;
                integer(color, "quantity")?;
                if !colors.contains(&id) || !known.insert(id) {
                    return Err(invalid("Unknown or duplicate known color"));
                }
            }
            for (color, image) in &record.images {
                if !known.contains(color)
                    || string(image, "no")? != no
                    || string(image, "type")? != kind
                {
                    return Err(invalid("Image identity/color mismatch"));
                }
                string(image, "thumbnail_url")?;
            }
            if let Some(mappings) = &record.element_mappings {
                if kind != "PART" {
                    return Err(invalid("Element mappings apply only to PART"));
                }
                for mapping in mappings {
                    if mapping["item"]["no"] != no
                        || mapping["item"]["type"] != kind
                        || !known.contains(&integer(mapping, "color_id")?)
                    {
                        return Err(invalid("Element mapping identity/color mismatch"));
                    }
                    string(mapping, "element_id")?;
                    string(mapping, "color_name")?;
                }
            }
            // Relationship resources are authored directly; no automatic part-out or theme inference.
            for (name, groups) in [
                ("subsets", &record.subsets),
                ("supersets", &record.supersets),
            ] {
                if let Some(groups) = groups {
                    for group in groups {
                        integer(
                            group,
                            if name == "subsets" {
                                "match_no"
                            } else {
                                "color_id"
                            },
                        )?;
                        let entries = group["entries"]
                            .as_array()
                            .ok_or_else(|| invalid("Relationship entries must be an array"))?;
                        for entry in entries {
                            string(&entry["item"], "no")?;
                            string(&entry["item"], "type")?;
                            string(&entry["item"], "name")?;
                            integer(&entry["item"], "category_id")?;
                            integer(entry, "quantity")?;
                            if name == "subsets" {
                                integer(entry, "color_id")?;
                                integer(entry, "extra_quantity")?;
                                for flag in ["is_alternate", "is_counterpart"] {
                                    if !entry[flag].is_boolean() {
                                        return Err(invalid("Subset flags must be boolean"));
                                    }
                                }
                            } else if !["A", "C", "E", "R"].contains(&string(entry, "appear_as")?) {
                                return Err(invalid("Invalid superset appear_as"));
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
    pub(super) fn check_inventory(&self, input: &InventoryInput) -> Result<(), ApiError> {
        if !self.validate_inventory {
            return Ok(());
        }
        let record = self
            .items
            .iter()
            .find(|r| r.item["no"] == input.item.no && r.item["type"] == input.item.kind)
            .ok_or_else(|| invalid("Inventory identity absent from strict catalog"))?;
        if record.item["is_obsolete"] == true
            || !record
                .known_colors
                .iter()
                .any(|c| c["color_id"] == input.color_id)
        {
            return Err(invalid(
                "Inventory color unsupported or catalog item obsolete",
            ));
        }
        if input.item.category_id != 0 && record.item["category_id"] != input.item.category_id {
            return Err(invalid("Inventory category conflicts with catalog"));
        }
        Ok(())
    }
    pub(super) fn dispatch(&self, parts: &[&str], query: &BTreeMap<String, String>) -> ApiResult {
        let ok = |v: Value| Ok((StatusCode::OK, v));
        match parts {
            [resource @ ("colors" | "categories")] | [resource @ ("colors" | "categories"), _] => {
                if !query.is_empty() {
                    return Err(invalid("Reference endpoints take no query parameters"));
                }
                let (values, key) = if *resource == "colors" {
                    (&self.colors, "color_id")
                } else {
                    (&self.categories, "category_id")
                };
                if parts.len() == 1 {
                    return ok(json!(values));
                }
                let id: u64 = parts[1]
                    .parse()
                    .map_err(|_| invalid("Invalid reference ID"))?;
                ok(values
                    .iter()
                    .find(|v| v[key] == id)
                    .ok_or_else(missing)?
                    .clone())
            }
            ["items", kind, no, ..] => {
                let kind = kind.to_uppercase();
                if !TYPES.contains(&kind.as_str()) {
                    return Err(invalid("Unsupported catalog type"));
                }
                let record = self
                    .items
                    .iter()
                    .find(|r| r.item["type"] == kind && r.item["no"] == *no)
                    .ok_or_else(missing)?;
                if !query.is_empty() {
                    return Err(invalid(
                        "Filtered/exploded catalog representations are not implemented",
                    ));
                }
                match &parts[3..] {
                    [] => ok(record.item.clone()),
                    ["colors"] => ok(json!(record.known_colors)),
                    ["images", color] => ok(record
                        .images
                        .get(
                            &color
                                .parse::<u64>()
                                .map_err(|_| invalid("Invalid color ID"))?,
                        )
                        .ok_or_else(missing)?
                        .clone()),
                    ["subsets"] => ok(json!(record.subsets.as_ref().ok_or_else(missing)?)),
                    ["supersets"] => ok(json!(record.supersets.as_ref().ok_or_else(missing)?)),
                    _ => Err(missing()),
                }
            }
            ["item_mapping", kind, no] if kind.eq_ignore_ascii_case("PART") => {
                if query.keys().any(|k| k != "color_id") {
                    return Err(invalid("Unsupported mapping filter"));
                }
                let color = query
                    .get("color_id")
                    .map(|s| s.parse::<u64>().map_err(|_| invalid("Invalid color ID")))
                    .transpose()?;
                let record = self
                    .items
                    .iter()
                    .find(|r| r.item["type"] == "PART" && r.item["no"] == *no)
                    .ok_or_else(missing)?;
                let entries = record.element_mappings.as_ref().ok_or_else(missing)?;
                ok(json!(
                    entries
                        .iter()
                        .filter(|m| color.is_none_or(|c| m["color_id"] == c))
                        .collect::<Vec<_>>()
                ))
            }
            ["item_mapping", element] => {
                if !query.is_empty() {
                    return Err(invalid("Reverse mapping takes no query parameters"));
                }
                let values: Vec<_> = self
                    .items
                    .iter()
                    .filter_map(|r| r.element_mappings.as_ref())
                    .flatten()
                    .filter(|m| m["element_id"] == *element)
                    .collect();
                if values.is_empty() {
                    return Err(missing());
                }
                ok(json!(values))
            }
            _ => Err(missing()),
        }
    }
}
