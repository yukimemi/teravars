//! Tera templating + smart `[vars]` handling for self-rendering TOML configs.
//!
//! See `README.md` and `ROADMAP.md` for the design rationale and migration plan.
//!
//! # Quickstart
//!
//! ```
//! use teravars::{Context, Engine, extract_vars, resolve, system_context};
//!
//! let raw = r#"
//! [vars]
//! greeting = "hello"
//! who      = "{{ vars.greeting }} world"
//!
//! [server]
//! banner = "{{ vars.who }} on {{ system.os }}"
//! "#;
//!
//! let mut engine = Engine::new();
//!
//! let mut vars = extract_vars(raw).unwrap();
//! resolve(&mut vars, &mut engine).unwrap();
//!
//! let mut ctx: Context = system_context();
//! ctx.insert("vars", &vars);
//!
//! let rendered = engine.render(raw, &ctx).unwrap();
//! assert!(rendered.contains("hello world on "));
//! ```

mod engine;
mod error;
mod helpers;
#[cfg(feature = "merge")]
mod merge;
mod system;
mod vars;

pub use engine::Engine;
pub use error::Error;
pub use system::{SystemInfo, system_context};
pub use vars::{
    expand_value, extract_vars, resolve, resolve_in_context, resolve_in_context_with_max_iter,
    resolve_with_max_iter,
};

#[cfg(feature = "merge")]
pub use merge::{MergedConfig, deep_merge, discover_config_files, load_merged};

pub use tera::Context;

/// tera's error type, re-exported under an aliased name so it doesn't clash
/// with teravars' own [`Error`]. Use it to construct errors from custom
/// functions/filters (`TeraError::message("…")`).
pub use tera::Error as TeraError;
/// Re-exports of tera's function/filter authoring types, so consumers can
/// register custom helpers via [`Engine::register_function`] (or
/// [`Engine::tera_mut`]) without taking a direct `tera` dependency.
///
/// A custom function is `Fn(Kwargs, &State) -> TeraResult<Value>`; extract
/// arguments with [`Kwargs::get`] / [`Kwargs::must_get`]. tera's own error type
/// is re-exported as [`TeraError`] to avoid clashing with teravars' [`Error`].
///
/// ```
/// use teravars::{Context, Engine, Kwargs, State, TeraError, TeraResult, Value};
///
/// fn shout(kwargs: Kwargs, _state: &State) -> TeraResult<Value> {
///     let text: &str = kwargs
///         .get("text")?
///         .ok_or_else(|| TeraError::message("shout(): `text` is required"))?;
///     Ok(Value::from(text.to_uppercase()))
/// }
///
/// let mut engine = Engine::new_minimal();
/// engine.register_function("shout", shout);
/// let out = engine.render(r#"{{ shout(text="hi") }}"#, &Context::new()).unwrap();
/// assert_eq!(out, "HI");
/// ```
///
/// Custom filters work the same way — `Fn(Arg, Kwargs, &State) -> Res`, where
/// `Arg` is any [`ArgFromValue`] type (`&str`, `i64`, …) — registered through
/// [`Engine::tera_mut`]:
///
/// ```
/// use teravars::{Context, Engine, Kwargs, State, TeraResult, Value};
///
/// fn exclaim(value: &str, _kwargs: Kwargs, _state: &State) -> TeraResult<Value> {
///     Ok(Value::from(format!("{value}!")))
/// }
///
/// let mut engine = Engine::new_minimal();
/// engine.tera_mut().register_filter("exclaim", exclaim);
/// let out = engine.render(r#"{{ "hi" | exclaim }}"#, &Context::new()).unwrap();
/// assert_eq!(out, "hi!");
/// ```
pub use tera::{
    ArgFromValue, Filter, Function, FunctionResult, Kwargs, Map, Number, State, TeraResult, Value,
};

pub type Result<T, E = Error> = std::result::Result<T, E>;
