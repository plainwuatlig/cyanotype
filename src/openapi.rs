//! §4.7: assemble recorded exchanges into a minified OpenAPI 3.1 document.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use crate::exchange::{BodyCapture, Exchange};
use crate::infer::infer_schema;

const GENERATOR_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct OperationKey {
    route: String,
    method: String,
}

#[derive(Default)]
struct StatusAcc {
    /// Every response observed at this status, JSON or not — this is what
    /// `x-cyanotype-samples` reports, per §4.3's "always emitted... on every
    /// operation" (interpreted per response-status, since that's the
    /// granularity inference itself operates at).
    sample_count: u32,
    json_samples: Vec<Value>,
    declared_schema: Option<Value>,
}

#[derive(Default)]
struct OperationAcc {
    by_status: BTreeMap<u16, StatusAcc>,
    security_schemes: BTreeSet<String>,
}

/// `{name}` segments in a route template become OpenAPI `parameters`
/// entries; the recorder only ever sees that a segment was templated, not
/// its type, so every path parameter is declared as an untyped string.
fn path_parameters(route: &str) -> Vec<Value> {
    route
        .split('/')
        .filter_map(|segment| segment.strip_prefix('{').and_then(|s| s.strip_suffix('}')))
        .map(|name| {
            json!({
                "name": name,
                "in": "path",
                "required": true,
                "schema": { "type": "string" },
            })
        })
        .collect()
}

pub(crate) fn build_document(exchanges: &[Exchange]) -> Value {
    let mut operations: BTreeMap<OperationKey, OperationAcc> = BTreeMap::new();
    let mut security_scheme_defs: BTreeMap<String, Value> = BTreeMap::new();
    let mut component_schemas: Map<String, Value> = Map::new();

    for exchange in exchanges {
        let Some(status) = exchange.response.status else {
            // Response never completed (e.g. the test never drained the
            // body) — nothing to say about it yet. Per spec, a half-run is
            // the caller's diff to read, not ours to prevent.
            continue;
        };

        let key = OperationKey {
            route: exchange.route_template.clone(),
            method: exchange.method.as_str().to_ascii_lowercase(),
        };
        let acc = operations.entry(key).or_default();
        let status_acc = acc.by_status.entry(status).or_default();
        status_acc.sample_count += 1;

        if let BodyCapture::Json(value) = &exchange.response.body {
            status_acc.json_samples.push(value.clone());
        }

        if let Some((schema, definitions)) = &exchange.declared_response_schema {
            if status_acc.declared_schema.is_none() {
                status_acc.declared_schema =
                    Some(serde_json::to_value(schema).unwrap_or_else(|_| json!({})));
            }
            for (name, def) in definitions {
                component_schemas
                    .entry(name.clone())
                    .or_insert_with(|| serde_json::to_value(def).unwrap_or_else(|_| json!({})));
            }
        }

        if let Some(scheme) = &exchange.security {
            let name = scheme.component_name().to_string();
            security_scheme_defs
                .entry(name.clone())
                .or_insert_with(|| scheme.to_json());
            acc.security_schemes.insert(name);
        }
    }

    let mut paths: Map<String, Value> = Map::new();
    for (key, acc) in operations {
        let mut responses = Map::new();
        for (status, status_acc) in acc.by_status {
            // This is a generic, response-object-level description, not the
            // operation-level prose §4.4 means ("handler doc comments").
            // §4.4 is NOT implemented — see the "Descriptions (§4.4)" section
            // in DESIGN.md for why (Rust doc comments aren't reachable at
            // runtime without a companion proc-macro crate, which is out of
            // scope for v1) and README's Limitations for the disclosure.
            let mut response_obj = json!({
                "description": format!("Observed {status} response."),
                "x-cyanotype-samples": status_acc.sample_count,
            });

            let schema = status_acc.declared_schema.or_else(|| {
                (!status_acc.json_samples.is_empty())
                    .then(|| infer_schema(&status_acc.json_samples))
            });

            if let Some(schema) = schema {
                response_obj["content"] = json!({ "application/json": { "schema": schema } });
            }

            responses.insert(status.to_string(), response_obj);
        }

        let mut operation_obj = json!({ "responses": responses });
        let params = path_parameters(&key.route);
        if !params.is_empty() {
            operation_obj["parameters"] = Value::Array(params);
        }
        if !acc.security_schemes.is_empty() {
            operation_obj["security"] = Value::Array(
                acc.security_schemes
                    .iter()
                    .map(|name| json!({ name.clone(): [] }))
                    .collect(),
            );
        }

        let path_item = paths.entry(key.route.clone()).or_insert_with(|| json!({}));
        path_item
            .as_object_mut()
            .expect("path items are always objects")
            .insert(key.method.clone(), operation_obj);
    }

    let mut doc = json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Recorded API",
            "version": "0.0.0",
            "x-cyanotype-generator": format!("cyanotype/{GENERATOR_VERSION}"),
        },
        "paths": paths,
    });

    if !security_scheme_defs.is_empty() || !component_schemas.is_empty() {
        let mut components = json!({});
        if !security_scheme_defs.is_empty() {
            components["securitySchemes"] =
                Value::Object(security_scheme_defs.into_iter().collect());
        }
        if !component_schemas.is_empty() {
            components["schemas"] = Value::Object(component_schemas);
        }
        doc["components"] = components;
    }

    doc
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{SchemeKind, SecurityScheme};
    use crate::exchange::MessageCapture;
    use http::Method;

    fn exchange_with_json_response(
        route: &str,
        method: Method,
        status: u16,
        body: Value,
    ) -> Exchange {
        let mut ex = Exchange::new(method, route.to_string(), None);
        ex.response = MessageCapture {
            status: Some(status),
            headers: Vec::new(),
            body: BodyCapture::Json(body),
        };
        ex
    }

    #[test]
    fn groups_by_route_and_method_and_emits_sample_count() {
        let exchanges = vec![
            exchange_with_json_response("/users/{id}", Method::GET, 200, json!({"id": 1})),
            exchange_with_json_response("/users/{id}", Method::GET, 200, json!({"id": 2})),
        ];
        let doc = build_document(&exchanges);
        let response = &doc["paths"]["/users/{id}"]["get"]["responses"]["200"];
        assert_eq!(response["x-cyanotype-samples"], json!(2));
        assert_eq!(
            response["content"]["application/json"]["schema"]["type"],
            json!("object")
        );
    }

    #[test]
    fn different_statuses_get_separate_response_entries() {
        let exchanges = vec![
            exchange_with_json_response("/users/{id}", Method::GET, 200, json!({"id": 1})),
            exchange_with_json_response(
                "/users/{id}",
                Method::GET,
                404,
                json!({"error": "not found"}),
            ),
        ];
        let doc = build_document(&exchanges);
        let responses = doc["paths"]["/users/{id}"]["get"]["responses"]
            .as_object()
            .unwrap();
        assert!(responses.contains_key("200"));
        assert!(responses.contains_key("404"));
    }

    #[test]
    fn security_scheme_is_emitted_when_a_scheme_was_observed() {
        let mut ex =
            exchange_with_json_response("/balance", Method::GET, 200, json!({"balance": 0}));
        ex.security = Some(SecurityScheme {
            header_name: "Authorization",
            kind: SchemeKind::Bearer,
        });
        let doc = build_document(std::slice::from_ref(&ex));

        assert_eq!(
            doc["components"]["securitySchemes"]["bearerAuth"],
            json!({ "type": "http", "scheme": "bearer" })
        );
        assert_eq!(
            doc["paths"]["/balance"]["get"]["security"],
            json!([{ "bearerAuth": [] }])
        );
    }

    #[test]
    fn generator_version_is_stamped_on_info() {
        let doc = build_document(&[]);
        let stamp = doc["info"]["x-cyanotype-generator"].as_str().unwrap();
        assert!(stamp.starts_with("cyanotype/"));
    }

    #[test]
    fn incomplete_exchange_without_a_response_status_is_skipped() {
        let ex = Exchange::new(Method::GET, "/never-finished".to_string(), None);
        let doc = build_document(std::slice::from_ref(&ex));
        assert!(doc["paths"].get("/never-finished").is_none());
    }
}
