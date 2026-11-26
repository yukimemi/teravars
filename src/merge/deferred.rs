use std::collections::{HashMap, HashSet, hash_map::RandomState};
use std::fmt::Write as _;
use std::hash::{BuildHasher, Hasher};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use toml::de::ValueDeserializer;
use toml::{Table, Value};
use toml_parser::Source as TomlSource;
use toml_parser::lexer::TokenKind;

use super::SourceId;
use crate::comments::tera_tag_end;
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
    pub(super) prefix: String,
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
    pub(super) fn protect(&mut self, text: &str, source: &SourceId, allow: bool) -> Result<String> {
        self.protect_impl(text, source, allow, true, None, true)
            .map(|(text, _)| text)
    }

    pub(super) fn protect_classified(
        &mut self,
        text: &str,
        source: &SourceId,
    ) -> Result<(String, Option<Table>)> {
        let (text, offsets) = self.protect_impl(text, source, true, true, None, true)?;
        if offsets.is_empty() {
            return Ok((text, None));
        }
        // Carry scan provenance through TOML decoding, separately from the
        // real document. Only matching decoded values can use these offsets.
        let mut annotated = text.clone();
        for offset in offsets.into_iter().rev() {
            annotated.insert_str(offset, &self.prefix);
        }
        let classified = crate::vars::extract_vars(&annotated).ok();
        Ok((text, classified))
    }

    fn protect_impl(
        &mut self,
        text: &str,
        source: &SourceId,
        allow: bool,
        document: bool,
        context: Option<(&tera::Context, Option<&Table>)>,
        classify_runtime: bool,
    ) -> Result<(String, Vec<usize>)> {
        if !self.active() {
            return Ok((text.to_owned(), Vec::new()));
        }
        if document && context.is_none() && text.contains(&self.prefix) {
            return Err(Error::Render(format!(
                "{source}: deferred marker collision"
            )));
        }
        let classified = context.map_or_else(HashMap::new, |(ctx, vars)| {
            let mut addresses = HashMap::new();
            if let (Some(value), Some(vars)) = (ctx.get("vars"), vars) {
                collect_classified_table(value, vars, &self.prefix, &mut addresses);
            }
            addresses
        });
        let mut offsets = Vec::new();
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
                return Ok((out, offsets));
            };
            let end = start + 2 + end;
            let tag = &text[start..end];
            let body = expression_body(&tag[2..tag.len() - 2]);
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
                        "{source}: runtime variable `{name}` has an ambiguous load-time binding"
                    )));
                }
                if let Some(name) = refs
                    .iter()
                    .find(|n| classify_runtime && self.options.matches(n) && !bound(n))
                {
                    let line = text[..start].bytes().filter(|b| *b == b'\n').count() + 1;
                    let fail = |why| {
                        Error::Render(format!("{source}:{line}: runtime variable `{name}` {why}"))
                    };
                    if kind == '{'
                        && let Err(err) = &parsed
                        && matches!(err.kind(), tera::ErrorKind::SyntaxError(_))
                    {
                        return Err(Error::Render(format!("{source}:{line}: {err}")));
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
                if document
                    && context.is_none()
                    && kind == '{'
                    && quote.delimiter != 0
                    && quote.value
                    && refs.iter().any(|n| self.options.matches(n) && bound(n))
                {
                    offsets.push(out.len());
                }
                if let Some((ctx, _)) = context {
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
                        &|name| self.options.matches(name),
                        (&classified, &bound),
                        simple_copy,
                    ) {
                        let line = text[..start].bytes().filter(|b| *b == b'\n').count() + 1;
                        return Err(Error::Render(format!(
                            "{source}:{line}: runtime variable value may only be copied directly into a string; defer the original expression instead"
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
        Ok((out, offsets))
    }

    pub(super) fn resolve(
        &mut self,
        vars: &mut Table,
        engine: &mut crate::Engine,
        ctx: &tera::Context,
        source: &SourceId,
        best_effort: bool,
        classified: Option<&Table>,
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
            |text, ctx, iteration| {
                if !text.contains("{{") && !text.contains("{%") {
                    return engine.render(text, ctx);
                }
                // The document scan already classified free runtime references
                // in extracted values. On the first per-file iteration, let
                // surviving tags use the caller context just as ordinary vars
                // extraction does; their enclosing document locals are absent.
                // Subsequent iterations can expose new, unclassified tags.
                let (protected, _) = self
                    .protect_impl(
                        text,
                        source,
                        true,
                        false,
                        Some((ctx, classified)),
                        !best_effort || iteration != 0,
                    )
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
        source: &SourceId,
        ctx: &tera::Context,
        classified: Option<&Table>,
    ) -> Result<()> {
        if self.active() {
            self.protect_impl(text, source, true, true, Some((ctx, classified)), false)?;
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

    pub(super) fn normalize(&self, table: &mut Table, emissions: Option<&Emissions>) -> Result<()> {
        if let Some(emissions) = emissions {
            emissions.normalize(table, &self.prefix)?;
        }
        Ok(())
    }

    pub(super) fn check_keys(&self, table: &Table, source: &SourceId) -> Result<()> {
        if !self.active() {
            return Ok(());
        }
        for (key, value) in table {
            if key.contains(&self.prefix) {
                return Err(Error::Render(format!(
                    "{source}: runtime variables are not supported in TOML keys or headers"
                )));
            }
            self.check_value(value, source)?;
        }
        Ok(())
    }

    fn check_value(&self, value: &Value, source: &SourceId) -> Result<()> {
        match value {
            Value::Table(t) => self.check_keys(t, source),
            Value::Array(a) => {
                for v in a {
                    self.check_value(v, source)?;
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
                *s = self.restore_markers(s, None)?;
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

    /// Restore both persistent and emitted deferred markers inside string value
    /// tokens of a validated rendered TOML document, preserving valid TOML and
    /// line structure.
    pub(super) fn restore_rendered(
        &self,
        rendered: String,
        emitted_prefix: Option<&str>,
    ) -> Result<String> {
        if !self.active() {
            return Ok(rendered);
        }
        let has_persistent = !self.prefix.is_empty() && rendered.contains(&self.prefix);
        let has_emitted = emitted_prefix.is_some_and(|p| rendered.contains(p));
        if !has_persistent && !has_emitted {
            return Ok(rendered);
        }

        let mut out = String::with_capacity(rendered.len());
        let mut last_pos = 0;

        for token in TomlSource::new(&rendered).lex() {
            let kind = token.kind();
            if !matches!(
                kind,
                TokenKind::BasicString
                    | TokenKind::LiteralString
                    | TokenKind::MlBasicString
                    | TokenKind::MlLiteralString
            ) {
                continue;
            }
            let start = token.span().start();
            let end = token.span().end();
            let raw_token = &rendered[start..end];
            let token_has_persistent = has_persistent && raw_token.contains(&self.prefix);
            let token_has_emitted = emitted_prefix.is_some_and(|p| raw_token.contains(p));
            if !token_has_persistent && !token_has_emitted {
                continue;
            }

            let decoded = ValueDeserializer::parse(raw_token)
                .and_then(String::deserialize)
                .map_err(|e| Error::Render(format!("failed to decode rendered string: {e}")))?;
            let restored = self.restore_markers(&decoded, emitted_prefix)?;
            let orig_newlines = raw_token.bytes().filter(|&b| b == b'\n').count();
            let reencoded = encode_toml_string(kind, raw_token, &restored, orig_newlines);

            out.push_str(&rendered[last_pos..start]);
            out.push_str(&reencoded);
            last_pos = end;
        }

        out.push_str(&rendered[last_pos..]);
        Ok(out)
    }

    fn restore_markers(&self, input: &str, emitted_prefix: Option<&str>) -> Result<String> {
        let mut out = String::with_capacity(input.len());
        let mut rest = input;
        while !rest.is_empty() {
            let persistent_pos = (!self.prefix.is_empty())
                .then(|| rest.find(&self.prefix).map(|i| (i, self.prefix.len())))
                .flatten();
            let emitted_pos = emitted_prefix.and_then(|p| rest.find(p).map(|i| (i, p.len())));
            let next = match (persistent_pos, emitted_pos) {
                (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            };
            let Some((pos, prefix_len)) = next else {
                out.push_str(rest);
                break;
            };
            out.push_str(&rest[..pos]);
            rest = &rest[pos + prefix_len..];
            let Some((index, tail)) = rest.split_once(MARKER_END) else {
                return Err(Error::Render("corrupted deferred marker".into()));
            };
            let Some(tag) = index.parse::<usize>().ok().and_then(|i| self.tags.get(i)) else {
                return Err(Error::Render("unknown deferred marker".into()));
            };
            out.push_str(tag);
            rest = tail;
        }
        Ok(out)
    }
}

fn encode_toml_string(
    kind: TokenKind,
    raw_token: &str,
    value: &str,
    orig_newlines: usize,
) -> String {
    match kind {
        TokenKind::LiteralString => {
            if can_be_literal_string(value) {
                format!("'{value}'")
            } else {
                encode_basic_string(value)
            }
        }
        TokenKind::BasicString => encode_basic_string(value),
        TokenKind::MlLiteralString => {
            let had_leading_newline =
                raw_token.starts_with("'''\n") || raw_token.starts_with("'''\r\n");
            if let Some(encoded) =
                try_encode_ml_literal_string(value, orig_newlines, had_leading_newline)
            {
                encoded
            } else {
                encode_ml_basic_string(value, orig_newlines, had_leading_newline)
            }
        }
        TokenKind::MlBasicString => {
            let had_leading_newline =
                raw_token.starts_with("\"\"\"\n") || raw_token.starts_with("\"\"\"\r\n");
            encode_ml_basic_string(value, orig_newlines, had_leading_newline)
        }
        _ => encode_basic_string(value),
    }
}

fn can_be_literal_string(s: &str) -> bool {
    s.chars()
        .all(|c| c == '\t' || (c >= ' ' && c != '\'' && c != '\u{7F}'))
}

fn encode_basic_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\u{0008}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{000C}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 || c == '\u{007F}' => {
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn try_encode_ml_literal_string(
    value: &str,
    target_newlines: usize,
    had_leading_newline: bool,
) -> Option<String> {
    if value.contains("'''")
        || value.ends_with('\'')
        || !value
            .chars()
            .all(|c| c == '\t' || c == '\n' || (c >= ' ' && c != '\u{7F}'))
    {
        return None;
    }
    let value_newlines = value.bytes().filter(|&b| b == b'\n').count();
    if had_leading_newline && 1 + value_newlines == target_newlines {
        Some(format!("'''\n{value}'''"))
    } else if !had_leading_newline && !value.starts_with('\n') && value_newlines == target_newlines
    {
        Some(format!("'''{value}'''"))
    } else {
        None
    }
}

fn encode_ml_basic_string(
    value: &str,
    target_newlines: usize,
    prefer_leading_newline: bool,
) -> String {
    let mut out = String::with_capacity(value.len() + 6);
    out.push_str("\"\"\"");
    let mut emitted_newlines = 0usize;
    if prefer_leading_newline && target_newlines > 0 {
        out.push('\n');
        emitted_newlines += 1;
    }

    let mut consecutive_quotes = 0usize;
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        let is_last = chars.peek().is_none();
        if c != '"' {
            consecutive_quotes = 0;
        }
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => {
                if consecutive_quotes >= 2 || is_last {
                    out.push_str("\\\"");
                    consecutive_quotes = 0;
                } else {
                    out.push('"');
                    consecutive_quotes += 1;
                }
            }
            '\u{0008}' => out.push_str("\\b"),
            '\t' => out.push('\t'),
            '\n' => {
                if emitted_newlines < target_newlines && out.len() > 3 {
                    out.push('\n');
                    emitted_newlines += 1;
                } else {
                    out.push_str("\\n");
                }
            }
            '\u{000C}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 || c == '\u{007F}' => {
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }

    if emitted_newlines < target_newlines {
        out.push('\\');
        for _ in emitted_newlines..target_newlines {
            out.push('\n');
        }
    }
    out.push_str("\"\"\"");
    out
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

// Preserve array offsets and table merge rules, without trusting inherited strings.
pub(super) fn classified_vars(inherited: &Table, extracted: Table) -> Table {
    fn forget(value: &mut Value) {
        match value {
            Value::String(_) => *value = Value::Boolean(false),
            Value::Array(items) => items.iter_mut().for_each(forget),
            Value::Table(table) => table.iter_mut().for_each(|(_, value)| forget(value)),
            _ => {}
        }
    }
    let mut classified = inherited.clone();
    classified.iter_mut().for_each(|(_, value)| forget(value));
    super::deep_merge(&mut classified, extracted);
    classified
}

type ClassifiedOutputs = HashMap<usize, HashSet<usize>>;

fn collect_classified_table(
    value: &tera::Value,
    table: &Table,
    prefix: &str,
    addresses: &mut ClassifiedOutputs,
) {
    if let Some(map) = value.as_map() {
        for (key, original) in table {
            if let Some(value) = map.get(&tera::value::Key::String(Arc::from(key.as_str()))) {
                collect_classified(value, original, prefix, addresses);
            }
        }
    }
}

fn collect_classified(
    value: &tera::Value,
    original: &Value,
    prefix: &str,
    addresses: &mut ClassifiedOutputs,
) {
    match original {
        Value::String(s) if s.contains(prefix) => {
            let mut restored = String::new();
            let mut offsets = HashSet::new();
            let mut parts = s.split(prefix);
            restored.push_str(parts.next().unwrap());
            for part in parts {
                offsets.insert(restored.len());
                restored.push_str(part);
            }
            if value.as_str() == Some(restored.as_str()) {
                addresses.insert(value as *const _ as usize, offsets);
            }
        }
        Value::Table(table) => collect_classified_table(value, table, prefix, addresses),
        Value::Array(items) => {
            if let Some(array) = value.as_array() {
                for (value, original) in array.iter().zip(items) {
                    collect_classified(value, original, prefix, addresses);
                }
            }
        }
        _ => {}
    }
}

fn tainted(
    value: &tera::Value,
    ctx: &tera::Context,
    prefix: &str,
    runtime_name: &impl Fn(&str) -> bool,
    classified: &ClassifiedOutputs,
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
            let outputs = classified.get(&address);
            let mut rest = s;
            let mut found = false;
            while let Some(start) = rest.find("{{") {
                let classified_output =
                    outputs.is_some_and(|o| o.contains(&(s.len() - rest.len() + start)));
                rest = &rest[start + 2..];
                let Some(end) = tera_tag_end(rest, b"}}") else {
                    break;
                };
                let words = tokens(expression_body(&rest[..end - 2]));
                for root in references(&words) {
                    if (runtime_name(root) && !classified_output)
                        || ctx.get(root).is_some_and(|v| {
                            let (selected, _) = select_path(&words, root, v);
                            tainted(selected, ctx, prefix, runtime_name, classified, visiting)
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
        value.as_array().is_some_and(|a| {
            a.iter()
                .any(|v| tainted(v, ctx, prefix, runtime_name, classified, visiting))
        }) || value.as_map().is_some_and(|m| {
            m.values()
                .any(|v| tainted(v, ctx, prefix, runtime_name, classified, visiting))
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
    runtime_name: &impl Fn(&str) -> bool,
    (classified, bound): (&ClassifiedOutputs, &impl Fn(&str) -> bool),
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
        if tainted(value, ctx, prefix, runtime_name, classified, &mut HashSet::new())
            && !(simple_copy && start == 0 && end == words.len() && value.is_string())
        {
            return true;
        }
    }
    false
}

fn expression_body(body: &str) -> &str {
    let body = body.strip_prefix('-').unwrap_or(body);
    body.strip_suffix('-').unwrap_or(body).trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_toml_string_round_trips_and_preserves_newlines() {
        let values = [
            "",
            "plain",
            r##"{{ cap[0] | replace(from="a", to="#") }}"##,
            r#"{{ file_path | replace(from='\\', to='/') }}"#,
            "has 'single' and \"double\" quotes",
            "has \"\"\"triple\"\"\" and '''triple'''",
            "\"\"\"starts with triple",
            "\"\"starts with two",
            "\"starts with one",
            "\"\"\"",
            "\"\"",
            "\"",
            "ends with quote\"",
            "ends with two\"\"",
            "ends with backslash\\",
            "\nleading newline",
            "trailing newline\n",
            "line1\nline2\nline3",
            "cr\r\nlf\rand\ttab\x07bell",
            "   spaces   ",
        ];

        for value in values {
            for (kind, raw_token, newlines) in [
                (TokenKind::BasicString, r#""x""#, 0),
                (TokenKind::LiteralString, "'x'", 0),
                (TokenKind::MlBasicString, "\"\"\"x\"\"\"", 0),
                (TokenKind::MlBasicString, "\"\"\"\nx\n\"\"\"", 2),
                (TokenKind::MlBasicString, "\"\"\"\na\nb\nc\n\"\"\"", 4),
                (TokenKind::MlLiteralString, "'''x'''", 0),
                (TokenKind::MlLiteralString, "'''\nx\n'''", 2),
                (TokenKind::MlLiteralString, "'''\na\nb\nc\n'''", 4),
            ] {
                let encoded = encode_toml_string(kind, raw_token, value, newlines);
                assert_eq!(
                    encoded.bytes().filter(|&b| b == b'\n').count(),
                    newlines,
                    "newline count mismatch for kind={kind:?} value={value:?} encoded={encoded:?}"
                );
                let doc = format!("k = {encoded}\n");
                let parsed: Table = toml::from_str(&doc).unwrap_or_else(|e| {
                    panic!(
                        "invalid TOML for kind={kind:?} value={value:?} encoded={encoded:?}: {e}"
                    )
                });
                assert_eq!(
                    parsed["k"].as_str(),
                    Some(value),
                    "decoded mismatch for kind={kind:?} value={value:?} encoded={encoded:?}"
                );
            }
        }
    }
}
