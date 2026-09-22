//! §4.3: schema inference from recorded samples, and nothing else.
//!
//! Three rules, straight from the spec, are load-bearing here and each has a
//! test below naming the rule:
//! - `required` only at N ≥ 2, only when a field appeared in *every* sample
//!   (rspec-openapi's union-properties/intersect-required algorithm).
//! - `enum` is never inferred, at any N — observed values become `examples`.
//! - `nullable` only on positive evidence (an observed `null`), modeled the
//!   JSON-Schema-2020-12 way (`"type": [T, "null"]`), since OpenAPI 3.1 has
//!   no separate `nullable` keyword.

use serde_json::{Value, json};

const MAX_EXAMPLES: usize = 10;

/// §4.3's `required` threshold: a field is only ever asserted as required at
/// N >= this, and only when it appeared in *every* sample. Shared with
/// `openapi.rs`, which warns about the operations that fall short of it.
pub(crate) const MIN_SAMPLES_FOR_REQUIRED: usize = 2;

/// Infer a JSON Schema fragment describing every sample in `samples`.
/// `samples` is every observed body for one (route, method, status) —
/// `x-cyanotype-samples` is `samples.len()` and is attached by the caller,
/// not here, since it belongs on the response object, not the schema.
pub(crate) fn infer_schema(samples: &[Value]) -> Value {
    let nullable = samples.iter().any(Value::is_null);
    let non_null: Vec<&Value> = samples.iter().filter(|v| !v.is_null()).collect();

    if non_null.is_empty() {
        // Every sample was `null` (or there were no samples at all): the
        // schema already says everything there is to say, so return early
        // rather than let the `nullable` step below double up the `null`.
        return json!({ "type": "null" });
    }

    let mut schema = if non_null.iter().all(|v| v.is_object()) {
        infer_object(&non_null)
    } else if non_null.iter().all(|v| v.is_array()) {
        infer_array(&non_null)
    } else if let Some(prim_type) = common_primitive_type(&non_null) {
        infer_primitive(&non_null, prim_type)
    } else {
        // Mixed, irreconcilable shapes: no `type` constraint, but still
        // surface what was actually seen.
        let mut s = json!({});
        add_examples(&mut s, &non_null);
        s
    };

    if nullable {
        make_nullable(&mut schema);
    }

    schema
}

fn infer_object(objects: &[&Value]) -> Value {
    let n = objects.len();

    let mut keys: Vec<&str> = Vec::new();
    for obj in objects {
        if let Some(map) = obj.as_object() {
            for key in map.keys() {
                if !keys.contains(&key.as_str()) {
                    keys.push(key.as_str());
                }
            }
        }
    }

    let mut properties = serde_json::Map::new();
    let mut required: Vec<String> = Vec::new();

    for key in &keys {
        let mut present_count = 0usize;
        let mut values: Vec<Value> = Vec::new();
        for obj in objects {
            if let Some(v) = obj.as_object().and_then(|m| m.get(*key)) {
                present_count += 1;
                values.push(v.clone());
            }
        }
        if n >= MIN_SAMPLES_FOR_REQUIRED && present_count == n {
            required.push((*key).to_string());
        }
        properties.insert((*key).to_string(), infer_schema(&values));
    }

    let mut schema = json!({
        "type": "object",
        "properties": properties,
    });
    if !required.is_empty() {
        schema["required"] = Value::Array(required.into_iter().map(Value::String).collect());
    }
    schema
}

fn infer_array(arrays: &[&Value]) -> Value {
    let mut elements: Vec<Value> = Vec::new();
    for arr in arrays {
        if let Some(items) = arr.as_array() {
            elements.extend(items.iter().cloned());
        }
    }
    let items_schema = infer_schema(&elements);
    json!({ "type": "array", "items": items_schema })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PrimitiveType {
    String,
    Boolean,
    Integer,
    Number,
}

fn common_primitive_type(values: &[&Value]) -> Option<PrimitiveType> {
    let mut kind = None;
    for v in values {
        let this = match v {
            Value::String(_) => PrimitiveType::String,
            Value::Bool(_) => PrimitiveType::Boolean,
            Value::Number(n) => {
                if n.is_i64() || n.is_u64() {
                    PrimitiveType::Integer
                } else {
                    PrimitiveType::Number
                }
            }
            _ => return None,
        };
        match kind {
            None => kind = Some(this),
            // integer and number widen to number; anything else disagreeing is not a common type.
            Some(PrimitiveType::Integer) if this == PrimitiveType::Number => {
                kind = Some(PrimitiveType::Number);
            }
            Some(PrimitiveType::Number) if this == PrimitiveType::Integer => {}
            Some(existing) if existing == this => {}
            _ => return None,
        }
    }
    kind
}

fn infer_primitive(values: &[&Value], kind: PrimitiveType) -> Value {
    let type_name = match kind {
        PrimitiveType::String => "string",
        PrimitiveType::Boolean => "boolean",
        PrimitiveType::Integer => "integer",
        PrimitiveType::Number => "number",
    };
    let mut schema = json!({ "type": type_name });
    add_examples(&mut schema, values);
    schema
}

fn add_examples(schema: &mut Value, values: &[&Value]) {
    let mut examples: Vec<Value> = Vec::new();
    for v in values {
        let v = (*v).clone();
        if !examples.contains(&v) {
            examples.push(v);
        }
        if examples.len() >= MAX_EXAMPLES {
            break;
        }
    }
    if !examples.is_empty() {
        schema["examples"] = Value::Array(examples);
    }
}

fn make_nullable(schema: &mut Value) {
    if let Some(existing) = schema.get("type").cloned() {
        let types = match existing {
            Value::String(s) => vec![Value::String(s), Value::String("null".to_string())],
            Value::Array(mut arr) => {
                if !arr.iter().any(|t| t == "null") {
                    arr.push(Value::String("null".to_string()));
                }
                arr
            }
            other => vec![other, Value::String("null".to_string())],
        };
        schema["type"] = Value::Array(types);
    } else {
        // No `type` constraint to widen (e.g. the irreconcilable-mixed
        // case, or the whole-body-was-only-null case already handled by
        // the `"type": "null"` early return in `infer_schema`).
        schema["type"] = json!(["null"]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_is_not_asserted_at_n_equals_one() {
        let schema = infer_schema(&[json!({"a": 1})]);
        assert!(
            schema.get("required").is_none(),
            "N=1 must never assert required, got {schema}"
        );
    }

    #[test]
    fn required_is_asserted_at_n_equals_two_when_field_is_in_every_sample() {
        let schema = infer_schema(&[json!({"a": 1, "b": "x"}), json!({"a": 2, "b": "y"})]);
        let required: Vec<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(required, vec!["a", "b"]);
    }

    #[test]
    fn a_field_missing_from_even_one_sample_is_not_required() {
        let schema = infer_schema(&[json!({"a": 1, "b": "x"}), json!({"a": 2})]);
        let required: Vec<&str> = schema["required"]
            .as_array()
            .map(|arr| arr.iter().map(|v| v.as_str().unwrap()).collect())
            .unwrap_or_default();
        assert_eq!(
            required,
            vec!["a"],
            "`b` was absent from one sample and must not be required"
        );
    }

    #[test]
    fn enum_is_never_inferred_observed_values_become_examples_instead() {
        let schema = infer_schema(&[
            json!({"status": "pending"}),
            json!({"status": "pending"}),
            json!({"status": "shipped"}),
        ]);
        assert!(
            schema["properties"]["status"].get("enum").is_none(),
            "inference must never produce `enum`"
        );
        let examples = schema["properties"]["status"]["examples"]
            .as_array()
            .unwrap();
        assert!(examples.contains(&json!("pending")));
        assert!(examples.contains(&json!("shipped")));
    }

    #[test]
    fn nullable_only_on_positive_evidence() {
        let never_null = infer_schema(&[json!({"a": 1}), json!({"a": 2})]);
        assert_eq!(never_null["properties"]["a"]["type"], json!("integer"));

        let seen_null = infer_schema(&[json!({"a": 1}), json!({"a": null})]);
        assert_eq!(never_null["properties"]["a"]["type"], json!("integer"));
        assert_eq!(
            seen_null["properties"]["a"]["type"],
            json!(["integer", "null"])
        );
    }

    #[test]
    fn nullable_is_never_inferred_from_mere_absence() {
        // `b` is simply missing from the second sample, never observed as
        // `null` — that must not by itself make it nullable.
        let schema = infer_schema(&[json!({"a": 1, "b": 2}), json!({"a": 2})]);
        assert_eq!(schema["properties"]["b"]["type"], json!("integer"));
    }

    #[test]
    fn array_items_are_inferred_from_the_union_of_all_elements() {
        let schema = infer_schema(&[json!([1, 2]), json!([3])]);
        assert_eq!(schema["type"], json!("array"));
        assert_eq!(schema["items"]["type"], json!("integer"));
    }

    #[test]
    fn a_field_that_is_always_null_does_not_double_up_the_null_type() {
        let schema = infer_schema(&[json!({"a": null}), json!({"a": null})]);
        assert_eq!(schema["properties"]["a"], json!({ "type": "null" }));
    }

    #[test]
    fn integers_and_floats_widen_to_number() {
        let schema = infer_schema(&[json!(1), json!(1.5)]);
        assert_eq!(schema["type"], json!("number"));
    }
}
