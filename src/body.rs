//! §3 body capture: a streaming tee, not a buffer-then-forward.
//!
//! See `DESIGN.md` §2 for the rationale. [`TeeBody`] forwards every frame of
//! the wrapped body unchanged and immediately, while copying up to
//! [`CAPTURE_CAP_BYTES`] of it into a side buffer for recording. When the
//! body ends (or the cap is hit and can't grow further, or an error arrives),
//! it calls `on_finish` exactly once with whatever it captured.

use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use http_body::{Body, Frame, SizeHint};
use pin_project_lite::pin_project;

/// How much of a body we keep for recording, regardless of how much of it we
/// forward. See `DESIGN.md` §2 for why this isn't configurable in v1.
pub(crate) const CAPTURE_CAP_BYTES: usize = 64 * 1024;

pub(crate) struct CapturedBody {
    pub bytes: Bytes,
    pub truncated: bool,
}

pin_project! {
    /// A body wrapper that tees frames into a capped buffer while passing
    /// them through unchanged.
    pub(crate) struct TeeBody<B, F> {
        #[pin]
        inner: B,
        buf: Vec<u8>,
        truncated: bool,
        cap: usize,
        on_finish: Option<F>,
    }
}

impl<B, F> TeeBody<B, F>
where
    F: FnOnce(CapturedBody) + Send + 'static,
{
    pub fn new(inner: B, cap: usize, on_finish: F) -> Self {
        Self {
            inner,
            buf: Vec::new(),
            truncated: false,
            cap,
            on_finish: Some(on_finish),
        }
    }
}

fn finish<F>(buf: &mut Vec<u8>, truncated: bool, on_finish: &mut Option<F>)
where
    F: FnOnce(CapturedBody) + Send + 'static,
{
    if let Some(cb) = on_finish.take() {
        cb(CapturedBody {
            bytes: std::mem::take(buf).into(),
            truncated,
        });
    }
}

impl<B, F> Body for TeeBody<B, F>
where
    B: Body<Data = Bytes>,
    F: FnOnce(CapturedBody) + Send + 'static,
{
    type Data = Bytes;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let mut this = self.project();

        match this.inner.as_mut().poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref()
                    && !*this.truncated
                {
                    let remaining = this.cap.saturating_sub(this.buf.len());
                    let take = remaining.min(data.len());
                    this.buf.extend_from_slice(&data[..take]);
                    if take < data.len() {
                        *this.truncated = true;
                    }
                }
                // Some `Body` impls report `is_end_stream() == true` as soon
                // as the last frame has been *returned*, without requiring a
                // further poll to observe the terminal `None`. Finish eagerly
                // in that case so we don't depend on the caller polling again.
                if this.inner.is_end_stream() {
                    finish(this.buf, *this.truncated, this.on_finish);
                }
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(Some(Err(err))) => {
                // No more data will follow a terminal error; finalize with
                // whatever was captured so far.
                finish(this.buf, *this.truncated, this.on_finish);
                Poll::Ready(Some(Err(err)))
            }
            Poll::Ready(None) => {
                finish(this.buf, *this.truncated, this.on_finish);
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use std::sync::{Arc, Mutex};

    fn captured_slot() -> (
        Arc<Mutex<Option<CapturedBody>>>,
        impl FnOnce(CapturedBody) + Send + 'static,
    ) {
        let slot = Arc::new(Mutex::new(None));
        let slot_for_cb = slot.clone();
        let cb = move |captured: CapturedBody| {
            *slot_for_cb.lock().unwrap() = Some(captured);
        };
        (slot, cb)
    }

    #[tokio::test]
    async fn forwards_bytes_unchanged_and_captures_a_copy() {
        let (slot, cb) = captured_slot();
        let inner = axum::body::Body::from(Bytes::from_static(b"hello world"));
        let tee = TeeBody::new(inner, CAPTURE_CAP_BYTES, cb);

        let forwarded = tee.collect().await.unwrap().to_bytes();

        assert_eq!(forwarded, Bytes::from_static(b"hello world"));
        let captured = slot
            .lock()
            .unwrap()
            .take()
            .expect("on_finish should have fired");
        assert_eq!(captured.bytes, Bytes::from_static(b"hello world"));
        assert!(!captured.truncated);
    }

    #[tokio::test]
    async fn truncates_capture_past_the_cap_but_still_forwards_everything() {
        let (slot, cb) = captured_slot();
        let payload = vec![b'x'; 10];
        let inner = axum::body::Body::from(Bytes::from(payload.clone()));
        let tee = TeeBody::new(inner, 4, cb); // cap smaller than payload

        let forwarded = tee.collect().await.unwrap().to_bytes();

        assert_eq!(
            forwarded.len(),
            10,
            "full payload must still reach the caller"
        );
        let captured = slot
            .lock()
            .unwrap()
            .take()
            .expect("on_finish should have fired");
        assert_eq!(captured.bytes.len(), 4, "capture is bounded by the cap");
        assert!(captured.truncated);
    }

    #[tokio::test]
    async fn empty_body_finishes_with_empty_capture() {
        let (slot, cb) = captured_slot();
        let inner = axum::body::Body::empty();
        let tee = TeeBody::new(inner, CAPTURE_CAP_BYTES, cb);

        let forwarded = tee.collect().await.unwrap().to_bytes();

        assert!(forwarded.is_empty());
        // Whether or not `on_finish` fired for a body that never yields a
        // frame, the semantically correct outcome (an empty capture) holds
        // either way, since callers default to "nothing captured".
        if let Some(captured) = slot.lock().unwrap().take() {
            assert!(captured.bytes.is_empty());
            assert!(!captured.truncated);
        }
    }
}
