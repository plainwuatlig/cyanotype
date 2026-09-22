#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
// Pedantic lints that fight the shape of this crate rather than improve it:
// - `module_name_repetitions`: `exchange::Exchange`, `layer::RecordLayer` etc.
//   read fine at the call site (`crate::layer::RecordLayer`) and renaming
//   away from the domain noun loses clarity for no gain.
// - `must_use_candidate`: this crate's fallible constructors already return
//   `Result`/richer types where it matters; blanket `#[must_use]` on every
//   builder method is noise here.
// - `doc_markdown`: "OpenAPI" is a proper noun throughout this codebase and
//   its own spec, not a code identifier; wrapping it in backticks everywhere
//   reads worse, not better.
#![allow(
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::doc_markdown
)]

//! `cyanotype` records an axum service's own integration tests and emits an
//! OpenAPI 3.1 document describing what those tests actually exercised.
//!
//! ```no_run
//! # async fn build_router() -> axum::Router { axum::Router::new() }
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // 1. install the recorder — one line in the test helper
//! let app = cyanotype::record(build_router().await);
//!
//! // ... drive `app` through your existing tests with `tower::ServiceExt::oneshot` ...
//!
//! // 2. emit, wherever the caller chooses
//! cyanotype::collected().write_openapi("openapi.json")?;
//! # Ok(())
//! # }
//! ```
//!
//! See `DESIGN.md` in the repository for the two decisions this crate had to
//! make that the spec deliberately left open: the shape of test-side
//! declarations (§4.2) and how request/response bodies are captured (§3).

mod auth;
mod body;
mod declare;
mod error;
mod exchange;
mod infer;
mod layer;
mod openapi;
mod redact;
mod store;

use std::path::{Path, PathBuf};

use axum::Router;

pub use declare::Receipt;
pub use error::CyanotypeError;
pub use redact::{Redactions, configure};

/// Install the recorder into a fully-built router.
///
/// Call this once, wrapping whatever your test helper already returns —
/// per §3, this is meant to be the one line a caller changes:
///
/// ```no_run
/// # fn build_router() -> axum::Router { axum::Router::new() }
/// let app = cyanotype::record(build_router());
/// ```
///
/// Internally this installs the recorder via [`axum::Router::route_layer`]
/// rather than [`axum::Router::layer`], because only a route-level layer
/// runs after axum has inserted `MatchedPath` into the request — see
/// `spec.md` §3 and `src/layer.rs`.
pub fn record<S>(router: Router<S>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    router.route_layer(layer::RecordLayer)
}

/// Retrieve the [`Receipt`] for a response that `cyanotype` recorded, while
/// you still hold the `Response` — i.e. before calling `.into_body()` on it.
///
/// Returns `None` if the response wasn't produced by a router wrapped with
/// [`record`] (for example, a response that came from code under test that
/// isn't reachable through the recorded router at all).
pub fn receipt<B>(response: &http::Response<B>) -> Option<Receipt> {
    response.extensions().get::<Receipt>().cloned()
}

/// A snapshot of every exchange `cyanotype` has recorded so far in this
/// process, ready to be emitted.
pub struct Collected {
    exchanges: Vec<exchange::Exchange>,
}

/// Where [`Collected::write_openapi`] sends the document.
pub enum Destination {
    Stdout,
    Path(PathBuf),
}

impl From<&str> for Destination {
    fn from(value: &str) -> Self {
        if value.is_empty() || value == "-" {
            Destination::Stdout
        } else {
            Destination::Path(PathBuf::from(value))
        }
    }
}

impl From<String> for Destination {
    fn from(value: String) -> Self {
        Destination::from(value.as_str())
    }
}

impl From<PathBuf> for Destination {
    fn from(value: PathBuf) -> Self {
        Destination::Path(value)
    }
}

impl From<&Path> for Destination {
    fn from(value: &Path) -> Self {
        Destination::Path(value.to_path_buf())
    }
}

impl Collected {
    /// Emit the recorded exchanges as a minified OpenAPI 3.1 document.
    ///
    /// Writing is never automatic and never bound to test completion (§2) —
    /// call this wherever and whenever your own test suite decides to.
    /// Pass `"-"` (or any empty string) for stdout, or any path.
    ///
    /// # Errors
    ///
    /// Returns [`CyanotypeError::Serialize`] if, somehow, the assembled
    /// document isn't representable as JSON, or [`CyanotypeError::Write`] if
    /// writing to a given path fails.
    pub fn write_openapi(&self, destination: impl Into<Destination>) -> Result<(), CyanotypeError> {
        let document = openapi::build_document(&self.exchanges);
        let json = serde_json::to_string(&document).map_err(CyanotypeError::Serialize)?;

        match destination.into() {
            Destination::Stdout => {
                println!("{json}");
                Ok(())
            }
            Destination::Path(path) => std::fs::write(&path, json.as_bytes())
                .map_err(|source| CyanotypeError::Write { path, source }),
        }
    }

    /// Warnings accumulated so far: samples below the inference threshold,
    /// bodies truncated at the capture cap, undeclared high-entropy strings.
    /// Never fails a test on its own — surfacing these is the caller's call.
    pub fn warnings(&self) -> Vec<String> {
        store::warnings_snapshot()
    }
}

/// Snapshot everything recorded so far in this process.
///
/// See [`Collected::write_openapi`] for emitting it.
pub fn collected() -> Collected {
    Collected {
        exchanges: store::snapshot(),
    }
}
