//! Numeric filters: stable string hashing + port-range mapping.
//!
//! These exist so callers can write
//! `[vars] dev_port = "{{ vcs.branch | hash | port_offset(start=3000, range=1000) }}"`
//! and get a deterministic, collision-resistant port per branch — the
//! canonical "every worktree fights for :3000" problem.

use std::collections::HashMap;

use serde_json::Value;
use tera::{Error, Result};

/// FNV-1a 64-bit hash of a string. Pure Rust, no dependencies, deterministic
/// across platforms / processes / versions (the constants are part of the
/// FNV-1a spec).
pub(super) fn hash_filter(value: &Value, _args: &HashMap<String, Value>) -> Result<Value> {
    let s = value
        .as_str()
        .ok_or_else(|| Error::msg("`hash` filter requires a string input"))?;
    let h = fnv1a64(s.as_bytes());
    Ok(Value::Number(h.into()))
}

/// Map an integer into `[start, start + range)` deterministically.
/// Typical use: pipe `hash` into `port_offset(start=3000, range=1000)` to
/// get a stable port number from a branch name.
pub(super) fn port_offset_filter(value: &Value, args: &HashMap<String, Value>) -> Result<Value> {
    let n = value
        .as_u64()
        .ok_or_else(|| Error::msg("`port_offset` filter requires a non-negative integer input"))?;

    let start = args.get("start").and_then(|v| v.as_u64()).ok_or_else(|| {
        Error::msg("`port_offset` requires a `start` argument (non-negative integer)")
    })?;
    let range = args.get("range").and_then(|v| v.as_u64()).ok_or_else(|| {
        Error::msg("`port_offset` requires a `range` argument (non-negative integer)")
    })?;

    if range == 0 {
        return Err(Error::msg("`port_offset` `range` must be greater than 0"));
    }

    Ok(Value::Number(((n % range) + start).into()))
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(pairs: &[(&str, Value)]) -> HashMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn hash_is_deterministic_and_distinct() {
        let a = hash_filter(&Value::String("feature/auth".into()), &args(&[])).unwrap();
        let a2 = hash_filter(&Value::String("feature/auth".into()), &args(&[])).unwrap();
        let b = hash_filter(&Value::String("feature/billing".into()), &args(&[])).unwrap();
        assert_eq!(a, a2, "same input → same hash");
        assert_ne!(a, b, "different input → different hash");
    }

    #[test]
    fn hash_handles_empty_string() {
        let h = hash_filter(&Value::String(String::new()), &args(&[])).unwrap();
        assert!(h.as_u64().is_some());
    }

    #[test]
    fn hash_rejects_non_string() {
        let err = hash_filter(&Value::Bool(true), &args(&[])).unwrap_err();
        assert!(err.to_string().contains("string input"));
    }

    #[test]
    fn port_offset_maps_into_range() {
        let v = port_offset_filter(
            &Value::Number(123_456u64.into()),
            &args(&[
                ("start", Value::Number(3000u64.into())),
                ("range", Value::Number(1000u64.into())),
            ]),
        )
        .unwrap();
        let n = v.as_u64().unwrap();
        assert!((3000..4000).contains(&n));
        assert_eq!(n, 3000 + (123_456 % 1000));
    }

    #[test]
    fn port_offset_is_deterministic_via_pipeline() {
        let h = hash_filter(&Value::String("main".into()), &args(&[])).unwrap();
        let p1 = port_offset_filter(
            &h,
            &args(&[
                ("start", Value::Number(8000u64.into())),
                ("range", Value::Number(100u64.into())),
            ]),
        )
        .unwrap();
        let p2 = port_offset_filter(
            &h,
            &args(&[
                ("start", Value::Number(8000u64.into())),
                ("range", Value::Number(100u64.into())),
            ]),
        )
        .unwrap();
        assert_eq!(p1, p2);
        let n = p1.as_u64().unwrap();
        assert!((8000..8100).contains(&n));
    }

    #[test]
    fn port_offset_requires_start_and_range() {
        let h = Value::Number(1u64.into());
        assert!(
            port_offset_filter(&h, &args(&[("range", Value::Number(10u64.into()))]))
                .unwrap_err()
                .to_string()
                .contains("start")
        );
        assert!(
            port_offset_filter(&h, &args(&[("start", Value::Number(0u64.into()))]))
                .unwrap_err()
                .to_string()
                .contains("range")
        );
    }

    #[test]
    fn port_offset_rejects_zero_range() {
        let err = port_offset_filter(
            &Value::Number(1u64.into()),
            &args(&[
                ("start", Value::Number(0u64.into())),
                ("range", Value::Number(0u64.into())),
            ]),
        )
        .unwrap_err();
        assert!(err.to_string().contains("greater than 0"));
    }
}
