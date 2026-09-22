# cyanotype

Records an axum service's own integration tests and emits an OpenAPI 3.1
document describing what those tests actually exercised. A dev-dependency,
nothing more — install it in your test helper, drive your existing tests,
emit `openapi.json` whenever you choose.

The document isn't a claim of completeness. It's an impression of what the
tests actually touched — derived, not written. A contact print, not a
drawing. See `spec.md` in this repository's origin (`~/Working/ai/plain/.scratch/rswag-rs/spec.md`)
for the full design brief this crate was built against, and `DESIGN.md` for
the two decisions that brief deliberately left open.

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
confidence). It does warn — never fail — on undeclared strings that look
high-entropy, via `Collected::warnings()`.

## What's guaranteed regardless of configuration

- The `Authorization` header's value is never stored, logged, or emitted,
  under any configuration. It's stripped at the point headers are first
  captured (`src/auth.rs`), before an in-memory exchange even exists —
  there's no downstream code path that could leak it by accident.
- A body whose content-type isn't JSON (or `+json`) is never embedded in the
  document — only its content-type and length are recorded.
- Nothing this crate does can reach a production build; it's meant to live
  under `[dev-dependencies]` only.

## The two decisions the spec left open

`spec.md` §4.2 (test-side declaration syntax) and §3 (body capture under
streaming, without unbounded buffering) were both deliberately left
unspecified — they're the hardest engineering and the most user-visible API
surface, respectively. Both are resolved in `DESIGN.md`, with the rejected
alternatives and why, and both are flagged there as **agent decisions
pending human review** rather than settled by fiat.

## Measured, not asserted

Per `spec.md` §5, two numbers are published here rather than assumed.

### Adoption cost

The acceptance target is `uniar-api-rs`
(`~/Projects/backend/uniar/rust/uniar-api`), per the spec, which was **read
to understand its test-helper shape and not modified**, per this task's own
instruction — those two constraints (§5 wants it literally run against that
repo; the task forbids touching that repo) are in direct tension, and the
resolution taken here is the one the task itself allows: an equivalent
fixture in `tests/support/mod.rs`, plus a direct, line-by-line reading of the
real file for the numbers below.

`uniar-api/src/app.rs`'s test helper is, verbatim (read 2026-09-22):

```rust
pub fn build(cfg: Config, pools: Pools) -> Router {
    routes::router(cfg, pools)
}
```

Every one of that file's 40 `#[tokio::test]` functions calls `build(...)`
fresh rather than sharing one instance (measured: `grep -c '#\[tokio::test\]'`;
70 total across the whole crate, 30 of them not touching HTTP at all), and 58
of those 40 tests' calls reach `.oneshot(...)` (measured: `grep -c
'\.oneshot('`) — close to the spec's own cited "~60". Adopting `cyanotype`
for recording is:

```diff
 pub fn build(cfg: Config, pools: Pools) -> Router {
-    routes::router(cfg, pools)
+    cyanotype::record(routes::router(cfg, pools))
 }
```

**One line changed, zero added, zero removed** — verified mechanically, not
just asserted, in `tests/adoption.rs`. Every one of the 58 `oneshot` call
sites is untouched, because they all go through this one function. This
matches the spec's claim exactly, for the recording half of the API.

What that number doesn't include, because the spec's own "one line" claim is
scoped to §3 (recording) and doesn't cover this: `Cargo.toml` needs
`cyanotype` under `[dev-dependencies]` (+1 line, never reaching the
production build), and *emission* has no one-line answer at all. Rust has no
built-in "after all tests in this binary finished" hook, so
`cyanotype::collected().write_openapi(...)` has to run from somewhere —
a dedicated `#[test]` relying on `--test-threads=1` for ordering (a few
lines, fragile), or a small separate binary/xtask run by CI after `cargo
test` exits (robust, but a new file). This is a real gap, named in
`DESIGN.md`, not a `cyanotype` defect — §2 of the spec is explicit that
writing is never bound to test execution — but reporting "one line" without
it would only be reporting the easy half.

For clawspec's yardstick (463 lines of scaffolding for 5 paths / 9
operations): this crate's recording-side cost doesn't scale with route or
operation count at all (it's one wrapper around the router, regardless of
how many routes it contains), which is the entire bet §6 describes — inferred
templates instead of declared ones.

### The N-distribution

Running the real recorder against `uniar-api`'s actual suite wasn't possible
without modifying that repo (forbidden by this task), so this number is a
**static proxy**, not the live figure — reported as such, not dressed up as
the real thing. Tallying every literal path string referenced in
`uniar-api/src/app.rs`'s tests (`grep -oE '"/[a-zA-Z0-9_/{}:.-]*"' | sort |
uniq -c`) found 46 distinct literal paths:

| times referenced | distinct literal paths |
|---|---|
| 1 | 31 (67%) |
| 2 | 7 |
| 3 | 5 |
| 4 | 2 |
| 8 | 1 (`/api/v1/account`) |

This undercounts the true operation-level N in both directions at once: it's
too low where several concrete literal paths collapse onto one route
template at runtime (`/vue-api/v1/rewards/99999999` and
`/vue-api/v1/rewards/{id}` are two rows above but one operation to a live
recorder), and it's too high where the same literal path appears with
different query strings that a route template doesn't distinguish. Even so,
the qualitative shape is unlikely to be an artifact of the proxy: a clear
majority (67%) of the routes this suite touches are hit by only one literal
test call. Reading why matters more than the count — `app.rs`'s
`production_live_routes_match_rails_auth_contract` test alone hits 10 routes
in a single function purely to assert a 401 auth-boundary shape, contributing
nothing to any of those routes' *happy-path* sample count. Under §4.3's
`required`-at-N≥2 rule, that means a large fraction of `uniar-api`'s emitted
operations would carry **no `required` array at all** and would surface in
`cyanotype`'s own below-threshold warning — which is exactly the outcome
§4.3 designed for (silence over a false assertion), not a defect in this
measurement.

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
  mistake it for the handler's doc comment. See `spec.md`'s "Not in v1" table
  and `DESIGN.md` §3.
- Request bodies are documented only when they're JSON. A non-JSON request
  body is recorded as content-type and length and nothing else (`DESIGN.md`
  §2), so the operation gets no `requestBody` at all — the document says
  nothing rather than guessing a media type it never retained.
- §4.3's sample threshold is a constant (`MIN_SAMPLES_FOR_REQUIRED`), not a
  configuration API. The spec says "default 2", implying a knob; there is no
  `configure`-style surface for it in v1.
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
test`, all 60 tests including doctests) passes unmodified under `rustup run
1.91.0 cargo test`.

Licensed under either of `MIT` or `Apache-2.0`, at your option.
