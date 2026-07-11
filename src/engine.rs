use tera::{Context, Function, FunctionResult, Tera};

use crate::Result;
use crate::helpers;

pub struct Engine {
    tera: Tera,
}

impl Engine {
    pub fn new() -> Self {
        let mut tera = Tera::default();
        helpers::register_default(&mut tera);
        Self { tera }
    }

    pub fn new_minimal() -> Self {
        Self {
            tera: Tera::default(),
        }
    }

    pub fn register_function<F, Res>(&mut self, name: &str, f: F)
    where
        F: Function<Res>,
        Res: FunctionResult,
    {
        self.tera.register_function(name.to_string(), f);
    }

    pub fn render(&mut self, src: &str, ctx: &Context) -> Result<String> {
        // autoescape is disabled: these templates render TOML config values
        // (paths, commands, URLs), not HTML — escaping `&`/`<`/`>` would corrupt them.
        Ok(self.tera.render_str(src, ctx, false)?)
    }

    pub fn tera_mut(&mut self) -> &mut Tera {
        &mut self.tera
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_substitutes_variable() {
        let mut engine = Engine::new();
        let mut ctx = Context::new();
        ctx.insert("name", "world");
        let out = engine.render("hello {{ name }}", &ctx).unwrap();
        assert_eq!(out, "hello world");
    }

    #[test]
    fn new_minimal_has_no_default_helpers() {
        let mut engine = Engine::new_minimal();
        let ctx = Context::new();
        let err = engine
            .render(r#"{{ env(name="PATH") }}"#, &ctx)
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("env") || msg.contains("not found"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn register_function_overrides_helper() {
        use tera::{Kwargs, State, TeraResult, Value};

        let mut engine = Engine::new();
        engine.register_function(
            "custom",
            |_kwargs: Kwargs, _state: &State| -> TeraResult<Value> { Ok(Value::from("ok")) },
        );
        let out = engine.render("{{ custom() }}", &Context::new()).unwrap();
        assert_eq!(out, "ok");
    }

    #[cfg(feature = "std-helpers")]
    #[test]
    fn hash_and_port_offset_filters_work_end_to_end() {
        let mut engine = Engine::new();
        let mut ctx = Context::new();
        ctx.insert("branch", "feature/auth");

        // Same input always renders to the same number.
        let p1 = engine
            .render(
                r#"{{ branch | hash | port_offset(start=3000, range=1000) }}"#,
                &ctx,
            )
            .unwrap();
        let p2 = engine
            .render(
                r#"{{ branch | hash | port_offset(start=3000, range=1000) }}"#,
                &ctx,
            )
            .unwrap();
        assert_eq!(p1, p2, "deterministic across renders");

        let n: u64 = p1.parse().expect("port should be a u64-shaped string");
        assert!(
            (3000..4000).contains(&n),
            "port {n} should land in [3000, 4000)"
        );

        // Different input should produce a different (very likely) port.
        ctx.insert("branch", "feature/billing");
        let p3 = engine
            .render(
                r#"{{ branch | hash | port_offset(start=3000, range=1000) }}"#,
                &ctx,
            )
            .unwrap();
        assert_ne!(
            p1, p3,
            "different branch → different port (collision unlikely)"
        );
    }
}
