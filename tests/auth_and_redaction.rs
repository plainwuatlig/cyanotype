//! Seam under test: §4.5 (auth derivation + unconditional strip) and §4.6
//! (declared redaction + undeclared high-entropy warning), observed only
//! through the emitted document and `Collected::warnings()` — never through
//! any internal type, so these tests would catch a leak even if it happened
//! somewhere we didn't think to look.

mod support;

const SECRET_TOKEN: &str = "sk-live-eyJhbGciOiJIUzI1NiJ9-do-not-leak-this-9f8a7b6c5d4e";

#[tokio::test]
async fn bearer_authorization_is_never_present_anywhere_in_the_document() {
    let app = support::build();
    support::send(
        app,
        "GET",
        "/whoami-bearer",
        &[("authorization", &format!("Bearer {SECRET_TOKEN}"))],
        None,
    )
    .await;

    let doc = support::emit();
    let serialized = doc.to_string();
    assert!(
        !serialized.contains(SECRET_TOKEN),
        "the raw token must never appear in the emitted document"
    );
}

#[tokio::test]
async fn bearer_token_derives_an_http_bearer_security_scheme() {
    let app = support::build();
    support::send(
        app,
        "GET",
        "/whoami-bearer",
        &[("authorization", &format!("Bearer {SECRET_TOKEN}"))],
        None,
    )
    .await;

    let doc = support::emit();
    assert_eq!(
        doc["components"]["securitySchemes"]["bearerAuth"],
        serde_json::json!({ "type": "http", "scheme": "bearer" })
    );
    assert_eq!(
        doc["paths"]["/whoami-bearer"]["get"]["security"],
        serde_json::json!([{ "bearerAuth": [] }])
    );
}

#[tokio::test]
async fn non_bearer_authorization_derives_an_api_key_scheme() {
    let app = support::build();
    support::send(
        app,
        "GET",
        "/whoami-apikey",
        &[("authorization", "raw-key-abc")],
        None,
    )
    .await;

    let doc = support::emit();
    assert_eq!(
        doc["components"]["securitySchemes"]["apiKeyAuth"],
        serde_json::json!({ "type": "apiKey", "in": "header", "name": "Authorization" })
    );
}

#[tokio::test]
async fn no_authorization_header_means_no_security_scheme_for_that_call() {
    let app = support::build();
    support::send(app, "GET", "/whoami-none", &[], None).await;

    let doc = support::emit();
    assert!(
        doc["paths"]["/whoami-none"]["get"]
            .get("security")
            .is_none()
    );
}

#[tokio::test]
async fn declared_redaction_replaces_the_field_everywhere_it_appears() {
    cyanotype::configure(cyanotype::Redactions::new().field("nickname"));

    let app = support::build();
    // id=3 is odd, so `get_user` sets `nickname` on the response body.
    support::send(app, "GET", "/users/3", &[], None).await;

    let doc = support::emit();
    let schema = &doc["paths"]["/users/{id}"]["get"]["responses"]["200"]["content"]["application/json"]
        ["schema"];
    let serialized = schema.to_string();
    assert!(
        !serialized.contains("\"A\""),
        "the declared field's real value must not survive into the schema: {schema}"
    );
}

#[tokio::test]
async fn undeclared_high_entropy_value_produces_a_warning_not_a_failure() {
    let app = support::build();
    // The bearer token itself is redacted by the auth strip, not this
    // mechanism — exercise it on a body field instead via `create_user`,
    // whose `name` field is a small controlled input; use a body value that
    // looks like an accidental secret instead.
    support::send(
        app,
        "POST",
        "/users",
        &[],
        Some(serde_json::json!({ "name": "aK9$mQ2x!pL7@rT4&wZ1--not-declared-anywhere" })),
    )
    .await;

    let warnings = cyanotype::collected().warnings();
    // Warn-only: the test reaching this point at all (no panic) is part of
    // the assertion. We additionally expect at least one high-entropy
    // warning to have been recorded somewhere in this process.
    assert!(
        warnings.iter().any(|w| w.contains("high-entropy")),
        "expected a high-entropy warning, got: {warnings:?}"
    );
}

const REAL_JWT: &str = "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.eyJzdWIiOjE4LCJleHAiOjE3OTc4Mzk5MzMsImlzcyI6InVuaWFyIn0.qVh0mYQ9k3sT7pR2wX5cN8dL1fG4jB6hK0aZ3eU7vI4";

/// The regression test for the real proof run against `uniar-api`: six live
/// JWTs shipped in a document whose 41 warnings went nowhere, because the
/// emission step ran inside a passing `#[tokio::test]` and libtest captures
/// (and discards) the stdout/stderr of every passing test. The warning fired
/// every time and nobody could see it. So the warning must travel *in the
/// artifact*, not only down a channel the test harness swallows.
#[tokio::test]
async fn undeclared_high_entropy_warning_appears_in_the_document_itself() {
    let app = support::build();
    support::send(app, "GET", "/session", &[], None).await;

    let doc = support::emit();
    let warnings: Vec<&str> = doc["info"]["x-cyanotype-warnings"]
        .as_array()
        .expect("a document with warnings must carry them at info → x-cyanotype-warnings")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();

    assert!(
        warnings
            .iter()
            .any(|w| w.contains("high-entropy") && w.contains("GET /session") && w.contains("token")),
        "the JWT in the response body must be named in the artifact's own warnings: {warnings:?}"
    );
    // Warn-only, never redact-on-guess (§4.6): the value is still embedded,
    // because nothing declared `token`. The warning is the whole mechanism.
    assert!(doc.to_string().contains(REAL_JWT));
}

#[tokio::test]
async fn an_operation_below_the_sample_threshold_is_named_in_the_document() {
    let app = support::build();
    support::send(app, "GET", "/health", &[], None).await;

    let doc = support::emit();
    let warnings = doc["info"]["x-cyanotype-warnings"].to_string();
    assert!(
        warnings.contains("GET /health 200") && warnings.contains("below the threshold"),
        "§4.3 requires the generator to warn about sub-threshold operations: {warnings}"
    );
}
