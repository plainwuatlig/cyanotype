//! §4.2: test-side declarations, validated against the recording.
//!
//! See `DESIGN.md` §1 for why this is shaped as a [`Receipt`] pulled off a
//! response rather than a global lookup keyed by method/path/status.

use crate::error::CyanotypeError;
use crate::store::SharedExchange;

/// A handle to the exchange a specific `Response` belongs to.
///
/// [`crate::record`] attaches one of these as a response extension on every
/// response it observes. Retrieve it with [`crate::receipt`] while you still
/// hold the `Response` (before calling `.into_body()`), then validate the
/// bytes your test already collects against a Rust type with
/// [`Receipt::declare_response`].
#[derive(Clone)]
pub struct Receipt(SharedExchange);

impl Receipt {
    pub(crate) fn new(exchange: SharedExchange) -> Self {
        Self(exchange)
    }

    /// Declare the wire shape of this response's body as `T`.
    ///
    /// Fails if `bytes` doesn't deserialize as `T` — per §4.2, "a mismatch
    /// fails the test". On success, `T`'s JSON Schema (via
    /// `schemars::JsonSchema`) is recorded and takes precedence over
    /// inference for this operation+status when the document is emitted.
    ///
    /// Only response bodies are supported in v1 — see `DESIGN.md` §1 for why
    /// request-body declaration is cut.
    ///
    /// # Errors
    ///
    /// Returns [`CyanotypeError::DeclarationMismatch`] if `bytes` doesn't
    /// deserialize as `T`.
    pub fn declare_response<T>(&self, bytes: &[u8]) -> Result<(), CyanotypeError>
    where
        T: serde::de::DeserializeOwned + schemars::JsonSchema,
    {
        let mut exchange = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        serde_json::from_slice::<T>(bytes).map_err(|source| {
            CyanotypeError::DeclarationMismatch {
                method: exchange.method.to_string(),
                route: exchange.route_template.clone(),
                status: exchange.response.status,
                type_name: std::any::type_name::<T>(),
                source,
            }
        })?;

        let root_schema = schemars::r#gen::SchemaSettings::openapi3()
            .into_generator()
            .into_root_schema_for::<T>();
        exchange.declared_response_schema =
            Some((root_schema.schema.into(), root_schema.definitions));

        Ok(())
    }
}
