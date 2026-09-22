use std::path::PathBuf;

/// Everything that can go wrong calling into `cyanotype`.
#[derive(Debug, thiserror::Error)]
pub enum CyanotypeError {
    /// A [`crate::declare::Receipt::declare_response`] call whose declared
    /// type didn't deserialize from the observed bytes. Per §4.2, this is
    /// meant to fail the calling test.
    #[error(
        "declared type `{type_name}` does not match the recorded response for {method} {route} \
         (status {status:?}): {source}"
    )]
    DeclarationMismatch {
        method: String,
        route: String,
        status: Option<u16>,
        type_name: &'static str,
        #[source]
        source: serde_json::Error,
    },

    #[error("failed to write openapi.json to {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to serialize the recorded exchanges as JSON: {0}")]
    Serialize(#[source] serde_json::Error),
}
