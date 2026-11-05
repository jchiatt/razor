//! Codex aa854dab JSON Schema declarations, with bounded input and recursion: adapted from Codex's
//! json_schema_types.rs under the Apache License 2.0, as NOTICE records.

use serde_json::Value as JsonValue;
use std::collections::BTreeMap;

// Expose one nested recursive shape, then fall back to `unknown` on the next
// occurrence so generated tool declarations remain finite.
const MAX_LOCAL_REF_EXPANSIONS_PER_PATH: usize = 2;
// Bound repeated refs and DAG fan-out separately from cycle depth so one
// compact schema cannot expand into an arbitrarily large model-visible item.
const MAX_TOTAL_LOCAL_REF_EXPANSIONS: usize = 32;
// Bound individual schema rendering before assembling the final declaration.
const DEFAULT_INPUT_SCHEMA_MAX_BYTES: usize = 16_000;
// Charge intermediate render strings as they are built so repeated local refs
// cannot allocate unbounded expanded copies before the final schema cap runs.
const RENDER_WORK_MULTIPLIER: usize = 4;

/// Matches Codex's declaration syntax; oversized schemas and exhausted work become `unknown`.
pub(super) fn render(schema: &JsonValue) -> String {
    let max_bytes = DEFAULT_INPUT_SCHEMA_MAX_BYTES;
    // Bound input traversal, temporary property lists and recursion as well as output.
    // Local references can expand this input only within Codex's separate work/ref budgets.
    if schema_size(schema, 0) > 128_000 {
        return "unknown".into();
    }
    let rendered = JsonSchemaTypeRenderer::new(schema, max_bytes).render(schema);
    if rendered.len() > max_bytes {
        "unknown".to_string()
    } else {
        rendered
    }
}

struct JsonSchemaTypeRenderer<'a> {
    root: &'a JsonValue,
    nested_schema_resource_depth: usize,
    active_local_ref_expansions: BTreeMap<String, usize>,
    remaining_local_ref_expansions: usize,
    remaining_render_work_bytes: usize,
    render_work_budget_exhausted: bool,
    depth: usize,
}

impl<'a> JsonSchemaTypeRenderer<'a> {
    fn new(root: &'a JsonValue, max_bytes: usize) -> Self {
        Self {
            root,
            nested_schema_resource_depth: 0,
            active_local_ref_expansions: BTreeMap::new(),
            remaining_local_ref_expansions: MAX_TOTAL_LOCAL_REF_EXPANSIONS,
            remaining_render_work_bytes: max_bytes.saturating_mul(RENDER_WORK_MULTIPLIER),
            render_work_budget_exhausted: false,
            depth: 0,
        }
    }

    fn render(&mut self, schema: &JsonValue) -> String {
        if self.depth >= 64 {
            self.render_work_budget_exhausted = true;
        }
        if self.render_work_budget_exhausted {
            return "unknown".to_string();
        }

        // A nested `$id` starts a new schema resource. Fragment-only refs below
        // it are scoped to that resource, not to the outer document root.
        let enters_nested_schema_resource = !std::ptr::eq(schema, self.root)
            && schema
                .as_object()
                .is_some_and(|map| map.contains_key("$id"));
        if enters_nested_schema_resource {
            self.nested_schema_resource_depth += 1;
        }

        self.depth += 1;
        let rendered = match schema {
            JsonValue::Bool(true) => "unknown".to_string(),
            JsonValue::Bool(false) => "never".to_string(),
            JsonValue::Object(map) => self.render_map(map),
            _ => "unknown".to_string(),
        };
        if enters_nested_schema_resource {
            self.nested_schema_resource_depth -= 1;
        }
        self.depth -= 1;
        self.finish_render(rendered)
    }

    fn render_map(&mut self, map: &serde_json::Map<String, JsonValue>) -> String {
        if map.contains_key("$ref") {
            return self.render_ref(map);
        }

        if let Some(value) = map.get("const") {
            return self.render_literal(value);
        }

        if let Some(values) = map.get("enum").and_then(JsonValue::as_array) {
            let mut rendered = Vec::new();
            for value in values {
                let literal = self.render_literal(value);
                if self.render_work_budget_exhausted || !self.consume_render_work(literal.len()) {
                    return "unknown".to_string();
                }
                rendered.push(literal);
            }
            if !rendered.is_empty() {
                return rendered.join(" | ");
            }
        }

        for key in ["anyOf", "oneOf", "allOf"] {
            if let Some(variants) = map.get(key).and_then(JsonValue::as_array) {
                let mut rendered = Vec::new();
                for variant in variants {
                    if self.render_work_budget_exhausted {
                        return "unknown".to_string();
                    }
                    let value = self.render(variant);
                    rendered.push(if key == "allOf" && value.contains(" | ") {
                        format!("({value})")
                    } else {
                        value
                    });
                }
                if !rendered.is_empty() {
                    return rendered.join(if key == "allOf" { " & " } else { " | " });
                }
            }
        }

        if let Some(schema_type) = map.get("type") {
            if let Some(types) = schema_type.as_array() {
                let mut rendered = Vec::new();
                for schema_type in types.iter().filter_map(JsonValue::as_str) {
                    if self.render_work_budget_exhausted {
                        return "unknown".to_string();
                    }
                    rendered.push(self.render_type_keyword(map, schema_type));
                }
                if !rendered.is_empty() {
                    return rendered.join(" | ");
                }
            }

            if let Some(schema_type) = schema_type.as_str() {
                return self.render_type_keyword(map, schema_type);
            }
        }

        if map.contains_key("properties")
            || map.contains_key("additionalProperties")
            || map.contains_key("required")
        {
            return self.render_object(map);
        }

        if map.contains_key("items") || map.contains_key("prefixItems") {
            return self.render_array(map);
        }

        "unknown".to_string()
    }

    fn render_ref(&mut self, map: &serde_json::Map<String, JsonValue>) -> String {
        let referenced_type = if self.nested_schema_resource_depth > 0 {
            None
        } else {
            map.get("$ref")
                .and_then(JsonValue::as_str)
                .and_then(local_json_pointer)
                .and_then(|pointer| {
                    let active_expansions = self
                        .active_local_ref_expansions
                        .get(&pointer)
                        .copied()
                        .unwrap_or_default();
                    if active_expansions >= MAX_LOCAL_REF_EXPANSIONS_PER_PATH {
                        return Some("unknown".to_string());
                    }
                    if self.remaining_local_ref_expansions == 0 {
                        return Some("unknown".to_string());
                    }

                    let root = self.root;
                    let target = if pointer.is_empty() {
                        Some(root)
                    } else {
                        root.pointer(&pointer)
                    }?;
                    self.remaining_local_ref_expansions -= 1;
                    self.active_local_ref_expansions
                        .insert(pointer.clone(), active_expansions + 1);

                    let rendered = self.render(target);
                    if active_expansions == 0 {
                        self.active_local_ref_expansions.remove(&pointer);
                    } else {
                        self.active_local_ref_expansions
                            .insert(pointer, active_expansions);
                    }
                    Some(rendered)
                })
        }
        .unwrap_or_else(|| "unknown".to_string());
        if self.render_work_budget_exhausted {
            return "unknown".to_string();
        }

        let siblings = map
            .iter()
            .filter(|(key, _)| !matches!(key.as_str(), "$ref" | "$defs" | "definitions"))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        if !has_renderable_schema_keywords(&siblings) {
            return referenced_type;
        }

        let sibling_type = self.render_map(&siblings);
        match (referenced_type.as_str(), sibling_type.as_str()) {
            ("unknown", _) => sibling_type,
            (_, "unknown") => referenced_type,
            _ => format!("({referenced_type}) & ({sibling_type})"),
        }
    }

    fn render_type_keyword(
        &mut self,
        map: &serde_json::Map<String, JsonValue>,
        schema_type: &str,
    ) -> String {
        match schema_type {
            "string" => "string".to_string(),
            "number" | "integer" => "number".to_string(),
            "boolean" => "boolean".to_string(),
            "null" => "null".to_string(),
            "array" => self.render_array(map),
            "object" => self.render_object(map),
            _ => "unknown".to_string(),
        }
    }

    fn render_array(&mut self, map: &serde_json::Map<String, JsonValue>) -> String {
        if let Some(items) = map.get("items") {
            let item_type = self.render(items);
            if self.render_work_budget_exhausted {
                return "unknown".to_string();
            }
            return format!("Array<{item_type}>");
        }

        if let Some(items) = map.get("prefixItems").and_then(JsonValue::as_array) {
            let mut item_types = Vec::new();
            for item in items {
                if self.render_work_budget_exhausted {
                    return "unknown".to_string();
                }
                item_types.push(self.render(item));
            }
            if !item_types.is_empty() {
                return format!("[{}]", item_types.join(", "));
            }
        }

        "unknown[]".to_string()
    }

    fn append_additional_properties_line(
        &mut self,
        lines: &mut Vec<String>,
        map: &serde_json::Map<String, JsonValue>,
        properties: &serde_json::Map<String, JsonValue>,
        line_prefix: &str,
    ) -> bool {
        if let Some(additional_properties) = map.get("additionalProperties") {
            let property_type = match additional_properties {
                JsonValue::Bool(true) => Some("unknown".to_string()),
                JsonValue::Bool(false) => None,
                value => Some(self.render(value)),
            };

            if let Some(property_type) = property_type {
                return self.push_render_line(
                    lines,
                    format!("{line_prefix}[key: string]: {property_type};"),
                );
            }
        } else if properties.is_empty() {
            return self.push_render_line(lines, format!("{line_prefix}[key: string]: unknown;"));
        }
        true
    }

    fn render_object_property(
        &mut self,
        name: &str,
        value: &JsonValue,
        required: &[&str],
    ) -> String {
        if name.len() > self.remaining_render_work_bytes {
            self.render_work_budget_exhausted = true;
            return "unknown".to_string();
        }
        let optional = if required.iter().any(|required_name| required_name == &name) {
            ""
        } else {
            "?"
        };
        let property_name = render_json_schema_property_name(name);
        let property_type = self.render(value);
        if self.render_work_budget_exhausted {
            return "unknown".to_string();
        }
        format!("{property_name}{optional}: {property_type};")
    }

    fn render_object(&mut self, map: &serde_json::Map<String, JsonValue>) -> String {
        let required = map
            .get("required")
            .and_then(JsonValue::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(JsonValue::as_str)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let empty_properties = serde_json::Map::new();
        let properties = map
            .get("properties")
            .and_then(JsonValue::as_object)
            .unwrap_or(&empty_properties);

        let mut sorted_properties = properties.iter().collect::<Vec<_>>();
        sorted_properties.sort_unstable_by_key(|(name_a, _)| *name_a);
        let multiline = sorted_properties
            .iter()
            .any(|(_, value)| has_property_description(value));
        let prefix = if multiline { "  " } else { "" };
        let mut lines = Vec::new();
        if multiline && !self.push_render_line(&mut lines, "{".to_string()) {
            return "unknown".to_string();
        }
        for (name, value) in sorted_properties {
            if multiline
                && let Some(description) = value.get("description").and_then(JsonValue::as_str)
            {
                for description_line in description
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                {
                    if description_line.len().saturating_add(5) > self.remaining_render_work_bytes
                        || !self.push_render_line(&mut lines, format!("  // {description_line}"))
                    {
                        return "unknown".to_string();
                    }
                }
            }
            let property = self.render_object_property(name, value, &required);
            if self.render_work_budget_exhausted
                || !self.push_render_line(
                    &mut lines,
                    if multiline {
                        format!("  {property}")
                    } else {
                        property
                    },
                )
            {
                return "unknown".to_string();
            }
        }

        if !self.append_additional_properties_line(&mut lines, map, properties, prefix) {
            return "unknown".to_string();
        }
        if multiline {
            return if self.push_render_line(&mut lines, "}".to_string()) {
                lines.join("\n")
            } else {
                "unknown".to_string()
            };
        }
        if lines.is_empty() {
            return "{}".to_string();
        }

        format!("{{ {} }}", lines.join(" "))
    }

    fn finish_render(&mut self, rendered: String) -> String {
        if self.consume_render_work(rendered.len()) {
            rendered
        } else {
            "unknown".to_string()
        }
    }

    fn render_literal(&mut self, value: &JsonValue) -> String {
        if json_literal_serialization_upper_bound(value) > self.remaining_render_work_bytes {
            self.render_work_budget_exhausted = true;
            "unknown".to_string()
        } else {
            serde_json::to_string(value).unwrap_or_else(|_| "unknown".to_string())
        }
    }

    fn consume_render_work(&mut self, rendered_bytes: usize) -> bool {
        if rendered_bytes > self.remaining_render_work_bytes {
            self.render_work_budget_exhausted = true;
            false
        } else {
            self.remaining_render_work_bytes -= rendered_bytes;
            true
        }
    }

    fn push_render_line(&mut self, lines: &mut Vec<String>, line: String) -> bool {
        if !self.consume_render_work(line.len()) {
            return false;
        }
        lines.push(line);
        true
    }
}

fn local_json_pointer(reference: &str) -> Option<String> {
    let fragment = reference.strip_prefix('#')?;
    let pointer = percent_decode_uri_fragment(fragment)?;
    if pointer.is_empty() || pointer.starts_with('/') {
        Some(pointer)
    } else {
        None
    }
}

fn percent_decode_uri_fragment(fragment: &str) -> Option<String> {
    let bytes = fragment.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = decode_hex_digit(*bytes.get(index + 1)?)?;
            let low = decode_hex_digit(*bytes.get(index + 2)?)?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn decode_hex_digit(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
    }
}

fn has_renderable_schema_keywords(map: &serde_json::Map<String, JsonValue>) -> bool {
    [
        "const",
        "enum",
        "anyOf",
        "oneOf",
        "allOf",
        "type",
        "properties",
        "additionalProperties",
        "required",
        "items",
        "prefixItems",
    ]
    .iter()
    .any(|key| map.contains_key(*key))
}

fn has_property_description(value: &JsonValue) -> bool {
    value
        .get("description")
        .and_then(JsonValue::as_str)
        .is_some_and(|description| !description.is_empty())
}

fn render_json_schema_property_name(name: &str) -> String {
    if !name.is_empty()
        && name.chars().enumerate().all(|(index, ch)| {
            ch == '_' || ch == '$' || ch.is_ascii_alphabetic() || (index > 0 && ch.is_ascii_digit())
        })
    {
        name.to_string()
    } else {
        serde_json::to_string(name).unwrap_or_else(|_| format!("\"{}\"", name.replace('"', "\\\"")))
    }
}

fn json_literal_serialization_upper_bound(value: &JsonValue) -> usize {
    match value {
        JsonValue::Null => 4,
        JsonValue::Bool(false) => 5,
        JsonValue::Bool(true) => 4,
        JsonValue::Number(number) => number.to_string().len(),
        // JSON escaping can expand one UTF-8 byte to at most one six-byte
        // Unicode escape, so this bounds allocation before serialization.
        JsonValue::String(string) => string.len().saturating_mul(6).saturating_add(2),
        JsonValue::Array(values) => values.iter().fold(2, |size, value| {
            size.saturating_add(1)
                .saturating_add(json_literal_serialization_upper_bound(value))
        }),
        JsonValue::Object(map) => map.iter().fold(2, |size, (key, value)| {
            size.saturating_add(4)
                .saturating_add(key.len().saturating_mul(6))
                .saturating_add(json_literal_serialization_upper_bound(value))
        }),
    }
}

// Saturating, short-circuiting input bound; the schema itself is borrowed, never copied
// beyond this cap. A depth limit also bounds literal serialization and ref-sibling clones.
fn schema_size(value: &JsonValue, depth: usize) -> usize {
    if depth >= 64 {
        return 128_001;
    }
    let mut size: usize = 8;
    match value {
        JsonValue::String(text) => size = size.saturating_add(text.len()),
        JsonValue::Array(values) => {
            for value in values {
                size = size.saturating_add(schema_size(value, depth + 1));
                if size > 128_000 {
                    break;
                }
            }
        }
        JsonValue::Object(map) => {
            for (key, value) in map {
                size = size
                    .saturating_add(key.len())
                    .saturating_add(schema_size(value, depth + 1));
                if size > 128_000 {
                    break;
                }
            }
        }
        _ => {}
    }
    size
}
