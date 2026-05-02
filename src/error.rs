use std::error::Error as StdError;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("template render failed: {0}")]
    Render(String),

    #[error("vars resolution did not converge after {iterations} iterations")]
    ResolveNotConverged { iterations: usize },

    #[error("vars extraction failed: {0}")]
    Extract(String),

    #[error(transparent)]
    Toml(#[from] toml::de::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl From<tera::Error> for Error {
    fn from(err: tera::Error) -> Self {
        let mut messages = vec![err.to_string()];
        let mut current: &dyn StdError = &err;
        while let Some(source) = current.source() {
            messages.push(source.to_string());
            current = source;
        }
        Error::Render(messages.join(": "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tera_error_chain_is_flattened_into_render_message() {
        let mut tera = tera::Tera::default();
        let err = tera
            .render_str("{{ vars.missing }}", &tera::Context::new())
            .unwrap_err();
        let converted: Error = err.into();
        let msg = converted.to_string();

        assert!(msg.contains("template render failed"));
        // The whole point of walking err.source(): the actual cause must reach the user,
        // not just the bare top-level "Failed to render '__tera_one_off'" message.
        assert!(
            msg.contains("vars.missing"),
            "expected the missing-variable name to be surfaced, got: {msg}"
        );
        assert!(
            msg.contains("not found"),
            "expected the cause description to be surfaced, got: {msg}"
        );
    }
}
