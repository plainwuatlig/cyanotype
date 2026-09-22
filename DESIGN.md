# DESIGN.md — the two gaps the spec left open

`spec.md` (§4.2 and §3) deliberately leaves two shapes unspecified because they
are the API surface and the hardest engineering, respectively. Both decisions
below are **agent decisions pending human review** — flagged per the task
brief, not blocking on it. If Plain wants a different shape for either, both
are isolated (one API in `declare.rs`/`receipt` on the `lib.rs` surface, one
internal type in `body.rs`) and can be swapped without touching the rest of
the crate.

---

## 1. Test-side declaration syntax (§4.2)

### The chosen shape: a `Receipt` pulled off the response, validated against caller-supplied bytes

```rust
#[derive(serde::Deserialize, schemars::JsonSchema)]
struct BalanceResponse {
    balance: i64,
}

#[tokio::test]
async fn balance_happy_path() {
    let res = app.oneshot(req).await.unwrap();
    let receipt = cyanotype::receipt(&res); // Option<Receipt>, from a response extension
    let bytes = res.into_body().collect().await.unwrap().to_bytes();

    if let Some(receipt) = receipt {
        receipt.declare_response::<BalanceResponse>(&bytes)?; // mismatch -> Err -> test fails
    }
    // ... the test's own assertions on `bytes` continue unchanged
}
```

`cyanotype::record()` inserts a `Receipt` (an `Arc<Mutex<Exchange>>` handle) as
a response extension on every recorded response, at the same point it wraps
the response body for capture (`layer.rs`). `receipt.declare_response::<T>`:

1. Deserializes the caller's own already-collected `bytes` as `T`
   (`serde_json::from_slice`). Failure is a normal `Result::Err`, so `?` or
   `.unwrap()` in the test fails it — no new assertion macro to learn.
2. On success, derives a JSON Schema fragment from `T` via
   `schemars::JsonSchema::json_schema()` and stores it on the exchange,
   where it takes precedence over inferred schema for that operation+status
   in `openapi.rs`.

### Why this shape and not the alternatives

- **Why a struct + `schemars`, not a hand-rolled schema fn.** Plain's ruling
  (ticket 08) was "we should WRITE the def for them" — a Rust struct *is*
  that definition, and it is a Rust struct anyway: it's what `serde`
  deserialization needs to validate against. Piggybacking `schemars::JsonSchema`
  derive on the same struct is one derive, not a second artifact. Rejected:
  a hand-written JSON Schema literal per declaration — doubles the
  maintenance surface Plain explicitly wanted to avoid (a struct is one
  source of truth; a struct plus a schema literal is two, and they drift).

- **Why validation is "does it deserialize", not full JSON Schema
  validation against the generated schema.** `serde`'s `Deserialize` is
  already a stricter, more idiomatic Rust contract than replaying our own
  generated schema through a JSON Schema validator crate (which would
  validate the schema *we derived from the same type*, a tautology — the
  interesting check is "does the wire body actually match the Rust type",
  which `serde_json::from_slice::<T>` answers directly). Rejected: pulling
  in `jsonschema` (or similar) to validate bytes against the emitted
  schema — extra dependency, and it validates the derivation's own output
  against itself rather than the type against the wire.

- **Why a `Receipt` handle, not `cyanotype::declare(method, path, status,
  &bytes)`.** A free function keyed by (method, path, status) is racy: axum
  integration test binaries run `#[tokio::test]` fns concurrently by
  default (separate OS threads in one process, one global recorder store),
  so "declare against the last exchange matching this route" can pick up
  a different, concurrently-running test's exchange. A `Receipt` is
  obtained from the exact `Response` the test already holds, so there is no
  shared lookup key and no race. Rejected: a global "most recent match"
  API — cheap to write, wrong under `cargo test`'s default parallelism.

- **Why request-body *declaration* is out of scope for v1.** The friction
  §4.2 addresses is server-authored ad hoc `json!()` responses with no Rust
  type behind them. Request bodies in these tests are already
  caller-constructed Rust values (`json!({...})` literals or already-typed
  structs at the call site) — the test author already has the shape in
  hand and doesn't need `cyanotype` to help them declare something they
  just wrote. (Request bodies are still *recorded and emitted*, by
  inference, per §3 — see `openapi.rs`'s `requestBody` assembly. What is
  cut here is only the declaration API, `declare_request`.) Symmetric request-side declaration is a small extension
  (attach a `Receipt` to the request extensions too) if it turns out to be
  wanted; cut for v1 on measured absence of need, not difficulty.

- **Naming: `declare_response`, not `declare`.** Leaves room for a
  `declare_request` later without a breaking rename.

**Honest limitation for the README:** if a test never calls
`cyanotype::receipt()`, nothing about declarations changes — inference
still runs. Declaration is additive and optional, exactly as §4.2 requires.

---

## 2. Body capture (§3): a streaming tee, not a buffer-then-forward

### The chosen shape: `TeeBody<B>`, a transparent `http_body::Body` wrapper

Both the request body (before it reaches the handler) and the response body
(before it reaches the test's own `.collect()`) are replaced with
`body::TeeBody`, which:

- **passes every frame through unchanged and immediately** — it never
  delays, reorders, or buffers-then-replays a frame. Streaming, chunked
  responses, and backpressure inside the handler under test behave exactly
  as they would without `cyanotype` installed.
- **copies each frame's bytes into a capped side-buffer** (64 KiB constant,
  `body::CAPTURE_CAP_BYTES`) purely for recording. Once the side-buffer
  hits the cap, further bytes are still forwarded downstream untouched but
  stop being copied; the exchange is marked `truncated`.
- **on end-of-stream**, hands the captured (possibly truncated) bytes plus
  the observed content-type to the owning `Exchange`, where:
  - `content-type` is `application/json` or `…+json` **and** the captured
    bytes are complete (not truncated) → parsed as `serde_json::Value` for
    inference/examples.
  - `content-type` is JSON but the body was **truncated** → the bytes are
    dropped entirely (a partial JSON document is worse than none — it
    would either fail to parse or silently parse as a different,
    truncated shape) and a warning is recorded: `"body for <method>
    <route> exceeded 64KiB and was not sampled"`.
  - `content-type` is anything else (or absent, see below) → **no bytes
    are retained past content-type + length**, only `content_type` and
    `content_length` are recorded. No binary blobs, multipart bodies, or
    opaque payloads are ever embedded in `openapi.json`, regardless of
    size. This is a size decision as much as a leakage one: a multipart
    upload fixture would otherwise dominate the artifact.
  - Content-type **absent** and the body is non-empty: a best-effort
    `serde_json::from_slice` is attempted (many ad hoc `json!()` handlers
    in practice still set `application/json` via axum's `Json<>`
    extractor, but raw `(StatusCode, String)` returns don't); if it
    parses, treated as JSON; if not, treated as opaque per the rule above.
    This fallback only fires on an *absent* header — an explicit
    non-JSON content-type is never second-guessed.

### Why this shape and not the alternatives

- **Rejected: full-buffer via `axum::body::to_bytes(body, limit)`, then
  `Body::from(bytes)` to forward.** This is the obvious first design and
  is what most examples of axum request-logging middleware show. It was
  rejected because it inverts the spec's own framing: it *does* buffer the
  whole body before the handler ever sees a byte, which (a) defeats
  streaming for any handler that starts responding before the request
  body finishes (none in `uniar-api` today, but the layer shouldn't assume
  that forever), and (b) forces a size limit that, if exceeded, must
  either reject the request (changing the behavior of the app under test —
  a `cyanotype`-instrumented test suite must not 413 requests an
  uninstrumented one would accept) or buffer unboundedly (the literal
  "blowing memory" the task calls out). A tee avoids the dilemma: the cap
  only bounds what we *keep*, never what we *forward*.
- **Rejected: hashing bodies instead of storing bytes.** Solves memory and
  leakage at once, but throws away the one thing §4.3's inference and
  §4.7's examples need — actual sample values. Not viable given §4.1
  ("inference carries 100% of the schema").
- **Rejected: `Content-Length`-gated buffering** (buffer fully only when
  the header promises a small body, stream-passthrough-only otherwise).
  Adds a code path (and a lie: `Content-Length` is caller-supplied and not
  binding) for a case the tee already handles uniformly with one code
  path and no special-casing.
- **Why 64 KiB and not configurable.** The spec makes exactly two things
  configurable — the inference sample threshold (§4.3) and (implicitly)
  the redaction entropy threshold (§4.6) — and is silent on this one.
  64 KiB comfortably covers every JSON body in `uniar-api`'s test corpus
  (checked by inspection of `src/**/*.rs` fixtures: the largest literal
  JSON body order tens of routes construct is low single-digit KiB) while
  keeping a full test run's aggregate memory bounded regardless of sample
  count. If real usage shows JSON responses routinely exceeding this, the
  fix is a constructor parameter on `record()`, not a global — left as a
  follow-up rather than speculative config now.

**Known limitation, stated plainly:** if a response body is never fully
drained (the test drops the `Response` without calling `.collect()` or
similar), that exchange's response side never finishes recording — no
`Drop`-based fallback flush is implemented. Every `axum` integration test
idiom that asserts on a body (which is the only reason to write the test)
drains it, so this is not expected to bite in practice, but it is a real gap
versus a fully defensive implementation and is called out here rather than
silently accepted.

---

## 3. §4.4 (descriptions) — cut from the spec on 2026-09-22

Independent code review (`code-review` skill, spec axis) caught that §4.4
("Prose lives as handler doc comments...") had no implementation at all, and
that this had gone undisclosed in both DESIGN.md and the README's own
Limitations list — a silent drop, not a stated cut. This section closed the
disclosure gap; the ruling has since closed the gap itself. **§4.4 is cut from
the spec** (`spec.md`'s "Not in v1" table, amended 2026-09-22), so this is no
longer an open question and no longer a defect: it is the product.

**The obstacle was structural, not effort.** A Rust `///` doc comment compiles
to a `#[doc = "..."]` attribute, which is only visible at *compile time*, to a
macro applied to the item that carries it. There is no runtime API that lets
`cyanotype`'s recorder — which only ever sees `http::Request`/`http::Response`
values crossing a `tower::Layer`, with no knowledge of which Rust function
produced them — read a handler function's doc comment while a test is running.

**Two ways to bridge that gap, both rejected — now permanently:**

- **An overlay** — a separate place the author writes each operation's
  description, keyed by method+route. This is exactly what §4.4 argued against
  by name ("There is no overlay... Ticket 07's overlay... died with AXI") and
  would reintroduce the drift problem doc comments were chosen to avoid.
  Rejected on the spec's own stated reasoning, not a new judgement.
- **A companion attribute proc-macro** (e.g. `#[cyanotype::documented(method
  = "GET", route = "/users/{id}")]` above each handler fn), which captures
  the fn's `#[doc]` attribute at compile time and registers `(method, route)
  -> description` into a static table the recorder can consult at request
  time. Technically correct, and it doesn't reintroduce an overlay — but it
  requires annotating *every documented handler* with its own route
  redundantly (the macro can't see what `.route(...)` call will attach the fn
  to, since that happens later, elsewhere), which is real per-handler ceremony
  running directly against §3's one-line adoption budget being "the product's
  single differentiating factor." It also means a second crate (`cyanotype`
  isn't itself a proc-macro crate) — defensible as a companion the way
  `serde`/`serde_derive` split, but a larger surface than v1 covers.

**Consequence for a consumer:** an agent reading the emitted document gets no
prose about what an operation is *for*. The generic per-status `"description"`
field ("Observed 200 response.") is a fixed placeholder derived from the
recording — it is not operation-level documentation, and a reader must not
mistake it for the doc comment of the handler behind the route. That cost is
accepted knowingly, not overlooked.
