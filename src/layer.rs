//! The recorder itself: a `tower::Layer`/`Service` pair installed via
//! `Router::route_layer` (not `Router::layer`) so that `MatchedPath` is
//! already present in the request extensions by the time [`RecordService`]
//! runs — verified against axum's own routing per `SPEC.md` §3.

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::extract::MatchedPath;
use http::{HeaderMap, Method, Request, Response};
use tower::{Layer, Service};

use crate::auth::capture_headers;
use crate::body::{CAPTURE_CAP_BYTES, CapturedBody, TeeBody};
use crate::declare::Receipt;
use crate::exchange::{BodyCapture, Exchange};
use crate::store::{self, SharedExchange};

/// Installs the recorder. See [`crate::record`] for the public entry point.
#[derive(Clone, Copy, Default)]
pub(crate) struct RecordLayer;

impl<S> Layer<S> for RecordLayer {
    type Service = RecordService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RecordService { inner }
    }
}

#[derive(Clone)]
pub(crate) struct RecordService<S> {
    inner: S,
}

impl<S> Service<Request<Body>> for RecordService<S>
where
    S: Service<Request<Body>, Response = Response<Body>, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Response<Body>, Infallible>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);

        Box::pin(async move {
            let method = req.method().clone();
            let route_template = req.extensions().get::<MatchedPath>().map_or_else(
                // Should not happen when installed via `route_layer`, but a
                // recorder must never be the reason a test fails to route.
                || req.uri().path().to_string(),
                |p| to_openapi_template(p.as_str()),
            );

            let (parts, body) = req.into_parts();
            let request_content_type = content_type_of(&parts.headers);
            let (request_headers, security) = capture_headers(&parts.headers);

            let exchange = Exchange::new(method.clone(), route_template.clone(), security);
            let shared = store::push(exchange);

            let tee_request = {
                let exchange = shared.clone();
                let method = method.clone();
                let route = route_template.clone();
                TeeBody::new(body, CAPTURE_CAP_BYTES, move |captured: CapturedBody| {
                    record_side(
                        &exchange,
                        Side::Request(&method, &route),
                        request_content_type.as_deref(),
                        request_headers,
                        captured,
                    );
                })
            };
            let req = Request::from_parts(parts, Body::new(tee_request));

            let res = inner.call(req).await?;
            let status = res.status().as_u16();
            shared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .response
                .status = Some(status);

            let (mut res_parts, res_body) = res.into_parts();
            let response_content_type = content_type_of(&res_parts.headers);
            let (response_headers, _) = capture_headers(&res_parts.headers);

            let tee_response = {
                let exchange = shared.clone();
                let method = method.clone();
                let route = route_template.clone();
                TeeBody::new(
                    res_body,
                    CAPTURE_CAP_BYTES,
                    move |captured: CapturedBody| {
                        record_side(
                            &exchange,
                            Side::Response(&method, &route),
                            response_content_type.as_deref(),
                            response_headers,
                            captured,
                        );
                    },
                )
            };

            res_parts.extensions.insert(Receipt::new(shared.clone()));

            Ok(Response::from_parts(res_parts, Body::new(tee_response)))
        })
    }
}

#[derive(Clone, Copy)]
enum Side<'a> {
    Request(&'a Method, &'a str),
    Response(&'a Method, &'a str),
}

fn record_side(
    exchange: &SharedExchange,
    side: Side<'_>,
    content_type: Option<&str>,
    headers: Vec<(String, String)>,
    captured: CapturedBody,
) {
    let mut body = BodyCapture::from_captured(content_type, captured);
    let (which, method, route) = match side {
        Side::Request(m, r) => ("request", m, r),
        Side::Response(m, r) => ("response", m, r),
    };
    if let BodyCapture::Json(ref mut value) = body {
        crate::redact::redact_value(value);
        crate::redact::warn_undeclared_high_entropy(value, &format!("{which} {method} {route}"));
    }
    if matches!(body, BodyCapture::Truncated { .. }) {
        store::warn(format!(
            "{which} body for {method} {route} exceeded {}KiB and was not sampled",
            CAPTURE_CAP_BYTES / 1024
        ));
    }

    let mut exchange = exchange
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let target = match side {
        Side::Request(..) => &mut exchange.request,
        Side::Response(..) => &mut exchange.response,
    };
    target.headers = headers;
    target.body = body;
}

fn content_type_of(headers: &HeaderMap) -> Option<String> {
    headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// axum 0.7's `MatchedPath` reports its own routing syntax (`:id`, `*rest`),
/// not OpenAPI's (`{id}`). The first consumer's routes are written with axum 0.7
/// syntax throughout, so this conversion is required, not cosmetic — see
/// `README.md` for why a wildcard segment (`*rest`) is mapped to a single
/// named parameter rather than something OpenAPI has no equivalent for.
fn to_openapi_template(matched: &str) -> String {
    matched
        .split('/')
        .map(|segment| {
            if let Some(name) = segment.strip_prefix(':') {
                format!("{{{name}}}")
            } else if let Some(name) = segment.strip_prefix('*') {
                format!("{{{name}}}")
            } else {
                segment.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::to_openapi_template;

    #[test]
    fn colon_segments_become_curly_brace_parameters() {
        assert_eq!(to_openapi_template("/users/:id"), "/users/{id}");
        assert_eq!(
            to_openapi_template("/api/v2/activities/:id"),
            "/api/v2/activities/{id}"
        );
    }

    #[test]
    fn wildcard_segments_become_a_named_parameter_too() {
        assert_eq!(to_openapi_template("/files/*path"), "/files/{path}");
    }

    #[test]
    fn plain_segments_are_untouched() {
        assert_eq!(to_openapi_template("/healthz"), "/healthz");
    }
}
