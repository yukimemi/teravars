use std::collections::HashMap;

use serde_json::Value;
use tera::{Error, Result};

pub(super) fn env_fn(args: &HashMap<String, Value>) -> Result<Value> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::msg("env(): required argument 'name' missing or not a string"))?;

    match std::env::var(name) {
        Ok(value) => Ok(Value::String(value)),
        Err(_) => match args.get("default") {
            Some(default) => Ok(default.clone()),
            None => Err(Error::msg(format!(
                "env(): environment variable '{name}' is not set and no default was provided"
            ))),
        },
    }
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
    fn env_returns_value_when_set() {
        // SAFETY: test-local env mutation; cargo test serializes within a process per default test
        // harness, but std::env::set_var is unsafe in 2024 edition.
        unsafe {
            std::env::set_var("TERAVARS_TEST_ENV", "abc");
        }
        let v = env_fn(&args(&[(
            "name",
            Value::String("TERAVARS_TEST_ENV".into()),
        )]))
        .unwrap();
        assert_eq!(v, Value::String("abc".into()));
    }

    #[test]
    fn env_uses_default_when_unset() {
        let v = env_fn(&args(&[
            ("name", Value::String("TERAVARS_DOES_NOT_EXIST".into())),
            ("default", Value::String("fallback".into())),
        ]))
        .unwrap();
        assert_eq!(v, Value::String("fallback".into()));
    }

    #[test]
    fn env_errors_when_unset_and_no_default() {
        let err = env_fn(&args(&[(
            "name",
            Value::String("TERAVARS_DOES_NOT_EXIST_2".into()),
        )]))
        .unwrap_err();
        assert!(err.to_string().contains("not set"));
    }
}
