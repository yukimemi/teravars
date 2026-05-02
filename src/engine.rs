use tera::{Context, Function, Tera};

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

    pub fn register_function<F>(&mut self, name: &str, f: F)
    where
        F: Function + 'static,
    {
        self.tera.register_function(name, f);
    }

    pub fn render(&mut self, src: &str, ctx: &Context) -> Result<String> {
        Ok(self.tera.render_str(src, ctx)?)
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
        let mut engine = Engine::new();
        engine.register_function("custom", |_args: &_| Ok("ok".into()));
        let out = engine.render("{{ custom() }}", &Context::new()).unwrap();
        assert_eq!(out, "ok");
    }
}
