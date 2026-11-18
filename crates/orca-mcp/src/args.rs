//! Tool input schemas, and tool or prompt arguments as strings.

use std::collections::BTreeMap;

use rmcp::model::JsonObject;
use serde_json::{Value, json};

use crate::catalog::{Kind, Op};

pub(crate) fn input_schema(op: &Op) -> JsonObject {
    let props: serde_json::Map<String, Value> = op
        .params
        .iter()
        .map(|p| {
            let ty = match p.kind {
                Kind::Str => "string",
                Kind::Int => "integer",
                Kind::Bool => "boolean",
            };
            (
                p.name.to_string(),
                json!({ "type": ty, "description": p.description }),
            )
        })
        .collect();
    let required: Vec<&str> = op
        .params
        .iter()
        .filter(|p| p.required)
        .map(|p| p.name)
        .collect();
    let schema = json!({ "type": "object", "properties": props, "required": required });
    schema.as_object().cloned().unwrap_or_default()
}

pub(crate) fn string_args(args: Option<&JsonObject>) -> BTreeMap<String, String> {
    args.into_iter()
        .flatten()
        .filter_map(|(k, v)| {
            let s = match v {
                Value::String(s) => s.clone(),
                Value::Null => return None,
                other => other.to_string(),
            };
            Some((k.clone(), s))
        })
        .collect()
}
