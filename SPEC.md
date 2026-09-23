# cyanotype — build-ready spec

**Status:** specification (v1)
**Scope:** Architectural contract, invariants, and boundaries for `cyanotype`.
All module and test comments referencing `§X.Y` resolve directly to sections of this document.

---

## 1. What it is

A Rust library that records a service's existing **axum integration tests** and emits an **OpenAPI 3.1 document** describing what those tests actually exercised.

One sentence: **records your axum tests, emits `openapi.json`.**

The promise is not that the document is complete. It is that **the document is an impression of what the tests actually touched** — derived, not written. A contact print, not a drawing. Hence the name.

### Not in v1

Named explicitly, because each was considered and cut:

| Cut | Why |
|---|---|
| A drift gate / `check` mode / CI enforcement | Not asked for. `cyanotype` generates and stops. A caller wanting a gate regenerates and runs `git diff --exit-code`. |
| Operation descriptions from handler doc comments (§4.4) | Cut. A `///` comment compiles to a `#[doc]` attribute, readable only at compile time by a macro on the item that carries it — a `tower::Layer` sees `Request`/`Response` and knows nothing of which function produced them. The overlay that would bridge it is rejected by name; the companion proc-macro crate needs every documented handler annotated with its own route redundantly, which runs against §3's one-line adoption budget. |
| A compatibility diff against the last release | Not in v1. Outside the tool's scope; `cyanotype` generates and stops. |
| An AXI CLI, an AXI SDK, TOON | Cut on measurement — see §7. |
| A rendered human surface (Swagger UI, Redoc) | Emitting the document is enough; rendering is the caller's. |
| Multi-framework support | axum only, said plainly. |
| Coverage reporting | Impossible: axum cannot enumerate its own routes. |
| Non-Rust services | Route templates live inside the router. |
| A CLI of our own | Collecting requires the caller's tests in-process; a CLI cannot do that. |

---

## 2. Shape

**One crate**, `cyanotype`, taken as a **`dev-dependency`**. Nothing it does may reach a production build.

Two calls in the caller's test code:

```rust
// 1. install the recorder — one line in the test helper
let app = cyanotype::record(build_router());

// 2. emit, wherever the caller chooses
cyanotype::collected().write_openapi(path_or_stdout)?;
```

**Writing is never bound to test execution.** The caller decides when and whether. A half-run that writes partial data is their diff to read, not ours to prevent. (`cyanotype` is a library, not an application.)

**Output goes to stdout or a caller-supplied path**, never a fixed location we choose. Downstream consumers (CI jobs, storage, documentation caches) remain the caller's concern.

**Nothing is committed.** The artifact is regenerable from any commit. The only committed things are the caller's tests, handler code, and optional redaction config.

**Distribution:** MSRV = latest stable minus two, stated in the README. Licence `MIT OR Apache-2.0`. Available via crates.io and GitHub.

---

## 3. Recording

**Attach point:** a `.layer()` inside axum's routing, where `MatchedPath` is already inserted before dispatch (verified in axum `path_router.rs:346-351`). This is an **axum adapter**, not a portable `tower::Layer` — the correct hook position differs per framework, and only axum is supported.

**Adoption budget: one line in a test helper for recording.** Anything needing more is cut rather than paid for. This is the product's single differentiator (see §7), so it is a hard constraint, not a goal.

Per request/response pair, capture:

- the **route template** from `MatchedPath` (`/users/{id}`, never `/users/42`)
- method, status, request and response headers, request and response bodies
- content types

---

## 4. Deriving the document

### 4.1 Schemas come from samples, and nothing else

There is **no code-side derivation**. Macro-based extraction paths were dropped because they buy nothing where a response has no named type and over-report where one exists (`skip_serializing_if`, `flatten`).

In measured production axum codebases, handlers return ad-hoc `json!` structures almost exclusively (over 500 ad-hoc `json!` returns vs 1 typed return across surveyed production services). **Inference carries 100% of the schema, with no floor beneath it.** §4.3 is therefore the most load-bearing section of this spec.

### 4.2 Declarations, where inference cannot see

Where the wire shape cannot be inferred, it is **declared test-side** — by annotation or comment inside the test. Core rationale: API-specific structs are part of the interface contract, and authors should explicitly write the definition for shapes inference cannot discover.

**The line that separates this from rswag: rswag makes declaration the form; here it is the exception.** The recorder still derives route templates, structure, the status matrix and examples. Declaration covers only what inference cannot see.

**A declaration is validated against the recording.** A declared type and a handler emitting `json!({...})` can diverge silently, so the recorder compares the observed body against the declared type and **a mismatch fails the test**. This is what stops declarations becoming a second place to lie.

Declaration is **optional**; inference is the fallback. Declaring buys precision, it is not the price of entry.

### 4.3 What inference may and may not assert

| Property | Rule | Why |
|---|---|---|
| `required` | Only when **N ≥ 2** and the field appeared in **every** sample. Union the properties, intersect the required (rspec-openapi's algorithm: union properties, intersect required). | At N=1 the intersection is "whatever that one response happened to contain." |
| `enum` | **Never inferred, at any N.** Observed values become `examples`. An `enum` appears only from a declaration. | Declaring an enum of 2 where the API has 12 makes a reading agent **reject valid values** — worse than saying nothing. |
| `nullable` | On **positive evidence only** — a `null` was observed. Never inferred from absence. | Seeing `null` once proves nullable; never seeing it proves nothing. |
| sample count | **Always** emitted as `x-cyanotype-samples: N` on every operation. | Exposes the evidence so a reader can judge the assertion. |

The threshold is **not configurable, and that is now the design rather than a gap**: v1 ships one constant, `MIN_SAMPLES_FOR_REQUIRED` (= 2), shared by the inference rule and the warning that reports on it so the two cannot drift. The earlier "default 2" wording implied a knob; there is no `configure`-style surface for it, and none is planned until a real caller needs one. Empirical data confirms the distribution — no published accuracy figure for sample-based inference existed in literature.

**The generator warns** which operations fall below the threshold, so the author can add a test or declare the type. It does not fail.

### 4.4 Descriptions — cut

**There is no §4.4.** Prose stays in handler doc comments, where it is code, reviewed with the code, and cannot rot when an endpoint is deleted — but `cyanotype` does not lift it into the document, because it cannot reach it (see §1's cut table for the two bridging designs and why each was rejected).

The `description` on every emitted response is a fixed, response-object-level placeholder (`"Observed 200 response."`). It is derived from the recording, it is not operation prose, and a reader must not mistake it for the doc comment of the handler behind the route.

A documented overlay was the alternative and is explicitly refused: an overlay is a second place to write the same thing, and it drifts from the code it describes. Silence beats a document that lies about its own provenance.

**What a consumer loses:** an agent reading the emitted document gets no prose about what an operation is *for*. That is a real cost, accepted knowingly.

### 4.5 Auth

The recorder **derives the security scheme from the observed header** — the header name, and whether the value begins `Bearer ` (`http`/`bearer`) or not (`apiKey`). No configuration.

**The value is stripped before anything is written.** `Authorization` and any header matching a derived scheme are removed unconditionally — not a policy knob, not switchable off. The artifact frequently lands in CI logs and public builds; an unredacted credential leaks immediately.

What is emitted is the declaration only:

```json
{ "components": { "securitySchemes": { "bearerAuth": { "type": "http", "scheme": "bearer" } } } }
```

**Honest limit for the README:** the recorder sees *that* a header was sent, not what it authorised. No scopes, no per-role mapping. Derived auth **detection** is real; derived auth **semantics** are not.

### 4.6 Redaction

Bodies still carry real recorded payloads: secrets, PII, customer data.

**Declared, with no heuristics shipped.** The author declares paths in a committed config; `cyanotype` guesses nothing. A heuristic that catches 90% of secrets is **worse than none** — it manufactures a belief in protection that is wrong 10% of the time, and the 10% is what leaks.

**The generator warns** on undeclared high-entropy strings over a length threshold, and the author marks them redacted or safe with the decision recorded. Warn only, never fail. Accepted cost: false positives on legitimate opaque ids, and this is the first place `cyanotype` nags its user.

**Warnings are delivered inside the artifact**, at `info` → `x-cyanotype-warnings` (absent when there is nothing to say), *not* only on stderr or only through an API. Reason: emission frequently happens inside integration test harnesses that capture and discard stdout/stderr of passing tests. The artifact's diff is the one place this document is reviewed, so it is the one place a warning is guaranteed to be seen. Warn only, never fail, never redact on a guess — a 90%-accurate secret detector is worse than none, because it manufactures confidence in protection that is wrong 10% of the time.

**There is no determinism pass.** Canonicalising timestamps and UUIDs existed to stop a *committed* artifact churning (rswag #594). Nothing is committed, so there is no diff to stabilise.

### 4.7 Output

`openapi.json`, **minified**. One artifact. It is *shaped* to serve tooling (client codegen, Redoc, `oasdiff`) and agents (via docs-mcp, which caches and searches what it is given) alike — but no consumer of either kind exists today, so those are affordances, not integrations. See §2.

Every document records the **generator version** as an `info` extension, because "did the generator change?" is otherwise unanswerable when output shifts for no apparent reason.

---

## 5. Validation and Acceptance

v1 acceptance was validated against a production axum service (over 115 integration tests covering 38 paths and 45 operations):

1. The service's existing tests run with the recorder installed in its router construction helper.
2. An `openapi.json` document is produced covering the operations those tests actually exercise.
3. Two measurements are published in the documentation:
   - **Adoption cost in lines actually changed**, evaluated against alternative approaches requiring hundreds of lines of per-route annotations;
   - **The N-distribution** — samples per (path, method, status) in a real suite, establishing that a large majority of operations in practice are exercised with N=1 sample.

---

## 6. Prior art, and what we take from it

| Project | What it is | What we take |
|---|---|---|
| **rswag** (Ruby) | The named baseline — and it **does not do the thing**. It derives nothing from the running test, reads declared metadata only, and `rswag_dry_run` defaults true so the shipped path runs zero tests. No check mode, no output validation; it emits invalid OpenAPI in its own repo. | The idea, and a list of mistakes. |
| **rspec-openapi** (Ruby) | The honest implementation of what rswag is credited with: derives from *unmodified* request specs, no DSL. | Its union-properties / intersect-required algorithm (~200 lines, port it). |
| **clawspec** (Rust) | The only Rust prior art. **Sidesteps the hard problem**: path templates are caller-declared with zero inference, and the parameter name is load-bearing operation identity. 47% of its own output is `utoipa::ToSchema` annotation. No drift gate. Bus factor 1, stalled since 2026-07-18, zero external PRs ever merged. | Read, don't depend. Its `split` output module and `TestServer`-on-a-`TcpListener` coupling are worth reimplementing. |
| **Optic**, **Akita** | Both shipped this thesis at company scale. Both archived. | A warning. |

**Positioning:** credit clawspec by name and state the difference as a different bet on adoption cost — **they ask you to declare the template, we infer it from `MatchedPath`.** That is the whole differentiator, which is why §3's one-line budget is a constraint rather than an aspiration.

---

## 7. Decisions worth not relitigating

Three positions in this spec reverse something the map originally held. Each is recorded so it is not rediscovered.

**The drift gate was never asked for.** The drift gate was deliberately excluded: `cyanotype` generates and stops. Callers wanting CI enforcement run `git diff --exit-code` on the generated document.

**Artifacts are not committed.** Tested against the package-lock analogy: a lockfile is committed because it **cannot** be re-derived — the registry moves — while the same commit and the same tests always produce the same document.

**TOON and AXI were cut on measurement.** Measured on 25 paths of the GitHub API spec (`cl100k_base`; direction reliable, magnitude approximate):

| encoding | tokens |
|---|---|
| OpenAPI as pretty JSON | 34,417 |
| OpenAPI as YAML | 26,882 |
| OpenAPI as TOON | 25,648 |
| **OpenAPI as minified JSON** | **22,361** |

TOON beats YAML by 4.6% and **loses to minified JSON by 14.7%**. Its advertised 42.6% is against *pretty* JSON, which nobody ships to an agent; OpenAPI is a nested `$ref` tree, not the uniform rows TOON is built for.

A flattened operations index was also considered and declined (evaluated in design benchmarks). Its 96.7% figure measured an index against the full document — **true arithmetic on unlike things**, since a table of contents is smaller than the book by definition. Its one real argument was enumeration: an agent cannot search for a word it has never met. Judged not worth a second artifact.

---

## 8. Open, deliberately

- **Enumeration.** Without an operation index, automated tooling relies on full OpenAPI document parsing rather than a shallow table of contents.
- **Sustainability.** Optic and Akita are archived. The hypothesis — a test suite is a smaller, more tractable surface than production traffic, and a dev-dependency has nothing to operate — is structural and unmeasured. One honest README line, not a claim.
- **Coverage visibility.** Emitting an OpenAPI document from integration tests inherently reflects the test suite's coverage rather than theoretical routes.

---

## 9. Build order

1. Recorder layer for axum, capturing `MatchedPath` + request/response pairs. Prove it against real axum router suites.
2. Schema inference per §4.3, with `x-cyanotype-samples` from the start.
3. Auth scheme derivation and the unconditional header strip (§4.5) — before any output is written anywhere.
4. Declared redaction and the high-entropy warning (§4.6).
5. Test-side declarations, validated against the recording (§4.2).
6. `openapi.json` emission to stdout or path, with the generator-version stamp.
7. Measure and publish the two §5 numbers.
