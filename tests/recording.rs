//! Seam under test: `cyanotype::record(router)` -> drive requests with
//! `oneshot` (mirroring `uniar-api`'s own test helper) -> `collected().write_openapi()`
//! -> the emitted `openapi.json`. This is the seam a real caller uses, so
//! it's the one these tests assert against, rather than any internal type.

mod support;

#[tokio::test]
async fn records_a_matched_route_template_method_and_status() {
    let app = support::build();
    let (status, _headers, body) = support::send(app, "GET", "/users/501", &[], None).await;
    assert_eq!(status, 200);
    assert_eq!(body["id"], 501);

    let doc = support::emit();
    let response = &doc["paths"]["/users/{id}"]["get"]["responses"]["200"];
    assert!(
        response.is_object(),
        "expected a recorded 200 response for GET /users/{{id}}, got document: {doc}"
    );
}

#[tokio::test]
async fn records_the_concrete_path_as_the_route_template_not_the_literal_url() {
    let app = support::build();
    support::send(app, "GET", "/users/999999", &[], None).await;

    let doc = support::emit();
    // The literal concrete path must never appear as a path key.
    assert!(doc["paths"].get("/users/999999").is_none());
    assert!(doc["paths"].get("/users/{id}").is_some());
}

#[tokio::test]
async fn different_statuses_from_the_same_route_get_separate_entries() {
    let app = support::build();
    let (found_status, ..) = support::send(app.clone(), "GET", "/users/7", &[], None).await;
    let (missing_status, ..) = support::send(app, "GET", "/users/0", &[], None).await;
    assert_eq!(found_status, 200);
    assert_eq!(missing_status, 404);

    let doc = support::emit();
    let responses = doc["paths"]["/users/{id}"]["get"]["responses"]
        .as_object()
        .unwrap();
    assert!(responses.contains_key("200"));
    assert!(responses.contains_key("404"));
}

#[tokio::test]
async fn a_post_with_a_json_body_is_recorded_under_its_own_method() {
    let app = support::build();
    let (status, ..) = support::send(
        app,
        "POST",
        "/users",
        &[],
        Some(serde_json::json!({ "name": "Grace" })),
    )
    .await;
    assert_eq!(status, 201);

    let doc = support::emit();
    assert!(doc["paths"]["/users"]["post"]["responses"]["201"].is_object());
    // GET and POST on routes that share a path must not clobber each other.
    assert!(doc["paths"]["/users"].get("get").is_none());
}

#[tokio::test]
async fn non_json_response_bodies_keep_only_content_type_never_the_bytes() {
    let app = support::build();
    let (status, ..) = support::send(app, "GET", "/under-construction", &[], None).await;
    assert_eq!(status, 200);

    let doc = support::emit();
    let response = &doc["paths"]["/under-construction"]["get"]["responses"]["200"];
    // No `content` block at all for a body we deliberately never embed.
    assert!(
        response.get("content").is_none(),
        "non-JSON body must not be embedded in the document: {response}"
    );
}

#[tokio::test]
async fn sample_count_is_always_emitted() {
    let app = support::build();
    support::send(app.clone(), "GET", "/health", &[], None).await;
    support::send(app.clone(), "GET", "/health", &[], None).await;
    support::send(app, "GET", "/health", &[], None).await;

    let doc = support::emit();
    let response = &doc["paths"]["/health"]["get"]["responses"]["200"];
    let samples = response["x-cyanotype-samples"].as_u64().unwrap();
    assert!(samples >= 3, "expected at least 3 samples, got {samples}");
}

#[tokio::test]
async fn generator_version_is_always_stamped() {
    let app = support::build();
    support::send(app, "GET", "/health", &[], None).await;
    let doc = support::emit();
    let stamp = doc["info"]["x-cyanotype-generator"].as_str().unwrap();
    assert!(stamp.starts_with("cyanotype/"));
}

#[tokio::test]
async fn document_is_minified_not_pretty_printed() {
    let app = support::build();
    support::send(app, "GET", "/health", &[], None).await;

    let dir = std::env::temp_dir();
    let path = dir.join("cyanotype-minified-check.json");
    cyanotype::collected().write_openapi(path.clone()).unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    let _ = std::fs::remove_file(&path);

    assert!(!raw.contains('\n'), "minified output must be a single line");
}
