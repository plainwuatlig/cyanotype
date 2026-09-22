//! §4.5: derive a security scheme from the `Authorization` header, and strip
//! its value unconditionally, at the earliest possible point.
//!
//! [`capture_headers`] is called from `layer.rs` on the raw [`http::HeaderMap`]
//! before an [`crate::exchange::Exchange`] is constructed. The real header
//! value is used only inside this function, to decide `Bearer` vs not; it is
//! never copied into the `Vec<(String, String)>` this function returns. That
//! makes the strip structural rather than a step someone downstream could
//! forget: there is no code path from "captured headers" to "output" that
//! ever sees the raw value, in tests, fixtures, or the emitted document.

use http::HeaderMap;

pub(crate) const REDACTED_MARKER: &str = "[cyanotype:redacted]";
const NON_UTF8_MARKER: &str = "[cyanotype:non-utf8]";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SchemeKind {
    /// `Authorization: Bearer <token>` -> `{"type": "http", "scheme": "bearer"}`.
    Bearer,
    /// `Authorization: <anything else>` -> `{"type": "apiKey", "in": "header", "name": "Authorization"}`.
    ApiKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SecurityScheme {
    pub header_name: &'static str,
    pub kind: SchemeKind,
}

impl SecurityScheme {
    /// The OpenAPI `components.securitySchemes` key for this scheme.
    pub fn component_name(&self) -> &'static str {
        match self.kind {
            SchemeKind::Bearer => "bearerAuth",
            SchemeKind::ApiKey => "apiKeyAuth",
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        match self.kind {
            SchemeKind::Bearer => serde_json::json!({ "type": "http", "scheme": "bearer" }),
            SchemeKind::ApiKey => {
                serde_json::json!({ "type": "apiKey", "in": "header", "name": self.header_name })
            }
        }
    }
}

/// Turn a raw header map into the redacted `(name, value)` pairs an
/// [`crate::exchange::MessageCapture`] stores, deriving a security scheme
/// along the way if an `Authorization` header was present.
///
/// Only `Authorization` is treated as a candidate auth header in v1 — see
/// `DESIGN.md`/README for why a custom header like `X-Api-Key` is not
/// detected.
pub(crate) fn capture_headers(
    headers: &HeaderMap,
) -> (Vec<(String, String)>, Option<SecurityScheme>) {
    let mut captured = Vec::with_capacity(headers.len());
    let mut scheme = None;

    for (name, value) in headers {
        let name_lower = name.as_str().to_ascii_lowercase();
        if name_lower == "authorization" {
            let kind = match value.to_str() {
                Ok(v) if v.starts_with("Bearer ") => SchemeKind::Bearer,
                _ => SchemeKind::ApiKey,
            };
            scheme = Some(SecurityScheme {
                header_name: "Authorization",
                kind,
            });
            captured.push((name_lower, REDACTED_MARKER.to_string()));
            continue;
        }

        let value_string = value
            .to_str()
            .map_or_else(|_| NON_UTF8_MARKER.to_string(), str::to_string);
        captured.push((name_lower, value_string));
    }

    (captured, scheme)
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderValue;

    #[test]
    fn bearer_token_is_redacted_and_detected_as_http_bearer() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "Authorization",
            HeaderValue::from_static("Bearer super-secret-token"),
        );

        let (captured, scheme) = capture_headers(&headers);

        assert_eq!(
            captured,
            vec![("authorization".to_string(), REDACTED_MARKER.to_string())]
        );
        let scheme = scheme.expect("scheme should be derived");
        assert_eq!(scheme.kind, SchemeKind::Bearer);
        assert_eq!(
            scheme.to_json(),
            serde_json::json!({ "type": "http", "scheme": "bearer" })
        );
    }

    #[test]
    fn non_bearer_authorization_is_redacted_and_detected_as_api_key() {
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", HeaderValue::from_static("raw-api-key-123"));

        let (captured, scheme) = capture_headers(&headers);

        assert_eq!(
            captured,
            vec![("authorization".to_string(), REDACTED_MARKER.to_string())]
        );
        let scheme = scheme.expect("scheme should be derived");
        assert_eq!(scheme.kind, SchemeKind::ApiKey);
        assert_eq!(
            scheme.to_json(),
            serde_json::json!({ "type": "apiKey", "in": "header", "name": "Authorization" })
        );
    }

    #[test]
    fn no_authorization_header_means_no_scheme_and_other_headers_pass_through() {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));

        let (captured, scheme) = capture_headers(&headers);

        assert_eq!(
            captured,
            vec![("content-type".to_string(), "application/json".to_string())]
        );
        assert!(scheme.is_none());
    }

    #[test]
    fn authorization_header_name_is_matched_case_insensitively() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer x"));

        let (captured, scheme) = capture_headers(&headers);

        assert_eq!(captured[0].1, REDACTED_MARKER);
        assert!(scheme.is_some());
    }
}
