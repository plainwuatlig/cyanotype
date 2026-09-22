//! Seam under test: §3's "capture ... request and response bodies", observed
//! the way a consumer observes it — an agent reading the emitted document has
//! to be able to construct a call, which means a `requestBody` with a schema.
//!
//! Both tests share this binary's process-global store, so neither asserts an
//! absolute sample count; only `required`'s rule depends on N, and it holds
//! under either run order.

mod support;

#[tokio::test]
async fn a_posted_json_body_becomes_a_request_body_with_an_inferred_schema() {
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
    let schema = &doc["paths"]["/users"]["post"]["requestBody"]["content"]["application/json"]
        ["schema"];
    assert_eq!(
        schema["properties"]["name"]["type"], "string",
        "the posted body's shape must be inferable from the document alone: {doc}"
    );
    assert!(
        doc["paths"]["/users"]["post"]["requestBody"]["x-cyanotype-samples"]
            .as_u64()
            .is_some_and(|n| n >= 1),
        "the request body must expose its own sample count like every other \
         inferred assertion does"
    );
}

#[tokio::test]
async fn a_request_body_observed_more_than_once_is_asserted_required() {
    let app = support::build();
    for name in ["Grace", "Ada"] {
        support::send(
            app.clone(),
            "POST",
            "/users",
            &[],
            Some(serde_json::json!({ "name": name })),
        )
        .await;
    }

    let doc = support::emit();
    assert_eq!(
        doc["paths"]["/users"]["post"]["requestBody"]["required"],
        serde_json::json!(true),
        "every observed POST carried a body, and N >= 2, so it is required"
    );
}

#[tokio::test]
async fn a_request_with_no_body_gets_no_request_body() {
    let app = support::build();
    support::send(app, "GET", "/health", &[], None).await;

    let doc = support::emit();
    assert!(
        doc["paths"]["/health"]["get"].get("requestBody").is_none(),
        "a bodiless request must not grow a requestBody: {}",
        doc["paths"]["/health"]["get"]
    );
}
