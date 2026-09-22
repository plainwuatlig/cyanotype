//! Process-global storage for recorded exchanges and generator warnings.
//!
//! One `cargo test` binary is one process, so this is scoped exactly to "one
//! test run" — the granularity the spec's two-call API (`record` /
//! `collected`) assumes. See `DESIGN.md` for why a `Receipt` (not a global
//! lookup) is used to correlate a specific declaration back to a specific
//! exchange despite tests running concurrently on this shared store.

use std::sync::{Arc, Mutex, OnceLock};

use crate::exchange::Exchange;

pub(crate) type SharedExchange = Arc<Mutex<Exchange>>;

fn exchanges() -> &'static Mutex<Vec<SharedExchange>> {
    static EXCHANGES: OnceLock<Mutex<Vec<SharedExchange>>> = OnceLock::new();
    EXCHANGES.get_or_init(|| Mutex::new(Vec::new()))
}

fn warnings() -> &'static Mutex<Vec<String>> {
    static WARNINGS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    WARNINGS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Register a newly-started exchange (headers captured, bodies still filling in).
pub(crate) fn push(exchange: Exchange) -> SharedExchange {
    let shared = Arc::new(Mutex::new(exchange));
    exchanges()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(shared.clone());
    shared
}

/// A snapshot of every exchange recorded so far in this process.
pub(crate) fn snapshot() -> Vec<Exchange> {
    exchanges()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .map(|shared| {
            shared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        })
        .collect()
}

pub(crate) fn warn(message: impl Into<String>) {
    warnings()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(message.into());
}

pub(crate) fn warnings_snapshot() -> Vec<String> {
    warnings()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::Method;

    #[test]
    fn pushed_exchange_is_visible_in_snapshot() {
        let before = snapshot().len();
        push(Exchange::new(
            Method::GET,
            "/store-test-marker".into(),
            None,
        ));
        let after = snapshot();
        assert_eq!(after.len(), before + 1);
        assert!(
            after
                .iter()
                .any(|e| e.route_template == "/store-test-marker")
        );
    }

    #[test]
    fn warnings_accumulate() {
        let before = warnings_snapshot().len();
        warn("store-test warning");
        assert_eq!(warnings_snapshot().len(), before + 1);
    }
}
