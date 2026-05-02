use std::collections::HashMap;
use std::process::Command;

use serde_json::Value;
use tera::{Error, Result, Tera};

pub(super) fn register(tera: &mut Tera) {
    tera.register_function("ps", ps_fn);
    tera.register_function("psf", psf_fn);
    tera.register_function("bash", bash_fn);
    tera.register_function("bashf", bashf_fn);
}

#[cfg(windows)]
fn ps_fn(args: &HashMap<String, Value>) -> Result<Value> {
    let script = required_str(args, "script", "ps")?;
    run("powershell", &["-NoProfile", "-Command", script], "ps")
}

#[cfg(not(windows))]
fn ps_fn(_args: &HashMap<String, Value>) -> Result<Value> {
    Err(Error::msg("ps() is only available on Windows targets"))
}

#[cfg(windows)]
fn psf_fn(args: &HashMap<String, Value>) -> Result<Value> {
    let file = required_str(args, "file", "psf")?;
    run("powershell", &["-NoProfile", "-File", file], "psf")
}

#[cfg(not(windows))]
fn psf_fn(_args: &HashMap<String, Value>) -> Result<Value> {
    Err(Error::msg("psf() is only available on Windows targets"))
}

#[cfg(unix)]
fn bash_fn(args: &HashMap<String, Value>) -> Result<Value> {
    let script = required_str(args, "script", "bash")?;
    run("bash", &["-c", script], "bash")
}

#[cfg(not(unix))]
fn bash_fn(_args: &HashMap<String, Value>) -> Result<Value> {
    Err(Error::msg("bash() is only available on Unix targets"))
}

#[cfg(unix)]
fn bashf_fn(args: &HashMap<String, Value>) -> Result<Value> {
    let file = required_str(args, "file", "bashf")?;
    run("bash", &[file], "bashf")
}

#[cfg(not(unix))]
fn bashf_fn(_args: &HashMap<String, Value>) -> Result<Value> {
    Err(Error::msg("bashf() is only available on Unix targets"))
}

fn required_str<'a>(args: &'a HashMap<String, Value>, key: &str, fname: &str) -> Result<&'a str> {
    args.get(key).and_then(|v| v.as_str()).ok_or_else(|| {
        Error::msg(format!(
            "{fname}(): required argument '{key}' missing or not a string"
        ))
    })
}

fn run(program: &str, args: &[&str], fname: &str) -> Result<Value> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| Error::msg(format!("{fname}() failed to spawn '{program}': {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::msg(format!(
            "{fname}() exited with {}: {}",
            output.status,
            stderr.trim()
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout)
        .trim_end_matches(['\n', '\r'])
        .to_string();
    Ok(Value::String(stdout))
}
