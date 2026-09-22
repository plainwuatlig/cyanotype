//! §5's acceptance measurement: adoption cost in lines actually changed.
//!
//! The earlier version of this file measured by inspection, because the task
//! forbade modifying `uniar-api` while §5 asked for the recorder to be run
//! against it. That tension is gone: the recorder **has** been run against
//! `uniar-api`'s real suite, on the unpushed branch `experiment/cyanotype`
//! in that repo (117 passed, 1 pre-existing failure, 45 operations over 38
//! paths). What follows is the change that run actually made, not the change
//! it was predicted to make.
//!
//! `uniar-api/src/app.rs` before (read 2026-09-22):
//!
//! ```text
//! pub fn build(cfg: Config, pools: Pools) -> Router {
//!     routes::router(cfg, pools)
//! }
//! ```
//!
//! The prediction was `cyanotype::record(routes::router(cfg, pools))` — one
//! line replaced, same line count. It does not survive contact with the repo:
//! `build()` is production code, called inline by ~40 `#[tokio::test]`
//! functions (70 across the crate, 30 of which never touch HTTP) with no
//! test-only helper to wrap, so an unconditional `record()` would put the
//! recorder in the production build. It needs a `#[cfg(test)]` gate, which
//! costs three more lines:
//!
//! ```text
//! pub fn build(cfg: Config, pools: Pools) -> Router {
//!     let router = routes::router(cfg, pools);
//!     #[cfg(test)]
//!     let router = cyanotype::record(router);
//!     router
//! }
//! ```
//!
//! The earlier one-line figure was measured against `ls-api-rs`, whose test
//! helper has a different shape and *can* take the wrapper directly. Both
//! numbers are honest; only one of them is about the repo §5 chose.
//!
//! The 58 `.oneshot(...)` call sites are untouched either way, because they
//! all route through this one function — which is what §3's "one line in a
//! test helper" budget rests on.
//!
//! One number in the spec's own framing didn't hold up under a direct count:
//! `src/http/routes.rs` has 76 `.route(...)` registrations, not the "35
//! routes" §5 cites (ticket provenance for that figure wasn't re-read, per
//! the instruction to trust the spec over the tickets) — some of that gap is
//! routes registered more than once under different API version prefixes
//! (`/api/v1/...` and `/api/v2/...` for the same handler) and method-only
//! variations, but 76 vs. 35 is more than that alone plausibly explains.
//! Reported here rather than quietly adopted.

/// The recording half of the change, reproduced verbatim from what the proof
/// branch actually contains (not a paraphrase).
const BEFORE: &str = "\
pub fn build(cfg: Config, pools: Pools) -> Router {
    routes::router(cfg, pools)
}";

const AFTER: &str = "\
pub fn build(cfg: Config, pools: Pools) -> Router {
    let router = routes::router(cfg, pools);
    #[cfg(test)]
    let router = cyanotype::record(router);
    router
}";

/// §4.6's declaration is *additional* setup, not part of the recording cost —
/// but it is not optional in a service that returns a token, and the proof run
/// needed it: the recorder's own warning named `response POST
/// /vue-api/v1/login data.token`, and the author's half of §4.6 is the
/// decision. Repos with nothing to redact stop at [`AFTER`].
const AFTER_WITH_DECLARED_REDACTION: &str = "\
pub fn build(cfg: Config, pools: Pools) -> Router {
    let router = routes::router(cfg, pools);
    #[cfg(test)]
    let router = {
        static REDACTIONS: std::sync::Once = std::sync::Once::new();
        REDACTIONS.call_once(|| {
            cyanotype::configure(cyanotype::Redactions::new().field(\"token\"));
        });
        cyanotype::record(router)
    };
    router
}";

fn lines(s: &str) -> Vec<&str> {
    s.lines().collect()
}

/// Lines present in `after` that were not present in `before` — the additions.
fn added<'a>(before: &[&'a str], after: &[&'a str]) -> Vec<&'a str> {
    after
        .iter()
        .filter(|line| !before.contains(line))
        .copied()
        .collect()
}

/// Lines present in `before` that are gone from `after` — the removals.
fn removed<'a>(before: &[&'a str], after: &[&'a str]) -> Vec<&'a str> {
    before
        .iter()
        .filter(|line| !after.contains(line))
        .copied()
        .collect()
}

#[test]
fn recording_costs_one_line_changed_and_three_added() {
    let before = lines(BEFORE);
    let after = lines(AFTER);

    assert_eq!(
        added(&before, &after),
        vec![
            "    let router = routes::router(cfg, pools);",
            "    #[cfg(test)]",
            "    let router = cyanotype::record(router);",
            "    router",
        ],
        "one line is rewritten in place, and the `#[cfg(test)]` gate, the wrapper \
         and the rebound return are added on top of it"
    );
    assert_eq!(
        removed(&before, &after),
        vec!["    routes::router(cfg, pools)"],
        "exactly one line is replaced: the router construction"
    );
    assert_eq!(
        after.len() - before.len(),
        3,
        "four additions minus one replacement is a net +3, not the predicted +0"
    );
}

#[test]
fn declaring_a_redacted_field_is_seven_more_lines_and_is_not_optional_here() {
    let recording = lines(AFTER);
    let with_redaction = lines(AFTER_WITH_DECLARED_REDACTION);

    assert_eq!(
        added(&recording, &with_redaction).len(),
        7,
        "the §4.6 declaration block is six lines, plus the plain `record` line \
         restructured into it"
    );
    assert_eq!(
        removed(&recording, &with_redaction),
        vec!["    let router = cyanotype::record(router);"],
        "declaring a redaction subsumes the plain `record` call; nothing else moves"
    );
    assert_eq!(
        with_redaction.len() - recording.len(),
        6,
        "net +6 lines for the declaration block"
    );
}

/// The cost §3's "one line" claim explicitly does *not* cover, made concrete:
/// getting the document out at the end.
///
/// - `Cargo.toml`: **+1 line** — `cyanotype` under `[dev-dependencies]`. Never
///   reaches the production build, which is the crate's one hard constraint on
///   this cost.
/// - Emission: **not one line, and not in the test helper.** Rust has no
///   built-in "after all tests in this binary finished" hook, and §2 is
///   explicit that writing is never bound to test execution, so emission
///   cannot be a framework callback. The recipe that works is a `#[test]` at
///   the *crate root* named to sort after every module, run under `cargo test
///   -- --test-threads=1`. The proof run used `app::tests::zzz_emit_openapi`
///   first and it ran at position 65 of 118 — libtest sorts by full test path,
///   so a name inside a module is not last in the binary. Measured impact in
///   that suite: none (trapped and untrapped runs emitted the same document),
///   which is exactly why it survived a full build and exactly why it is still
///   a trap. See README, "Where to emit from".
#[test]
fn emission_cost_is_named_here_because_the_one_line_claim_does_not_include_it() {
    // No assertion beyond compiling: this test's only job is to be the place
    // this measurement's honesty check lives, so it cannot silently rot out of
    // the doc comment above.
}
