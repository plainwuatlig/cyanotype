//! The negative half of the §4.3/§4.6 warning contract, in a test binary of
//! its own: the recorder's store and its warning list are process-global, so
//! "a clean run carries no warnings" can only be asserted in a process where
//! nothing else has recorded anything.

mod support;

#[tokio::test]
async fn a_clean_run_carries_no_warnings_key_at_all() {
    let app = support::build();
    // Twice, so the operation clears §4.3's threshold and nothing else warns.
    support::send(app.clone(), "GET", "/health", &[], None).await;
    support::send(app, "GET", "/health", &[], None).await;

    let doc = support::emit();
    assert!(
        doc["info"].get("x-cyanotype-warnings").is_none(),
        "absence of the key is what a clean run looks like: {doc}"
    );
}
