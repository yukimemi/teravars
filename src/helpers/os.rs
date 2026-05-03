use std::collections::HashMap;

use serde_json::Value;
use tera::{Error, Result};

pub(super) fn is_windows(_args: &HashMap<String, Value>) -> Result<Value> {
    Ok(Value::Bool(cfg!(target_os = "windows")))
}

pub(super) fn is_linux(_args: &HashMap<String, Value>) -> Result<Value> {
    Ok(Value::Bool(cfg!(target_os = "linux")))
}

pub(super) fn is_mac(_args: &HashMap<String, Value>) -> Result<Value> {
    Ok(Value::Bool(cfg!(target_os = "macos")))
}

/// `home()` — the user's home directory as a string. Maps to `dirs::home_dir`,
/// which reads `$HOME` on Unix and `%USERPROFILE%` on Windows. Errors if the
/// platform can't resolve a home directory (rare — the user has bigger
/// problems by then).
pub(super) fn home(_args: &HashMap<String, Value>) -> Result<Value> {
    let path = dirs::home_dir()
        .ok_or_else(|| Error::msg("home(): could not determine the user's home directory"))?;
    let s = path
        .to_str()
        .ok_or_else(|| Error::msg("home(): home directory path is not valid UTF-8"))?;
    Ok(Value::String(s.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_predicates_match_cfg() {
        let no_args: HashMap<String, Value> = HashMap::new();
        assert_eq!(
            is_windows(&no_args).unwrap(),
            Value::Bool(cfg!(target_os = "windows"))
        );
        assert_eq!(
            is_linux(&no_args).unwrap(),
            Value::Bool(cfg!(target_os = "linux"))
        );
        assert_eq!(
            is_mac(&no_args).unwrap(),
            Value::Bool(cfg!(target_os = "macos"))
        );

        let true_count = [
            cfg!(target_os = "windows"),
            cfg!(target_os = "linux"),
            cfg!(target_os = "macos"),
        ]
        .iter()
        .filter(|b| **b)
        .count();
        assert!(true_count <= 1, "at most one OS predicate should be true");
    }

    #[test]
    fn home_returns_a_non_empty_string() {
        let no_args: HashMap<String, Value> = HashMap::new();
        let v = home(&no_args).unwrap();
        let s = v.as_str().expect("home() returns a string");
        assert!(!s.is_empty(), "home() should not be empty on a sane host");
    }
}
