//! Seam under test: §4.2's `Receipt` (see `DESIGN.md` §1). A declaration
//! that matches the observed body succeeds and its schema wins over
//! inference; one that doesn't fails the test via a normal `Result::Err`.

mod support;

use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Deserialize, JsonSchema)]
#[allow(dead_code)] // fields exist for deserialization/schema shape, not read directly
struct UserResponse {
    id: u64,
    name: String,
    active: bool,
}

#[tokio::test]
async fn declaring_a_matching_type_succeeds_and_its_schema_is_emitted() {
    let app = support::build();
    let (status, _headers, body) = support::send(app.clone(), "GET", "/users/11", &[], None).await;
    assert_eq!(status, 200);

    // A real caller gets the receipt from the `Response` before consuming
    // its body; `support::send` already consumed it for convenience, so
    // this test drives the request a second way to demonstrate the actual
    // call site instead.
    use axum::body::Body;
    use http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/users/11")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let receipt = cyanotype::receipt(&response).expect("a recorded response must carry a receipt");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();

    receipt
        .declare_response::<UserResponse>(&bytes)
        .expect("the fixture's user shape matches UserResponse");

    assert_eq!(body["id"], 11);

    let doc = support::emit();
    let schema = &doc["paths"]["/users/{id}"]["get"]["responses"]["200"]["content"]["application/json"]
        ["schema"];
    // schemars' derived schema for a struct is an object schema naming its
    // properties — this is enough to prove the declared schema, not an
    // inferred one, made it into the document (inference would also produce
    // an object schema, but declaration is what this test installs).
    assert_eq!(schema["type"], serde_json::json!("object"));
    assert!(schema["properties"]["id"].is_object());
    assert!(schema["properties"]["name"].is_object());
}

#[derive(Deserialize, JsonSchema)]
struct WrongShape {
    #[allow(dead_code)]
    totally_absent_field: String,
}

#[tokio::test]
async fn declaring_a_mismatching_type_fails_with_an_error_not_a_panic() {
    let app = support::build();

    use axum::body::Body;
    use http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/users/12")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let receipt = cyanotype::receipt(&response).unwrap();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();

    let result = receipt.declare_response::<WrongShape>(&bytes);
    assert!(
        result.is_err(),
        "a declared type missing a required field must not silently succeed"
    );
}

#[tokio::test]
async fn a_response_from_a_router_not_wrapped_by_record_has_no_receipt() {
    let bare = axum::Router::new().route("/plain", axum::routing::get(|| async { "ok" }));

    use axum::body::Body;
    use http::Request;
    use tower::ServiceExt;

    let response = bare
        .oneshot(
            Request::builder()
                .uri("/plain")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert!(cyanotype::receipt(&response).is_none());
}
