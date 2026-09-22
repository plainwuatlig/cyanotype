//! The in-memory shape of one recorded request/response pair.
//!
//! Nothing in this module ever holds an unredacted `Authorization` value —
//! [`crate::auth::capture_headers`] strips it at the point headers are first
//! turned into a [`MessageCapture`], before an `Exchange` exists at all.

use http::Method;

use crate::auth::SecurityScheme;

/// One recorded request/response pair.
#[derive(Debug, Clone)]
pub(crate) struct Exchange {
    pub method: Method,
    /// The route template from `MatchedPath` (`/users/{id}`, never `/users/42`).
    pub route_template: String,
    pub request: MessageCapture,
    pub response: MessageCapture,
    pub security: Option<SecurityScheme>,
    /// Set by [`crate::declare::Receipt::declare_response`] when a test
    /// declares the response wire shape (§4.2). Takes precedence over
    /// inference for this operation+status during assembly. The second
    /// element carries any named subschemas the declared type referenced
    /// (via `$ref`), to be merged into `components.schemas`.
    pub declared_response_schema: Option<(
        schemars::schema::Schema,
        schemars::Map<String, schemars::schema::Schema>,
    )>,
}

impl Exchange {
    pub fn new(method: Method, route_template: String, security: Option<SecurityScheme>) -> Self {
        Self {
            method,
            route_template,
            request: MessageCapture::default(),
            response: MessageCapture::default(),
            security,
            declared_response_schema: None,
        }
    }
}

/// Captured headers + body for one side (request or response) of an exchange.
#[derive(Debug, Clone, Default)]
pub(crate) struct MessageCapture {
    /// `None` until the response's status line has arrived; always `None` for requests.
    pub status: Option<u16>,
    /// Header name (lowercased) -> value. `authorization` is already redacted.
    pub headers: Vec<(String, String)>,
    pub body: BodyCapture,
}

/// What we kept of a body, per the rules in `DESIGN.md` §2.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) enum BodyCapture {
    #[default]
    Empty,
    /// Fully captured and parsed as JSON.
    Json(serde_json::Value),
    /// A non-JSON (or unparsable) body: only its shape metadata is kept.
    Opaque {
        content_type: Option<String>,
        len: usize,
    },
    /// A body whose content-type looked like JSON but exceeded the capture
    /// cap: dropped entirely rather than kept as a partial document.
    Truncated { content_type: Option<String> },
}

impl BodyCapture {
    /// Decide what to keep of a captured body, per `DESIGN.md` §2:
    /// JSON content-types are parsed and kept in full; anything else keeps
    /// only its shape (content-type + length); JSON bodies that hit the
    /// capture cap are dropped and marked truncated rather than kept partial.
    pub fn from_captured(content_type: Option<&str>, captured: crate::body::CapturedBody) -> Self {
        let crate::body::CapturedBody { bytes, truncated } = captured;

        if bytes.is_empty() && !truncated {
            return BodyCapture::Empty;
        }

        let looks_like_json = content_type.is_some_and(|ct| {
            let ct = ct.split(';').next().unwrap_or(ct).trim();
            ct.eq_ignore_ascii_case("application/json") || ct.ends_with("+json")
        });

        if truncated {
            return if looks_like_json {
                BodyCapture::Truncated {
                    content_type: content_type.map(str::to_string),
                }
            } else {
                BodyCapture::Opaque {
                    content_type: content_type.map(str::to_string),
                    len: bytes.len(),
                }
            };
        }

        if looks_like_json {
            if let Ok(value) = serde_json::from_slice(&bytes) {
                return BodyCapture::Json(value);
            }
        } else if content_type.is_none() {
            // No content-type at all: best-effort sniff, since plain
            // `(StatusCode, String)` handlers often skip setting one even
            // though the body is JSON text. An explicit non-JSON
            // content-type is never second-guessed this way.
            if let Ok(value) = serde_json::from_slice(&bytes) {
                return BodyCapture::Json(value);
            }
        }

        BodyCapture::Opaque {
            content_type: content_type.map(str::to_string),
            len: bytes.len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::CapturedBody;
    use bytes::Bytes;

    fn captured(bytes: &[u8], truncated: bool) -> CapturedBody {
        CapturedBody {
            bytes: Bytes::copy_from_slice(bytes),
            truncated,
        }
    }

    #[test]
    fn empty_body_is_empty() {
        let cap = BodyCapture::from_captured(None, captured(b"", false));
        assert_eq!(cap, BodyCapture::Empty);
    }

    #[test]
    fn json_content_type_is_parsed() {
        let cap =
            BodyCapture::from_captured(Some("application/json"), captured(br#"{"a":1}"#, false));
        assert_eq!(cap, BodyCapture::Json(serde_json::json!({"a": 1})));
    }

    #[test]
    fn vendor_plus_json_content_type_is_parsed() {
        let cap = BodyCapture::from_captured(
            Some("application/vnd.api+json; charset=utf-8"),
            captured(br#"{"a":1}"#, false),
        );
        assert_eq!(cap, BodyCapture::Json(serde_json::json!({"a": 1})));
    }

    #[test]
    fn non_json_content_type_keeps_shape_only() {
        let cap = BodyCapture::from_captured(Some("image/png"), captured(b"\x89PNG...", false));
        assert_eq!(
            cap,
            BodyCapture::Opaque {
                content_type: Some("image/png".to_string()),
                len: 7,
            }
        );
    }

    #[test]
    fn missing_content_type_falls_back_to_sniffing_json() {
        let cap = BodyCapture::from_captured(None, captured(br#"{"ok":true}"#, false));
        assert_eq!(cap, BodyCapture::Json(serde_json::json!({"ok": true})));
    }

    #[test]
    fn missing_content_type_non_json_bytes_are_opaque() {
        let cap = BodyCapture::from_captured(None, captured(b"plain text", false));
        assert_eq!(
            cap,
            BodyCapture::Opaque {
                content_type: None,
                len: 10,
            }
        );
    }

    #[test]
    fn truncated_json_body_is_dropped_not_kept_partial() {
        let cap = BodyCapture::from_captured(
            Some("application/json"),
            captured(br#"{"a":"not the whole thi"#, true),
        );
        assert_eq!(
            cap,
            BodyCapture::Truncated {
                content_type: Some("application/json".to_string())
            }
        );
    }

    #[test]
    fn truncated_non_json_body_keeps_shape_metadata() {
        let cap =
            BodyCapture::from_captured(Some("application/octet-stream"), captured(b"1234", true));
        assert_eq!(
            cap,
            BodyCapture::Opaque {
                content_type: Some("application/octet-stream".to_string()),
                len: 4,
            }
        );
    }
}
