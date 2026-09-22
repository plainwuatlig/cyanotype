//! §4.6: declared redaction, with no heuristics shipped for *what* to redact
//! (that's always the author's call, via [`Redactions`]) — but a heuristic
//! *warning* for what looks undeclared and dangerous, per the spec's
//! explicit "warn only, never fail" instruction.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use serde_json::Value;

/// Field names (JSON object keys, matched at any depth, in any captured
/// body) whose values are replaced before they can reach storage or output.
///
/// This is coarser than a full per-path declaration (`/user/ssn` vs. just
/// `ssn`) — see `DESIGN.md`/README for why that's the v1 cut. It fails safe
/// in the direction that matters: a name declared once is redacted
/// everywhere it appears, never only in the one place the author had in
/// mind.
#[derive(Debug, Default, Clone)]
pub struct Redactions {
    fields: HashSet<String>,
}

impl Redactions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Redact this field name wherever it appears as a JSON object key.
    #[must_use]
    pub fn field(mut self, name: impl Into<String>) -> Self {
        self.fields.insert(name.into());
        self
    }
}

pub(crate) const REDACTED_VALUE: &str = "[cyanotype:redacted]";

fn config_cell() -> &'static Mutex<Redactions> {
    static CONFIG: OnceLock<Mutex<Redactions>> = OnceLock::new();
    CONFIG.get_or_init(|| Mutex::new(Redactions::default()))
}

/// Register the redaction config for this process.
///
/// Unlike [`crate::record`], this isn't part of the one-line-per-test
/// budget — it's occasional setup, and can be called at any point (it
/// replaces the current config, it doesn't merge with it). Call it before
/// the tests whose bodies need it run: a `configure` racing a concurrently
/// running `#[tokio::test]`'s own recorded request is, like any shared
/// mutable process state, only as ordered as the caller makes it — the
/// straightforward way to avoid that is a `#[ctor]`-style one-time setup or
/// a serial "setup" test, rather than calling it from within an individual
/// `#[tokio::test]` that runs concurrently with others touching the same
/// routes.
pub fn configure(redactions: Redactions) {
    *config_cell()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = redactions;
}

fn config_snapshot() -> Redactions {
    config_cell()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Replace the value of every declared field name, at any depth.
pub(crate) fn redact_value(value: &mut Value) {
    let config = config_snapshot();
    redact_with(&config, value);
}

fn redact_with(config: &Redactions, value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, v) in map.iter_mut() {
                if config.fields.contains(key.as_str()) {
                    *v = Value::String(REDACTED_VALUE.to_string());
                } else {
                    redact_with(config, v);
                }
            }
        }
        Value::Array(items) => {
            for v in items.iter_mut() {
                redact_with(config, v);
            }
        }
        _ => {}
    }
}

// A length floor before entropy is even worth computing, and a
// bits-per-character floor above which a string looks generated rather than
// written (natural-language English sits well below this; API tokens, keys
// and hashes sit at or above it). Both are heuristics, not proof — the
// generator only ever warns on them (§4.6), never fails, and false positives
// on legitimate opaque ids are an accepted, named cost.
const MIN_LENGTH: usize = 20;
const MIN_BITS_PER_CHAR: f64 = 3.5;

pub(crate) fn shannon_entropy_bits_per_char(s: &str) -> f64 {
    let len = s.chars().count();
    if len == 0 {
        return 0.0;
    }
    // This is a heuristic over strings short enough to be HTTP header/body
    // values (nowhere near 2^52 characters), so the `usize -> f64`
    // conversion below never loses meaningful precision in practice.
    #[allow(clippy::cast_precision_loss)]
    let len = len as f64;
    let mut counts = std::collections::HashMap::new();
    for c in s.chars() {
        *counts.entry(c).or_insert(0u32) += 1;
    }
    counts
        .values()
        .map(|&count| {
            let p = f64::from(count) / len;
            -p * p.log2()
        })
        .sum()
}

fn looks_high_entropy(s: &str) -> bool {
    s.len() >= MIN_LENGTH && shannon_entropy_bits_per_char(s) >= MIN_BITS_PER_CHAR
}

/// Warn (via [`crate::store::warn`]) about any string in `value` that looks
/// high-entropy and wasn't redacted. Call this *after* [`redact_value`], so
/// declared fields (now short marker strings) never trigger it.
pub(crate) fn warn_undeclared_high_entropy(value: &Value, context: &str) {
    scan(value, context, &mut String::new());
}

fn scan(value: &Value, context: &str, path: &mut String) {
    match value {
        Value::String(s) if looks_high_entropy(s) => {
            let location = if path.is_empty() {
                "body".to_string()
            } else {
                path.clone()
            };
            crate::store::warn(format!(
                "undeclared high-entropy string at {context} {location} (len {}) — \
                 mark it redacted with `Redactions::field`, or accept it as safe",
                s.len()
            ));
        }
        Value::Object(map) => {
            for (key, v) in map {
                let len_before = path.len();
                if !path.is_empty() {
                    path.push('.');
                }
                path.push_str(key);
                scan(v, context, path);
                path.truncate(len_before);
            }
        }
        Value::Array(items) => {
            use std::fmt::Write;
            for (i, v) in items.iter().enumerate() {
                let len_before = path.len();
                let _ = write!(path, "[{i}]");
                scan(v, context, path);
                path.truncate(len_before);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn declared_field_is_redacted_at_any_depth() {
        let mut redactions = Redactions::new();
        redactions = redactions.field("ssn");
        configure(redactions);

        let mut value = json!({ "user": { "ssn": "123-45-6789" }, "other": "kept" });
        redact_value(&mut value);

        assert_eq!(value["user"]["ssn"], json!(REDACTED_VALUE));
        assert_eq!(value["other"], json!("kept"));
    }

    #[test]
    fn shannon_entropy_ranks_random_looking_strings_above_words() {
        let word = shannon_entropy_bits_per_char("password");
        let token = shannon_entropy_bits_per_char("aK9$mQ2x!pL7@rT4&wZ1");
        assert!(
            token > word,
            "token entropy {token} should exceed word entropy {word}"
        );
    }

    #[test]
    fn short_strings_never_warn_regardless_of_entropy() {
        assert!(!looks_high_entropy("aK9$m"));
    }
}
