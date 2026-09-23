# cyanotype

Records an axum service's own integration tests and emits an OpenAPI 3.1
document describing what those tests actually exercised. A dev-dependency,
nothing more — install it in your test helper, drive your existing tests,
emit `openapi.json` whenever you choose.

The document isn't a claim of completeness. It's an impression of what the
tests actually touched — derived, not written. A contact print, not a
drawing. See `SPEC.md` for the full design specification this crate is built
against, and `DESIGN.md` for the two decisions that specification left open.

## Usage

```rust
// 1. install the recorder — one line in the test helper
let app = cyanotype::record(build_router());

// ... drive `app` through your existing tests, e.g. with
// `tower::ServiceExt::oneshot`, exactly as before ...

// 2. emit, wherever and whenever you choose
cyanotype::collected().write_openapi("openapi.json")?;
```

`record` must be the *last* thing applied to a fully-built router — it
installs itself via `Router::route_layer`, which only sees requests axum has
already matched to a route (this is what makes `MatchedPath`, and therefore
the route template in the output, available at all).

### Where to emit from — and why it has no clean hook

Rust has no "after every test in this binary finished" hook, and §2 of the
spec is explicit that writing is never bound to test execution. So the second
call is yours to place, and placing it is the one genuinely awkward step in
adopting this crate.

A recipe that works today:

```bash
cargo test -- --test-threads=1   # ordering is the only thing serialism buys
```

```rust
// At the *crate root* of your test module tree, not inside one of them.
#[tokio::test]
async fn zzzz_emit_openapi() {
    cyanotype::collected().write_openapi("openapi.json").unwrap();
}
```

**The trap, measured.** libtest runs tests in lexicographic order of their
full path, so a test named `zzz_…` inside a module sorts *before* every later
module. In the proof run the emission test sat inside one, and ran at
position 65 of 118 — 53 tests in later modules ran after it, and whatever they
recorded was never emitted. A `zzzz` prefix at the crate root sorts after
every module name, which is the version that works by construction rather than
by luck.

Measured impact of the trap in this suite: none. Trapped and untrapped runs
emit the same 45 operations over the same 38 paths, the same 61 (operation,
status) pairs, and the same per-pair sample counts — the later modules happen
to add no operation the earlier ones had not already recorded. That is exactly
why it survived a full build, and exactly why it is still a trap: it bites when
a later-sorting test owns a route exclusively, and nothing tells you whether
yours does.

Alternatives, honestly: accept the fragility and document it (what this crate
does), or put emission in a small separate binary/xtask your CI runs after
`cargo test` exits — robust, but a new file, and it can't see this process's
recorder, so it doesn't actually work. There is no `Drop`/`ctor` guard here:
emitting on process exit is exactly the hook that silently half-writes when a
test panics.

**One thing this no longer risks.** Warnings ride in the artifact itself (see
below), so an emission step whose stderr libtest swallows cannot hide them.

### Declaring a wire shape inference can't see

Handlers that return `json!({...})` with no backing Rust type are the norm,
not the exception, in the services this was built for — inference carries
essentially the whole schema. Where that's not precise enough (an `enum`,
say — inference deliberately never asserts one), declare it against the
response you already have, before consuming its body:

```rust
#[derive(serde::Deserialize, schemars::JsonSchema)]
struct BalanceResponse { balance: i64 }

let res = app.oneshot(req).await?;
let receipt = cyanotype::receipt(&res); // Option<Receipt>
let bytes = res.into_body().collect().await?.to_bytes();

if let Some(receipt) = receipt {
    receipt.declare_response::<BalanceResponse>(&bytes)?; // mismatch -> Err -> your test fails
}
```

A mismatch is a normal `Result::Err` — `?` or `.unwrap()` fails the test, per
the spec's "a mismatch fails the test" rule. See `DESIGN.md` §1 for why this
is shaped as a `Receipt` rather than a global lookup, and why request bodies
aren't covered (only response bodies are, in v1).

### Redaction

```rust
cyanotype::configure(cyanotype::Redactions::new().field("ssn").field("password"));
```

Call this once, before the tests whose bodies contain the field run — it
isn't part of the one-line recorder installation, it's occasional setup.
`cyanotype` ships no heuristics for *what* counts as sensitive (a heuristic
that's 90% right is worse than none, per the spec: it manufactures false
confidence). It does warn — never fail, and never redact on a guess — on
undeclared strings that look high-entropy.

**Warnings are delivered in the document**, at `info` →
`x-cyanotype-warnings`; the key is absent on a clean run. They also come back
from `Collected::warnings()`, but that channel alone is not enough: the
emission step runs inside a `#[tokio::test]`, and libtest captures and
discards the stdout and stderr of passing tests. In the first real run, six
live-looking JWTs were emitted with 41 warnings attached to them and not one
warning visible anywhere. The artifact's diff is the one place this document
is actually reviewed, so it is the one place a warning is guaranteed to be
seen.

Warnings are of two kinds, in the same array, and they arrive differently
because they cost differently:

- **Undeclared high-entropy strings** (§4.6) — one entry each, naming the site
  (`response POST /sessions data.token`). Security, so it stays
  impossible to miss.
- **Every operation below §4.3's sample threshold** — **one aggregated notice**
  naming all of them, not one entry per operation. On the measured run that is
  all 50 of the 61 (operation, status) pairs, and phrasing them
  individually cost ~6.8 KB of a 33 KB document — roughly 4k agent tokens. The
  notice repeats no sample counts: each response already carries its own
  `x-cyanotype-samples`.

## What's guaranteed regardless of configuration

- The `Authorization` header's value is never stored, logged, or emitted,
  under any configuration. It's stripped at the point headers are first
  captured (`src/auth.rs`), before an in-memory exchange even exists —
  there's no downstream code path that could leak it by accident.
- Every undeclared high-entropy string and every sub-threshold operation is
  named in the emitted document at `info` → `x-cyanotype-warnings`; absent
  key means a clean run. `cyanotype` never redacts on a guess (§4.6).
- A body whose content-type isn't JSON (or `+json`) is never embedded in the
  document — only its content-type and length are recorded.
- Nothing this crate does can reach a production build; it's meant to live
  under `[dev-dependencies]` only.

## The two decisions the spec left open

`SPEC.md` §4.2 (test-side declaration syntax) and §3 (body capture under
streaming, without unbounded buffering) were both deliberately left
unspecified — they're the hardest engineering and the most user-visible API
surface, respectively. Both are resolved in `DESIGN.md`, with the rejected
alternatives and why, and both are flagged there as **agent decisions
pending human review** rather than settled by fiat.

## Measured, not asserted

Per `SPEC.md` §5, two numbers are published here rather than assumed.

### Adoption cost

The acceptance target was the first consumer — a private production
Rust/axum service — and the recorder has now been **literally run against its
real suite**: 117 tests pass, 1 fails pre-existing and unrelated (`cyanotype`
added no failures), and it emitted a 33 KB document covering 45 operations
across 38 paths.

The claim was one line. The measured cost is **three**, and the difference is
the shape of that service rather than anything about this crate:

```diff
 pub fn build(cfg: Config, pools: Pools) -> Router {
-    routes::router(cfg, pools)
+    let router = routes::router(cfg, pools);
+    #[cfg(test)]
+    let router = cyanotype::record(router);
+    router
 }
```

`build()` is production code, called inline by ~40 test functions with no
test-only helper to wrap, so the recorder has to be gated behind
`#[cfg(test)]` rather than installed unconditionally: one line changed, three
added. Plus one line under `[dev-dependencies]`. Every one of the 58 `oneshot`
call sites is untouched, because they all route through this one function.

An earlier one-line figure came from a second service whose test helper has a
different shape and can take the wrapper directly. Both numbers are honest;
this is the one measured on the acceptance target.

For clawspec's yardstick (463 lines of scaffolding for 5 paths / 9
operations): this crate's recording-side cost doesn't scale with route or
operation count at all (one wrapper around the router, whatever it contains),
which is the entire bet §6 describes. Emission is not one line — see "Where to
emit from" above.

### The N-distribution

Measured, from the real run — this figure existed nowhere before, and it is the
number §4.3 was designed against. Across 61 (path, method, status) triples in
45 operations over 38 paths:

| samples | (operation, status) triples |
|---|---|
| 1 | 50 (82%) |
| 2 | 7 |
| 3 | 2 |
| 4 | 1 |
| 5 | 1 |

**82% of what this suite exercises is observed exactly once**, so §4.3's
`required`-at-N≥2 rule leaves most emitted operations with no `required` array
at all — and now says so in `x-cyanotype-warnings` rather than staying silent
about why. That is the outcome §4.3 designed for (silence over a false
assertion), confirmed on real data rather than assumed.

Reading why matters as much as the count: `app.rs`'s
`production_live_routes_match_rails_auth_contract` test alone hits 10 routes in
a single function purely to assert a 401 auth-boundary shape, contributing
nothing to any of those routes' happy-path sample count. A suite that asserts a
boundary shape is not a suite that exercises a contract, and this distribution
is what makes that visible.

## Limitations, stated rather than discovered

- **§4.4 (descriptions) is cut from the spec**, by ruling on 2026-09-22, not
  pending. A `///` doc comment compiles to a `#[doc]` attribute readable only
  at compile time by a macro on the item carrying it, and the recorder sees
  `Request`/`Response` values crossing a `tower::Layer` with no knowledge of
  which function produced them. The overlay that would bridge it is what the
  spec rejects by name; the companion proc-macro crate needs every documented
  handler annotated with its own route redundantly, against §3's one-line
  adoption budget. Every response's `description` field in the emitted
  document is therefore a fixed placeholder ("Observed 200 response."),
  derived from the recording and not from any handler — a reader must not
  mistake it for the handler's doc comment. See `SPEC.md`'s "Not in v1" table
  and `DESIGN.md` §3.
- Request bodies are documented only when they're JSON. A non-JSON request
  body is recorded as content-type and length and nothing else (`DESIGN.md`
  §2), so the operation gets no `requestBody` at all — the document says
  nothing rather than guessing a media type it never retained.
- §4.3's sample threshold is a constant (`MIN_SAMPLES_FOR_REQUIRED` = 2), not a
  configuration API. That is now the **design**, not a gap: the spec's earlier
  "default 2" wording implied a knob, the word has been struck, and no
  `configure`-style surface ships until a real caller needs one.
- Only the `Authorization` header is treated as a candidate auth header —
  a custom header like `X-Api-Key` is never detected or stripped. If a
  service authenticates that way, its key would need to be caught by the
  entropy-warning + `Redactions` path instead, not by §4.5's auth-derivation
  path.
- Redaction matches JSON object key *names*, at any depth, globally — not a
  full per-path declaration (`/user/ssn` vs. every `ssn` anywhere). It fails
  safe in the direction that matters (over-redacts, never under-), but it's
  coarser than what the spec's "declares paths" wording literally describes.
- A response body that's never fully drained by the test (dropped instead of
  `.collect()`-ed or similar) never finishes recording — there's no
  `Drop`-based fallback flush. Every axum test idiom that asserts on a body
  drains it, so this isn't expected to bite, but it's a real gap.
- The 64 KiB body-capture cap is a constant, not configurable, in v1 (see
  `DESIGN.md` §2 for why, and what it would take to make it one).
- `configure()`'s redaction config is last-write-wins, global, mutable
  process state — calling it from inside an individual `#[tokio::test]` that
  runs concurrently with others touching the same routes is racy by
  construction. Call it from a one-time setup path instead.
- Declared schemas (`declare_response`) that reference nested named types go
  through `schemars`' `openapi3()` preset, which resolves `$ref`s into
  `components.schemas` correctly for the common case; deeply recursive or
  generic declared types haven't been exercised beyond what the test suite
  covers here.

## MSRV, license

MSRV: latest stable minus two (1.93 → 1.91 as of 2026-09-22), tracked in
`Cargo.toml`'s `rust-version` and verified: the full test suite (`cargo
test`, all 66 tests including doctests) passes unmodified under `rustup run
1.91.0 cargo test`.

Licensed under either of `MIT` or `Apache-2.0`, at your option.
