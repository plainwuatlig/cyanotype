//! Seam under test: `DESIGN.md` §2 end to end — a body past the 64 KiB
//! capture cap is (a) still delivered to the caller in full, unmodified,
//! and (b) dropped from the recorded document with a warning, rather than
//! embedded partially.

mod support;

#[tokio::test]
async fn a_body_past_the_capture_cap_still_reaches_the_caller_in_full() {
    let app = support::build();
    let (status, _headers, body) = support::send(app, "GET", "/big", &[], None).await;
    assert_eq!(status, 200);
    let filler = body["filler"].as_str().unwrap();
    assert_eq!(
        filler.len(),
        80 * 1024,
        "the full body must reach the test, uncapped"
    );
}

#[tokio::test]
async fn a_body_past_the_capture_cap_is_not_embedded_in_the_document() {
    let app = support::build();
    support::send(app, "GET", "/big", &[], None).await;

    let doc = support::emit();
    let response = &doc["paths"]["/big"]["get"]["responses"]["200"];
    assert!(
        response.get("content").is_none(),
        "a truncated JSON body must be dropped, not embedded partially: {response}"
    );
    // The sample was still observed and counted, even though its body
    // wasn't kept. (`>= 1`, not `== 1`: this file's other test also hits
    // `/big` concurrently, in the same shared process-wide recorder.)
    let samples = response["x-cyanotype-samples"].as_u64().unwrap();
    assert!(samples >= 1, "expected at least 1 sample, got {samples}");

    let warnings = cyanotype::collected().warnings();
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("exceeded") && w.contains("/big")),
        "expected a truncation warning naming the route, got: {warnings:?}"
    );
}
