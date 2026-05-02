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
mod system;
mod vars;

pub use engine::Engine;
pub use error::Error;
pub use system::{SystemInfo, system_context};
pub use vars::{expand_value, extract_vars, resolve, resolve_with_max_iter};

pub use tera::Context;

pub type Result<T, E = Error> = std::result::Result<T, E>;
