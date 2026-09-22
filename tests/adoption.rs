//! §5's acceptance measurement: adoption cost in lines actually changed.
//!
//! What this file can and can't do, stated plainly: the task instructs
//! reading `uniar-api` to understand its test-helper shape but **not
//! modifying that repo**, and separately §5 of the spec wants "`uniar-api`'s
//! existing tests run with the recorder installed, changing one line in its
//! test helper" — literally run against the real repo. Those two
//! instructions conflict; per the delegating instructions, the resolution
//! is to build an equivalent fixture here and report the real repo's numbers
//! by inspection instead of by execution. That's what this file does, and
//! it's the reason this measurement is partly "read the real file and count"
//! rather than entirely "run it and see."
//!
//! `uniar-api/src/app.rs` (read 2026-09-22, not modified) contains exactly
//! this as its test helper's router construction:
//!
//! ```text
//! pub fn build(cfg: Config, pools: Pools) -> Router {
//!     routes::router(cfg, pools)
//! }
//! ```
//!
//! `uniar-api::app::tests` calls `build(cfg, pools)` fresh in every one of
//! its 40 `#[tokio::test]` functions (measured: `grep -c '#\[tokio::test\]'
//! src/app.rs`; 70 total across the whole crate, the other 30 not exercising
//! HTTP at all) rather than sharing one instance, and 58 of those calls
//! reach `.oneshot(...)` (measured: `grep -c '\.oneshot(' src/app.rs`,
//! matching the spec's own "~60" almost exactly). Wrapping the single
//! `build()` definition — not each call site — is what the spec's "one line
//! in a test helper" claim rests on. The test below reproduces that exact
//! before/after and proves it mechanically: exactly one line changed, none
//! added, none removed.
//!
//! One number in the spec's own framing didn't hold up under a direct
//! count: `src/http/routes.rs` has 76 `.route(...)` registrations, not the
//! "35 routes" §5 cites (ticket provenance for that figure wasn't re-read,
//! per the instruction to trust the spec over the tickets) — some of that
//! gap is routes registered more than once under different API version
//! prefixes (`/api/v1/...` and `/api/v2/...` for the same handler) and
//! method-only variations, but 76 vs. 35 is more than that alone plausibly
//! explains. Reported here rather than quietly adopted, since the acceptance
//! section is asking for measured numbers, not repeated ones.

#[test]
fn wrapping_the_shared_build_helper_changes_exactly_one_line() {
    // Reproduced verbatim from `uniar-api/src/app.rs` lines 6-8, read
    // 2026-09-22. Not a paraphrase: this is the literal text of the
    // function every one of that file's `#[tokio::test]`s calls.
    let before = "\
pub fn build(cfg: Config, pools: Pools) -> Router {
    routes::router(cfg, pools)
}";

    // The only change the spec's "one line" claim requires: wrap the
    // returned router. Every call site (all ~70 tests) is untouched,
    // because they all go through this one function.
    let after = "\
pub fn build(cfg: Config, pools: Pools) -> Router {
    cyanotype::record(routes::router(cfg, pools))
}";

    let before_lines: Vec<&str> = before.lines().collect();
    let after_lines: Vec<&str> = after.lines().collect();

    assert_eq!(
        before_lines.len(),
        after_lines.len(),
        "the change must not add or remove any lines"
    );

    let changed: Vec<usize> = before_lines
        .iter()
        .zip(after_lines.iter())
        .enumerate()
        .filter_map(|(i, (b, a))| (b != a).then_some(i))
        .collect();

    assert_eq!(
        changed,
        vec![1],
        "exactly one line (the router-construction line) should differ"
    );
}

/// The cost the spec's "one line" claim explicitly does *not* cover, made
/// concrete rather than left as a footnote: getting the recorder into the
/// build in the first place, and getting the document out at the end.
///
/// - `Cargo.toml`: **+1 line** — `cyanotype` added under `[dev-dependencies]`.
///   Never reaches `uniar-api`'s production build (it isn't a dependency,
///   only a dev-dependency), which is the crate's one hard constraint on
///   this cost.
/// - Emission: **not one line, and not in the test helper** — `write_openapi`
///   has to run *after* the test binary's tests have all executed, and Rust
///   has no built-in "after all tests" hook (see `DESIGN.md` — this exact
///   gap is called out there as a known limitation, not solved by this
///   crate). The realistic options, in increasing order of the caller's own
///   effort: a `#[test]` at the end of the file relying on `cargo test
///   --test-threads=1` for ordering (fragile, ~3 lines, no new files); a
///   tiny separate `#[test]`-free binary/xtask that links the lib and calls
///   `cyanotype::collected().write_openapi(...)` after `cargo test` exits in
///   CI (robust, but a new file plus a CI step, not "one line" by any
///   count). Neither is a `cyanotype` limitation to fix — §2 of the spec is
///   explicit that writing is never bound to test execution — but reporting
///   the "one line" number without this would be reporting only the easy
///   half.
#[test]
fn emission_cost_is_named_here_because_the_one_line_claim_does_not_include_it() {
    // No assertion beyond compiling: this test's only job is to be the
    // place this measurement's honesty check lives, so it can't silently
    // rot out of the doc comment above.
}
