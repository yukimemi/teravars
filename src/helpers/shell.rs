use std::process::Command;

use tera::{Error, Kwargs, State, Tera, TeraResult, Value};

pub(super) fn register(tera: &mut Tera) {
    tera.register_function("ps", ps_fn);
    tera.register_function("psf", psf_fn);
    tera.register_function("bash", bash_fn);
    tera.register_function("bashf", bashf_fn);
}

#[cfg(windows)]
fn ps_fn(kwargs: Kwargs, _state: &State) -> TeraResult<Value> {
    let script = required_str(&kwargs, "script", "ps")?;
    run("powershell", &["-NoProfile", "-Command", script], "ps")
}

#[cfg(not(windows))]
fn ps_fn(_kwargs: Kwargs, _state: &State) -> TeraResult<Value> {
    Err(Error::message("ps() is only available on Windows targets"))
}

#[cfg(windows)]
fn psf_fn(kwargs: Kwargs, _state: &State) -> TeraResult<Value> {
    let file = required_str(&kwargs, "file", "psf")?;
    run("powershell", &["-NoProfile", "-File", file], "psf")
}

#[cfg(not(windows))]
fn psf_fn(_kwargs: Kwargs, _state: &State) -> TeraResult<Value> {
    Err(Error::message("psf() is only available on Windows targets"))
}

#[cfg(unix)]
fn bash_fn(kwargs: Kwargs, _state: &State) -> TeraResult<Value> {
    let script = required_str(&kwargs, "script", "bash")?;
    run("bash", &["-c", script], "bash")
}

#[cfg(not(unix))]
fn bash_fn(_kwargs: Kwargs, _state: &State) -> TeraResult<Value> {
    Err(Error::message("bash() is only available on Unix targets"))
}

#[cfg(unix)]
fn bashf_fn(kwargs: Kwargs, _state: &State) -> TeraResult<Value> {
    let file = required_str(&kwargs, "file", "bashf")?;
    run("bash", &[file], "bashf")
}

#[cfg(not(unix))]
fn bashf_fn(_kwargs: Kwargs, _state: &State) -> TeraResult<Value> {
    Err(Error::message("bashf() is only available on Unix targets"))
}

#[cfg(any(windows, unix))]
fn required_str<'a>(kwargs: &'a Kwargs, key: &'a str, fname: &str) -> TeraResult<&'a str> {
    match kwargs.get::<&str>(key)? {
        Some(s) => Ok(s),
        None => Err(Error::message(format!(
            "{fname}(): required argument '{key}' missing or not a string"
        ))),
    }
}

#[cfg(any(windows, unix))]
fn run(program: &str, args: &[&str], fname: &str) -> TeraResult<Value> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| Error::message(format!("{fname}() failed to spawn '{program}': {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::message(format!(
            "{fname}() exited with {}: {}",
            output.status,
            stderr.trim()
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout)
        .trim_end_matches(['\n', '\r'])
        .to_string();
    Ok(Value::from(stdout))
}
