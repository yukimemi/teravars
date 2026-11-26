use std::collections::{HashSet, hash_map::RandomState};
use std::hash::{BuildHasher, Hasher};
use std::path::Path;
use std::sync::{Arc, Mutex};

use toml::{Table, Value};

use crate::comments::{tera_tag_end, trim_tera_tag};
use crate::{Error, Result};

/// Opt-in runtime-only root variable names for [`super::load_merged_with`].
#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    names: HashSet<String>,
    prefixes: Vec<String>,
}

impl LoadOptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Defer output expressions freely referencing this exact root name.
    pub fn defer(mut self, name: impl Into<String>) -> Self {
        self.names.insert(name.into());
        self
    }

    /// Defer root names with this prefix, e.g. `file_` for shun.
    pub fn defer_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefixes.push(prefix.into());
        self
    }

    fn validate(&self) -> Result<()> {
        for (kind, value) in self
            .names
            .iter()
            .map(|n| ("name", n))
            .chain(self.prefixes.iter().map(|p| ("prefix", p)))
        {
            if !ident(value)
                || !value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                return Err(Error::Extract(format!(
                    "invalid deferred variable {kind} `{value}`: expected [A-Za-z_][A-Za-z0-9_]*"
                )));
            }
        }
        Ok(())
    }

    fn matches(&self, name: &str) -> bool {
        self.names.contains(name) || self.prefixes.iter().any(|p| name.starts_with(p))
    }
}

const MARKER_END: char = '\u{e001}';

pub(super) struct Deferred<'a> {
    options: &'a LoadOptions,
    prefix: String,
    function_name: String,
    tags: Vec<String>,
    analyzer: Option<tera::Tera>,
}

pub(super) struct Emissions {
    counts: Arc<Mutex<Vec<usize>>>,
    prefix: String,
}

impl Emissions {
    fn normalize(&self, table: &mut Table, persistent_prefix: &str) -> Result<()> {
        let counts = self.counts.lock().unwrap();
        let mut seen = vec![0; counts.len()];
        fn walk(
            value: &mut Value,
            prefix: &str,
            persistent: &str,
            seen: &mut [usize],
        ) -> Result<()> {
            match value {
                Value::String(s) => {
                    if !s.contains(prefix) {
                        return Ok(());
                    }
                    let mut output = String::new();
                    let mut rest = s.as_str();
                    while let Some(i) = rest.find(prefix) {
                        output.push_str(&rest[..i]);
                        let Some((id, tail)) = rest[i + prefix.len()..].split_once(MARKER_END)
                        else {
                            return Err(Error::Render("corrupted runtime variable marker".into()));
                        };
                        let Some(index) = id.parse::<usize>().ok().filter(|i| *i < seen.len())
                        else {
                            return Err(Error::Render("unknown runtime variable marker".into()));
                        };
                        seen[index] += 1;
                        output.push_str(persistent);
                        output.push_str(id);
                        output.push(MARKER_END);
                        rest = tail;
                    }
                    output.push_str(rest);
                    *s = output;
                }
                Value::Array(a) => {
                    for v in a {
                        walk(v, prefix, persistent, seen)?;
                    }
                }
                Value::Table(t) => {
                    for (key, v) in t.iter_mut() {
                        if key.contains(prefix) {
                            return Err(Error::Render(
                                "runtime variables are not supported in TOML keys or headers"
                                    .into(),
                            ));
                        }
                        walk(v, prefix, persistent, seen)?;
                    }
                }
                _ => {}
            }
            Ok(())
        }
        for (key, value) in table.iter_mut() {
            if key.contains(&self.prefix) {
                return Err(Error::Render(
                    "runtime variables are not supported in TOML keys or headers".into(),
                ));
            }
            walk(value, &self.prefix, persistent_prefix, &mut seen)?;
        }
        if seen != *counts {
            return Err(Error::Render(
                "runtime variable marker was lost, duplicated or rendered outside a string value"
                    .into(),
            ));
        }
        Ok(())
    }
}

struct Frame<'a> {
    kind: &'a str,
    locals: HashSet<String>,
    possible: HashSet<String>,
    branches: Vec<HashSet<String>>,
    live: bool,
    fallthrough: bool,
    capture: Option<String>,
}

impl<'a> Frame<'a> {
    fn new(kind: &'a str, locals: HashSet<String>) -> Self {
        Self {
            kind,
            locals,
            possible: HashSet::new(),
            branches: Vec::new(),
            live: true,
            fallthrough: true,
            capture: None,
        }
    }

    fn condition(&mut self, condition: &str) {
        self.live = self.fallthrough && condition != "false";
        self.fallthrough &= condition != "true";
    }

    fn next_branch(&mut self, condition: &str) {
        if self.kind == "if" {
            let locals = std::mem::take(&mut self.locals);
            if self.live {
                self.branches.push(locals);
            }
            self.condition(condition);
        } else if self.kind == "for" {
            self.locals.clear();
        }
    }

    fn finish(mut self, parent: &mut Self) {
        if self.kind == "if" {
            if self.live {
                self.branches.push(self.locals);
            }
            let mut definite = if self.fallthrough {
                HashSet::new()
            } else {
                self.branches.first().cloned().unwrap_or_default()
            };
            for branch in &self.branches {
                definite.retain(|name| branch.contains(name));
                self.possible.extend(branch.iter().cloned());
            }
            parent.locals.extend(definite);
            parent.possible.extend(self.possible);
        }
        if let Some(name) = self.capture {
            parent.locals.insert(name);
        }
    }
}

impl<'a> Deferred<'a> {
    pub(super) fn new(options: &'a LoadOptions, engine: &mut crate::Engine) -> Result<Self> {
        options.validate()?;
        let nonce = RandomState::new().build_hasher().finish();
        let (prefix, analyzer) = if options.names.is_empty() && options.prefixes.is_empty() {
            (String::new(), None)
        } else {
            let mut analyzer = tera::Tera::default();
            analyzer.register_from(engine.tera_mut());
            (format!("\u{e000}{nonce}_"), Some(analyzer))
        };
        Ok(Self {
            options,
            prefix,
            function_name: format!("__teravars_emit_{nonce:016x}"),
            tags: Vec::new(),
            analyzer,
        })
    }

    pub(super) fn active(&self) -> bool {
        !self.options.names.is_empty() || !self.options.prefixes.is_empty()
    }

    // Keep markers opaque through both vars resolution passes. The original
    // tags are only restored into parsed string values after the final pass.
    pub(super) fn protect(&mut self, text: &str, path: &Path, allow: bool) -> Result<String> {
        self.protect_impl(text, path, allow, true, None)
    }

    fn protect_impl(
        &mut self,
        text: &str,
        path: &Path,
        allow: bool,
        document: bool,
        ctx: Option<&tera::Context>,
    ) -> Result<String> {
        if !self.active() {
            return Ok(text.to_owned());
        }
        if document && ctx.is_none() && text.contains(&self.prefix) {
            return Err(Error::Render(format!(
                "{}: deferred marker collision",
                path.display()
            )));
        }
        let mut out = String::with_capacity(text.len());
        let mut pos = 0;
        let mut quote = TomlQuote::default();
        let mut frames = vec![Frame::new("root", HashSet::new())];
        while let Some(offset) = text[pos..].find('{') {
            let start = pos + offset;
            quote.scan(&text[pos..start]);
            out.push_str(&text[pos..start]);
            let tail = &text[start..];
            let (kind, close): (_, &[u8]) = if tail.starts_with("{{") {
                ('{', b"}}")
            } else if tail.starts_with("{%") {
                ('%', b"%}")
            } else if tail.starts_with("{#") {
                ('#', b"#}")
            } else {
                quote.scan("{");
                out.push('{');
                pos = start + 1;
                continue;
            };
            let end = if kind == '#' {
                tail[2..].find("#}").map(|i| i + 2)
            } else {
                tera_tag_end(&tail[2..], close)
            };
            let Some(end) = end else {
                out.push_str(tail);
                return Ok(out);
            };
            let end = start + 2 + end;
            let tag = &text[start..end];
            let body = trim_tera_tag(&tag[2..tag.len() - 2]);
            let words = tokens(body);
            let first = words.first().copied().unwrap_or("");
            if kind == '%' && first == "raw" {
                let (body_end, resume) = super::include::raw_block_end(text, end);
                quote.scan(&text[end..body_end]);
                out.push_str(&text[start..resume]);
                pos = resume;
                continue;
            } else if kind != '#' {
                if kind == '%' && matches!(first, "else" | "elif") {
                    let condition = if first == "else" {
                        "true"
                    } else {
                        body.strip_prefix("elif").unwrap_or("").trim()
                    };
                    frames.last_mut().unwrap().next_branch(condition);
                }
                let mut expr = words.as_slice();
                if kind == '%' {
                    expr = match first {
                        "for" => words
                            .iter()
                            .position(|w| *w == "in")
                            .map_or(&[][..], |i| &words[i + 1..]),
                        "set" | "set_global" if words.get(2) == Some(&"=") => {
                            words.get(3..).unwrap_or_default()
                        }
                        "set" | "set_global" => words.get(2..).unwrap_or_default(),
                        "macro" | "component" | "endcomponent" | "endmacro" | "block"
                        | "endblock" => &[],
                        _ => words.get(1..).unwrap_or_default(),
                    };
                }
                // Let Tera classify expression references, including comprehension
                // bindings, tests, named arguments and nested indexing. For
                // statement fragments or runtime-only helpers not registered on
                // this engine, use the lexical classification.
                let expression = expr
                    .first()
                    .map(|first| {
                        let offset = first.as_ptr() as usize - body.as_ptr() as usize;
                        &body[offset..]
                    })
                    .unwrap_or("");
                let candidate = if kind == '%' && first == "filter" {
                    format!("{{{{ \"\" | {expression} }}}}")
                } else {
                    format!("{{{{ {expression} }}}}")
                };
                let analyzer = self.analyzer.as_mut().unwrap();
                let parsed = analyzer.add_raw_template("expression", &candidate);
                let refs = if parsed.is_ok() {
                    analyzer
                        .get_template_variables("expression")
                        .unwrap_or_default()
                        .into_iter()
                        .collect::<Vec<_>>()
                } else {
                    let expr = if kind == '%' && first == "filter" {
                        expr.get(1..).unwrap_or_default()
                    } else {
                        expr
                    };
                    references(expr)
                };
                let bound = |name: &str| frames.iter().any(|f| f.locals.contains(name));
                let possible = |name: &str| frames.iter().any(|f| f.possible.contains(name));
                if let Some(name) = refs
                    .iter()
                    .find(|n| self.options.matches(n) && !bound(n) && possible(n))
                {
                    return Err(Error::Render(format!(
                        "{}: runtime variable `{name}` has an ambiguous load-time binding",
                        path.display()
                    )));
                }
                if let Some(name) = refs.iter().find(|n| self.options.matches(n) && !bound(n)) {
                    let line = text[..start].bytes().filter(|b| *b == b'\n').count() + 1;
                    let fail = |why| {
                        Error::Render(format!(
                            "{}:{line}: runtime variable `{name}` {why}",
                            path.display()
                        ))
                    };
                    if kind == '{'
                        && let Err(err) = &parsed
                        && matches!(err.kind(), tera::ErrorKind::SyntaxError(_))
                    {
                        return Err(Error::Render(format!("{}:{line}: {err}", path.display())));
                    }
                    if kind != '{' || !allow {
                        return Err(fail("is not supported in this context"));
                    }
                    if document && (quote.delimiter == 0 || !quote.value) {
                        return Err(fail("must occur in a TOML string value"));
                    }
                    if refs.iter().any(|n| bound(n) || possible(n)) {
                        return Err(fail("cannot be mixed with a load-time local variable"));
                    }
                    if frames
                        .iter()
                        .any(|f| matches!(f.kind, "filter" | "set" | "macro" | "component"))
                    {
                        return Err(fail("is not supported inside a capture or component"));
                    }
                    let marker = format!("{}{}{MARKER_END}", self.prefix, self.tags.len());
                    self.tags.push(tag.to_owned());
                    let left = if tag.starts_with("{{-") { "-" } else { "" };
                    let right = if tag.ends_with("-}}") { "-" } else { "" };
                    let newlines = "\n".repeat(tag.matches('\n').count());
                    out.push_str(&format!("{{{{{left} `{marker}`{newlines} {right}}}}}"));
                    pos = end;
                    continue;
                }
                if let Some(ctx) = ctx {
                    let simple_copy = kind == '{'
                        && (!document || (quote.delimiter != 0 && quote.value))
                        && !frames
                            .iter()
                            .any(|f| matches!(f.kind, "filter" | "set" | "macro" | "component"));
                    if indirect_use(
                        expr,
                        &refs,
                        ctx,
                        &self.prefix,
                        self.options,
                        &bound,
                        simple_copy,
                    ) {
                        let line = text[..start].bytes().filter(|b| *b == b'\n').count() + 1;
                        return Err(Error::Render(format!(
                            "{}:{line}: runtime variable value may only be copied directly into a string; defer the original expression instead",
                            path.display()
                        )));
                    }
                }
                if kind == '%' {
                    match first {
                        "for" => {
                            let mut locals = HashSet::from(["loop".to_owned()]);
                            for w in words[1..].iter().take_while(|w| **w != "in") {
                                if ident(w) {
                                    locals.insert((*w).to_owned());
                                }
                            }
                            frames.push(Frame::new("for", locals));
                        }
                        "if" | "filter" | "block" | "macro" | "component" => {
                            let mut locals = HashSet::new();
                            if matches!(first, "macro" | "component") {
                                for pair in words.windows(2) {
                                    if matches!(pair[1], ":" | "=" | "," | ")") && ident(pair[0]) {
                                        locals.insert(pair[0].to_owned());
                                    }
                                }
                            }
                            let mut frame = Frame::new(first, locals);
                            if first == "if" {
                                frame.condition(expression.trim());
                            }
                            frames.push(frame);
                        }
                        "set" | "set_global" => {
                            if let Some(name) = words.get(1) {
                                if words.get(2) == Some(&"=") {
                                    // Global writes executed conditionally may or may not
                                    // survive the enclosing block.
                                    if first == "set_global" && frames.len() > 1 {
                                        frames[0].possible.insert((*name).to_owned());
                                    }
                                    frames.last_mut().unwrap().locals.insert((*name).to_owned());
                                } else {
                                    let mut frame = Frame::new("set", HashSet::new());
                                    frame.capture = Some((*name).to_owned());
                                    frames.push(frame);
                                }
                            }
                        }
                        "endfor" | "endif" | "endfilter" | "endblock" | "endmacro"
                        | "endcomponent" | "endset"
                            if frames.len() > 1 =>
                        {
                            let frame = frames.pop().unwrap();
                            frame.finish(frames.last_mut().unwrap());
                        }
                        _ => {}
                    }
                }
            }
            out.push_str(tag);
            pos = end;
        }
        out.push_str(&text[pos..]);
        Ok(out)
    }

    pub(super) fn resolve(
        &mut self,
        vars: &mut Table,
        engine: &mut crate::Engine,
        ctx: &tera::Context,
        path: &Path,
        best_effort: bool,
    ) -> Result<()> {
        if !self.active() {
            let result = crate::vars::resolve_in_context(vars, engine, ctx);
            return if best_effort { Ok(()) } else { result };
        }
        let mut invalid = false;
        let result = crate::vars::resolve_with_renderer(
            vars,
            ctx,
            crate::vars::DEFAULT_MAX_RESOLVE_ITERATIONS,
            |text, ctx| {
                if !text.contains("{{") && !text.contains("{%") {
                    return engine.render(text, ctx);
                }
                let protected = self
                    .protect_impl(text, path, true, false, Some(ctx))
                    .inspect_err(|_| invalid = true)?;
                engine.render(&protected, ctx)
            },
        );
        if best_effort && !invalid {
            Ok(())
        } else {
            result
        }
    }

    pub(super) fn validate_indirect(
        &mut self,
        text: &str,
        path: &Path,
        ctx: &tera::Context,
    ) -> Result<()> {
        if self.active() {
            self.protect_impl(text, path, true, true, Some(ctx))?;
        }
        Ok(())
    }

    pub(super) fn render(
        &self,
        text: &str,
        engine: &mut crate::Engine,
        ctx: &tera::Context,
    ) -> Result<(String, Option<Emissions>)> {
        if !self.active() || !text.contains(&self.prefix) {
            return Ok((engine.render(text, ctx)?, None));
        }
        // The extraction pass uses inert markers, but the file render emits
        // distinct markers through a function. Count each actual emission
        // (rather than each source tag: a loop can repeat a tag, or a branch
        // can skip it) and require it to land in a parsed string value.
        let name = &self.function_name;
        let prefix = format!("\u{e002}{}", &self.prefix[3..]);
        let counts = Arc::new(Mutex::new(vec![0; self.tags.len()]));
        let calls = Arc::clone(&counts);
        let emitted_prefix = prefix.clone();
        let mut tera = engine.tera_mut().clone();
        tera.register_function(
            name.clone(),
            move |kwargs: tera::Kwargs, _: &tera::State| {
                let index: usize = kwargs.must_get("index")?;
                let mut counts = calls.lock().unwrap();
                let count = counts
                    .get_mut(index)
                    .ok_or_else(|| tera::Error::message("unknown runtime variable marker"))?;
                *count += 1;
                Ok(format!("{emitted_prefix}{index}{MARKER_END}"))
            },
        );
        let mut source = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(i) = rest.find(&self.prefix) {
            let before = rest[..i]
                .strip_suffix('`')
                .ok_or_else(|| Error::Render("corrupted runtime variable marker".into()))?;
            source.push_str(before);
            let (index, tail) = rest[i + self.prefix.len()..]
                .split_once(MARKER_END)
                .ok_or_else(|| Error::Render("corrupted runtime variable marker".into()))?;
            source.push_str(&format!("{name}(index={index})"));
            rest = tail
                .strip_prefix('`')
                .ok_or_else(|| Error::Render("corrupted runtime variable marker".into()))?;
        }
        source.push_str(rest);
        let output = tera.render_str(&source, ctx, false)?;
        Ok((output, Some(Emissions { counts, prefix })))
    }

    pub(super) fn normalize(&self, table: &mut Table, emissions: Option<Emissions>) -> Result<()> {
        if let Some(emissions) = emissions {
            emissions.normalize(table, &self.prefix)?;
        }
        Ok(())
    }

    pub(super) fn check_keys(&self, table: &Table, path: &Path) -> Result<()> {
        if !self.active() {
            return Ok(());
        }
        for (key, value) in table {
            if key.contains(&self.prefix) {
                return Err(Error::Render(format!(
                    "{}: runtime variables are not supported in TOML keys or headers",
                    path.display()
                )));
            }
            self.check_value(value, path)?;
        }
        Ok(())
    }

    fn check_value(&self, value: &Value, path: &Path) -> Result<()> {
        match value {
            Value::Table(t) => self.check_keys(t, path),
            Value::Array(a) => {
                for v in a {
                    self.check_value(v, path)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    pub(super) fn restore(&self, table: &mut Table) -> Result<()> {
        if self.tags.is_empty() {
            return Ok(());
        }
        for (_, value) in table.iter_mut() {
            self.restore_value(value)?;
        }
        Ok(())
    }

    fn restore_value(&self, value: &mut Value) -> Result<()> {
        match value {
            Value::String(s) => {
                if !s.contains(&self.prefix) {
                    return Ok(());
                }
                let mut out = String::new();
                let mut rest = s.as_str();
                while let Some(i) = rest.find(&self.prefix) {
                    out.push_str(&rest[..i]);
                    rest = &rest[i + self.prefix.len()..];
                    let Some((index, tail)) = rest.split_once(MARKER_END) else {
                        return Err(Error::Render("corrupted deferred marker".into()));
                    };
                    let Some(tag) = index.parse::<usize>().ok().and_then(|i| self.tags.get(i))
                    else {
                        return Err(Error::Render("unknown deferred marker".into()));
                    };
                    out.push_str(tag);
                    rest = tail;
                }
                out.push_str(rest);
                *s = out;
            }
            Value::Table(t) => self.restore(t)?,
            Value::Array(a) => {
                for v in a {
                    self.restore_value(v)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

fn ident(s: &str) -> bool {
    s.as_bytes()
        .first()
        .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_')
}

fn tokens(mut s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    while !s.is_empty() {
        s = s.trim_start();
        let Some(c) = s.chars().next() else {
            break;
        };
        let mut n = c.len_utf8();
        if matches!(c, '\'' | '"' | '`') {
            let mut escaped = false;
            for ch in s[n..].chars() {
                n += ch.len_utf8();
                if ch == c && !escaped {
                    break;
                }
                escaped = ch == '\\' && !escaped;
            }
        } else if c.is_ascii_alphanumeric() || c == '_' {
            n = s
                .bytes()
                .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
                .count();
        } else if s.starts_with("==") || s.starts_with("!=") || s.starts_with("::") {
            n = 2;
        }
        out.push(&s[..n]);
        s = &s[n..];
    }
    out
}

fn references<'a>(words: &[&'a str]) -> Vec<&'a str> {
    words
        .iter()
        .enumerate()
        .filter_map(|(i, &w)| {
            let prev = i.checked_sub(1).and_then(|i| words.get(i)).copied();
            let next = words.get(i + 1).copied();
            let prevprev = i.checked_sub(2).and_then(|i| words.get(i)).copied();
            (ident(w)
                && !matches!(
                    w,
                    "true"
                        | "false"
                        | "none"
                        | "and"
                        | "or"
                        | "not"
                        | "is"
                        | "in"
                        | "if"
                        | "else"
                        | "for"
                )
                && !matches!(prev, Some("." | "|" | "is" | "::"))
                && !(prev == Some("not") && prevprev == Some("is"))
                && !matches!(next, Some("(" | "=" | "::")))
            .then_some(w)
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Container {
    Array,
    Header,
    InlineTable,
}

#[derive(Default)]
struct TomlQuote {
    delimiter: u8,
    multiline: bool,
    value: bool,
    last: u8,
    containers: Vec<Container>,
}

impl TomlQuote {
    fn scan(&mut self, text: &str) {
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            if self.delimiter == b'"' && b == b'\\' {
                i += 2;
                continue;
            }
            if self.delimiter == 0 && matches!(b, b'"' | b'\'') {
                self.value = self.last == b'='
                    || (self.containers.last() == Some(&Container::Array)
                        && matches!(self.last, b'[' | b','));
                self.delimiter = b;
                self.multiline = bytes[i..].starts_with(&[b; 3]);
                i += if self.multiline { 3 } else { 1 };
            } else if self.delimiter == b {
                if !self.multiline {
                    self.delimiter = 0;
                    self.last = b'"';
                    i += 1;
                } else {
                    let run = bytes[i..].iter().take_while(|&&c| c == b).count();
                    if run >= 3 {
                        self.delimiter = 0;
                        self.last = b'"';
                    }
                    i += run;
                }
            } else {
                if self.delimiter == 0 {
                    match b {
                        b'[' => {
                            let value = self.last == b'='
                                || (self.containers.last() == Some(&Container::Array)
                                    && matches!(self.last, b'[' | b','));
                            self.containers.push(if value {
                                Container::Array
                            } else {
                                Container::Header
                            });
                        }
                        b'{' => self.containers.push(Container::InlineTable),
                        b']' | b'}' => {
                            self.containers.pop();
                        }
                        b'\n' if self.containers.is_empty() => self.last = 0,
                        _ => {}
                    }
                    if !b.is_ascii_whitespace() {
                        self.last = b;
                    }
                } else if b == b'\n' && !self.multiline {
                    self.delimiter = 0;
                    self.last = 0;
                }
                i += 1;
            }
        }
    }
}

fn tainted(
    value: &tera::Value,
    ctx: &tera::Context,
    prefix: &str,
    options: &LoadOptions,
    visiting: &mut HashSet<usize>,
) -> bool {
    let address = value as *const _ as usize;
    if !visiting.insert(address) {
        return false;
    }
    let result = if let Some(s) = value.as_str() {
        if s.contains(prefix) {
            true
        } else {
            let mut rest = s;
            let mut found = false;
            while let Some(start) = rest.find("{{") {
                rest = &rest[start + 2..];
                let Some(end) = tera_tag_end(rest, b"}}") else {
                    break;
                };
                let words = tokens(trim_tera_tag(&rest[..end - 2]));
                for root in references(&words) {
                    if options.matches(root)
                        || ctx.get(root).is_some_and(|v| {
                            let (selected, _) = select_path(&words, root, v);
                            tainted(selected, ctx, prefix, options, visiting)
                        })
                    {
                        found = true;
                        break;
                    }
                }
                if found {
                    break;
                }
                rest = &rest[end..];
            }
            found
        }
    } else {
        value
            .as_array()
            .is_some_and(|a| a.iter().any(|v| tainted(v, ctx, prefix, options, visiting)))
            || value.as_map().is_some_and(|m| {
                m.values()
                    .any(|v| tainted(v, ctx, prefix, options, visiting))
            })
    };
    visiting.remove(&address);
    result
}

fn select_path<'a>(
    words: &[&str],
    root: &str,
    mut value: &'a tera::Value,
) -> (&'a tera::Value, usize) {
    let start = words
        .iter()
        .position(|word| std::ptr::eq(word.as_ptr(), root.as_ptr()))
        .unwrap();
    let mut end = start + 1;
    while end < words.len() {
        let (key, width) = match words[end..] {
            [".", key, ..] => (key, 2),
            ["[", key, "]", ..] if key.bytes().all(|b| b.is_ascii_digit()) => (key, 3),
            ["[", key, "]", ..]
                if key.starts_with(['\'', '"', '`']) && key.len() >= 2 && !key.contains('\\') =>
            {
                (&key[1..key.len() - 1], 3)
            }
            _ => break,
        };
        let next = if let Some(map) = value.as_map() {
            map.get(&tera::value::Key::String(Arc::from(key)))
        } else if let Some(array) = value.as_array() {
            key.parse::<usize>().ok().and_then(|i| array.get(i))
        } else {
            None
        };
        let Some(next) = next else { break };
        value = next;
        end += width;
    }
    (value, end)
}

// Only statically selected, unmodified string values can carry markers across
// load-time expressions. Dynamic selection from a tainted container is
// deliberately rejected rather than evaluating it with placeholder data.
fn indirect_use(
    words: &[&str],
    refs: &[&str],
    ctx: &tera::Context,
    prefix: &str,
    options: &LoadOptions,
    bound: &impl Fn(&str) -> bool,
    simple_copy: bool,
) -> bool {
    for root in references(words) {
        if !refs.contains(&root) || bound(root) {
            continue;
        }
        let Some(value) = ctx.get(root) else { continue };
        let start = words
            .iter()
            .position(|word| std::ptr::eq(word.as_ptr(), root.as_ptr()))
            .unwrap();
        let (value, end) = select_path(words, root, value);
        if tainted(value, ctx, prefix, options, &mut HashSet::new())
            && !(simple_copy && start == 0 && end == words.len() && value.is_string())
        {
            return true;
        }
    }
    false
}
