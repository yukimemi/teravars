use tera::{Error, Kwargs, State, TeraResult, Value};

pub(super) fn env_fn(kwargs: Kwargs, _state: &State) -> TeraResult<Value> {
    let name = kwargs
        .get::<&str>("name")?
        .ok_or_else(|| Error::message("env(): required argument 'name' missing or not a string"))?;

    match std::env::var(name) {
        Ok(value) => Ok(Value::from(value)),
        Err(_) => match kwargs.get::<Value>("default")? {
            Some(default) => Ok(default),
            None => Err(Error::message(format!(
                "env(): environment variable '{name}' is not set and no default was provided"
            ))),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tera::Context;

    fn state<'a>(ctx: &'a Context) -> State<'a> {
        State::new(ctx)
    }

    #[test]
    fn env_returns_value_when_set() {
        // SAFETY: test-local env mutation; cargo test serializes within a process per default test
        // harness, but std::env::set_var is unsafe in 2024 edition.
        unsafe {
            std::env::set_var("TERAVARS_TEST_ENV", "abc");
        }
        let ctx = Context::new();
        let v = env_fn(
            Kwargs::from([("name", Value::from("TERAVARS_TEST_ENV"))]),
            &state(&ctx),
        )
        .unwrap();
        assert_eq!(v, Value::from("abc"));
    }

    #[test]
    fn env_uses_default_when_unset() {
        let ctx = Context::new();
        let v = env_fn(
            Kwargs::from([
                ("name", Value::from("TERAVARS_DOES_NOT_EXIST")),
                ("default", Value::from("fallback")),
            ]),
            &state(&ctx),
        )
        .unwrap();
        assert_eq!(v, Value::from("fallback"));
    }

    #[test]
    fn env_errors_when_unset_and_no_default() {
        let ctx = Context::new();
        let err = env_fn(
            Kwargs::from([("name", Value::from("TERAVARS_DOES_NOT_EXIST_2"))]),
            &state(&ctx),
        )
        .unwrap_err();
        assert!(err.to_string().contains("not set"));
    }
}
