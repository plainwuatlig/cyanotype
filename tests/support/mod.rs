//! A fixture axum app, wired the way `uniar-api`'s tests wire theirs: one
//! `build()` function returning a `Router` that a test drives with
//! `tower::ServiceExt::oneshot`. See `uniar-api/src/app.rs` (read, not
//! modified, per the task) for the pattern this mirrors.
//!
//! `cyanotype::record()` wraps the router inside `build()` — this is the
//! "one line in a test helper" the spec's adoption claim is measured
//! against; `tests/adoption.rs` does that measurement.

#![allow(dead_code)] // not every test file uses every route.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

#[derive(Clone, Default)]
pub struct AppState {
    next_id: Arc<AtomicI64>,
}

pub fn build() -> Router {
    let state = AppState::default();
    let router = Router::new()
        .route("/health", get(health))
        .route("/users/:id", get(get_user))
        .route("/users", post(create_user))
        .route("/whoami", get(whoami))
        .route("/whoami-bearer", get(whoami))
        .route("/whoami-apikey", get(whoami))
        .route("/whoami-none", get(whoami))
        .route("/under-construction", get(under_construction))
        .route("/big", get(big_body))
        .with_state(state);
    cyanotype::record(router)
}

async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

/// `id == 0` -> 404, otherwise a user object. `nickname` is only present for
/// odd ids, so N>=2 samples across both give inference something real to
/// reconcile (§4.3's required/nullable rules).
async fn get_user(Path(id): Path<u64>) -> impl IntoResponse {
    if id == 0 {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response();
    }
    let mut body = json!({ "id": id, "name": "Ada", "active": true, "note": null });
    if id % 2 == 1 {
        body["nickname"] = json!("A");
    }
    Json(body).into_response()
}

async fn create_user(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let id = state.next_id.fetch_add(1, Ordering::SeqCst) + 1;
    (
        StatusCode::CREATED,
        Json(json!({ "id": id, "name": body.get("name").cloned().unwrap_or(json!(null)) })),
    )
}

/// Echoes back whether an `Authorization` header was sent and, if so, its
/// derived kind — used to exercise §4.5 without ever asserting on the raw
/// header value itself.
async fn whoami(headers: HeaderMap) -> impl IntoResponse {
    let auth = headers.get("authorization").and_then(|v| v.to_str().ok());
    let kind = match auth {
        Some(v) if v.starts_with("Bearer ") => "bearer",
        Some(_) => "api-key",
        None => "anonymous",
    };
    Json(json!({ "auth_kind": kind }))
}

async fn under_construction() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "text/html")],
        "<h1>under construction</h1>",
    )
}

/// A JSON body comfortably past `cyanotype`'s 64 KiB capture cap, to exercise
/// the truncation path end-to-end rather than only at the `TeeBody` unit
/// level.
async fn big_body() -> impl IntoResponse {
    let filler = "x".repeat(80 * 1024);
    Json(json!({ "filler": filler }))
}

/// Drive one request through `app` via `oneshot`, the same way
/// `uniar-api`'s own test helper does, and collect the response.
pub async fn send(
    app: Router,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    json_body: Option<serde_json::Value>,
) -> (u16, http::HeaderMap, serde_json::Value) {
    use axum::body::Body;
    use http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    let mut builder = Request::builder().method(method).uri(path);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let body = match &json_body {
        Some(v) => {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(v).unwrap())
        }
        None => Body::empty(),
    };
    let request = builder.body(body).unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let response_headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, response_headers, value)
}

/// Write the currently-collected exchanges to a temp file and read them back
/// as parsed JSON — the same round trip a real caller's CI job does, minus
/// the pipe into docs-mcp.
pub fn emit() -> serde_json::Value {
    let dir = std::env::temp_dir();
    let path = dir.join(format!("cyanotype-test-{}.json", uuid_ish()));
    cyanotype::collected().write_openapi(path.clone()).unwrap();
    let contents = std::fs::read_to_string(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    serde_json::from_str(&contents).unwrap()
}

fn uuid_ish() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let tid = std::thread::current().id();
    format!("{nanos}-{tid:?}").replace(['(', ')', ' '], "-")
}
