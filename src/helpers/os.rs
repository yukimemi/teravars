use std::collections::HashMap;

use serde_json::Value;
use tera::Result;

pub(super) fn is_windows(_args: &HashMap<String, Value>) -> Result<Value> {
    Ok(Value::Bool(cfg!(target_os = "windows")))
}

pub(super) fn is_linux(_args: &HashMap<String, Value>) -> Result<Value> {
    Ok(Value::Bool(cfg!(target_os = "linux")))
}

pub(super) fn is_mac(_args: &HashMap<String, Value>) -> Result<Value> {
    Ok(Value::Bool(cfg!(target_os = "macos")))
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
}
