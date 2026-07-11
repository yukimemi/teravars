//! Numeric filters: stable string hashing + port-range mapping.
//!
//! These exist so callers can write
//! `[vars] dev_port = "{{ vcs.branch | hash | port_offset(start=3000, range=1000) }}"`
//! and get a deterministic, collision-resistant port per branch — the
//! canonical "every worktree fights for :3000" problem.

use tera::{Error, Kwargs, State, TeraResult, Value};

/// FNV-1a 64-bit hash of a string. Pure Rust, no dependencies, deterministic
/// across platforms / processes / versions (the constants are part of the
/// FNV-1a spec).
///
/// The input must be a string — Tera's argument coercion rejects non-string
/// values before this function is called.
pub(super) fn hash_filter(value: &str, _kwargs: Kwargs, _state: &State) -> TeraResult<Value> {
    let h = fnv1a64(value.as_bytes());
    Ok(Value::from(h))
}

/// Map an integer into `[start, start + range)` deterministically.
/// Typical use: pipe `hash` into `port_offset(start=3000, range=1000)` to
/// get a stable port number from a branch name.
pub(super) fn port_offset_filter(value: u64, kwargs: Kwargs, _state: &State) -> TeraResult<Value> {
    let start = kwargs.get::<u64>("start")?.ok_or_else(|| {
        Error::message("`port_offset` requires a `start` argument (non-negative integer)")
    })?;
    let range = kwargs.get::<u64>("range")?.ok_or_else(|| {
        Error::message("`port_offset` requires a `range` argument (non-negative integer)")
    })?;

    if range == 0 {
        return Err(Error::message(
            "`port_offset` `range` must be greater than 0",
        ));
    }

    Ok(Value::from((value % range) + start))
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
    use tera::Context;

    fn state<'a>(ctx: &'a Context) -> State<'a> {
        State::new(ctx)
    }

    #[test]
    fn hash_is_deterministic_and_distinct() {
        let ctx = Context::new();
        let s = state(&ctx);
        let a = hash_filter("feature/auth", Kwargs::default(), &s).unwrap();
        let a2 = hash_filter("feature/auth", Kwargs::default(), &s).unwrap();
        let b = hash_filter("feature/billing", Kwargs::default(), &s).unwrap();
        assert_eq!(a, a2, "same input → same hash");
        assert_ne!(a, b, "different input → different hash");
    }

    #[test]
    fn hash_handles_empty_string() {
        let ctx = Context::new();
        let h = hash_filter("", Kwargs::default(), &state(&ctx)).unwrap();
        assert!(h.as_u64().is_some());
    }

    #[test]
    fn hash_rejects_non_string_input() {
        // The `&str` argument type makes Tera coerce/reject non-string inputs
        // before the filter body runs; verify end-to-end through the engine.
        let mut engine = crate::Engine::new();
        let err = engine
            .render("{{ true | hash }}", &Context::new())
            .unwrap_err();
        assert!(
            !err.to_string().is_empty(),
            "piping a non-string into `hash` should error"
        );
    }

    #[test]
    fn port_offset_maps_into_range() {
        let ctx = Context::new();
        let v = port_offset_filter(
            123_456,
            Kwargs::from([
                ("start", Value::from(3000u64)),
                ("range", Value::from(1000u64)),
            ]),
            &state(&ctx),
        )
        .unwrap();
        let n = v.as_u64().unwrap();
        assert!((3000..4000).contains(&n));
        assert_eq!(n, 3000 + (123_456 % 1000));
    }

    #[test]
    fn port_offset_is_deterministic_via_pipeline() {
        let ctx = Context::new();
        let s = state(&ctx);
        let h = hash_filter("main", Kwargs::default(), &s)
            .unwrap()
            .as_u64()
            .unwrap();
        let p1 = port_offset_filter(
            h,
            Kwargs::from([
                ("start", Value::from(8000u64)),
                ("range", Value::from(100u64)),
            ]),
            &s,
        )
        .unwrap();
        let p2 = port_offset_filter(
            h,
            Kwargs::from([
                ("start", Value::from(8000u64)),
                ("range", Value::from(100u64)),
            ]),
            &s,
        )
        .unwrap();
        assert_eq!(p1, p2);
        let n = p1.as_u64().unwrap();
        assert!((8000..8100).contains(&n));
    }

    #[test]
    fn port_offset_requires_start_and_range() {
        let ctx = Context::new();
        let s = state(&ctx);
        assert!(
            port_offset_filter(1, Kwargs::from([("range", Value::from(10u64))]), &s)
                .unwrap_err()
                .to_string()
                .contains("start")
        );
        assert!(
            port_offset_filter(1, Kwargs::from([("start", Value::from(0u64))]), &s)
                .unwrap_err()
                .to_string()
                .contains("range")
        );
    }

    #[test]
    fn port_offset_rejects_zero_range() {
        let ctx = Context::new();
        let err = port_offset_filter(
            1,
            Kwargs::from([("start", Value::from(0u64)), ("range", Value::from(0u64))]),
            &state(&ctx),
        )
        .unwrap_err();
        assert!(err.to_string().contains("greater than 0"));
    }
}
