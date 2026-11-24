use std::borrow::Cow;
use std::iter::Peekable;
use std::path::Path;

use serde::Deserialize;
use toml::de::ValueDeserializer;
use toml_parser::lexer::{Lexer, TokenKind};
use toml_parser::{ParseError, Source, Span};

use crate::Result;
use crate::comments::{tera_tag_end, trim_tera_tag};
use crate::error::Error;
use crate::vars::tera_block_delta;

const ROOT: &str = "include";
const NAMESPACED: &str = "teravars.include";

#[derive(Debug)]
pub(super) struct Includes {
    pub(super) paths: Vec<String>,
    name: Option<&'static str>,
    ambiguous: bool,
}

impl Includes {
    pub(super) fn check_rendered(&self, parsed: &toml::Table, path: &Path) -> Result<()> {
        if !self.ambiguous {
            return Ok(());
        }
        let namespaced = parsed
            .get("teravars")
            .and_then(toml::Value::as_table)
            .is_some_and(|table| table.contains_key(ROOT));
        for (name, exists) in [(ROOT, parsed.contains_key(ROOT)), (NAMESPACED, namespaced)] {
            if exists && self.name != Some(name) {
                return Err(invalid(
                    path,
                    name,
                    "owning table depends on Tera control flow",
                ));
            }
        }
        Ok(())
    }
}

// Locate static assignments in the source without requiring unrelated
// template values to be valid TOML before rendering.
pub(super) fn extract_include_paths(text: &str, path: &Path) -> Result<Includes> {
    use TokenKind::*;

    let mut tokens = Tokens::new(text).peekable();
    let mut section = Section::Root;
    let mut root = None;
    let mut namespaced = None;
    let mut ambiguous = false;

    while let Some(first) = tokens.next() {
        let owner = match first.kind {
            Newline => continue,
            LeftSquareBracket => {
                // A conditional header may change the active table. Only
                // another static header can make the scope definite again.
                section = if first.block_depth != 0 {
                    Section::Ambiguous
                } else if take_key(&mut tokens, "teravars")
                    && take(&mut tokens, RightSquareBracket).is_some()
                    && tokens.peek().is_none_or(|t| t.kind == Newline)
                {
                    Section::Teravars
                } else {
                    Section::Other
                };
                while tokens
                    .next_if(|t| t.kind != Newline && t.block_depth >= first.block_depth)
                    .is_some()
                {}
                continue;
            }
            _ if first.block_depth != 0 => None,
            _ if first.is_key(ROOT) => match section {
                Section::Root => Some((&mut root, ROOT)),
                Section::Teravars => Some((&mut namespaced, NAMESPACED)),
                Section::Ambiguous => {
                    ambiguous |= take(&mut tokens, Equals).is_some();
                    None
                }
                Section::Other => None,
            },
            _ if matches!(section, Section::Root | Section::Ambiguous)
                && first.is_key("teravars") =>
            {
                if take(&mut tokens, Dot).is_some() {
                    take_key(&mut tokens, ROOT).then_some((&mut namespaced, NAMESPACED))
                } else {
                    if take(&mut tokens, Equals).is_some()
                        && take(&mut tokens, LeftCurlyBracket).is_some()
                    {
                        if let Some(value) = inline_include(text, &mut tokens, path)? {
                            if matches!(section, Section::Ambiguous) {
                                ambiguous = true;
                            } else {
                                record(&mut namespaced, value, path, NAMESPACED)?;
                            }
                        }
                    }
                    None
                }
            }
            _ => None,
        };

        if let Some((slot, name, eq)) =
            owner.and_then(|(slot, name)| take(&mut tokens, Equals).map(|eq| (slot, name, eq)))
        {
            let start = eq.end;
            let end = value_end(&mut tokens, &eq, false);
            if matches!(section, Section::Ambiguous) {
                ambiguous = true;
            } else {
                record(slot, &text[start..end], path, name)?;
            }
        } else {
            value_end(&mut tokens, &first, false);
        }
    }

    let (value, name) = match (root, namespaced) {
        (Some(_), Some(_)) => {
            return Err(Error::IncludeConflict {
                path: path.to_path_buf(),
            });
        }
        (Some(value), None) => (Some(value), Some(ROOT)),
        (None, Some(value)) => (Some(value), Some(NAMESPACED)),
        (None, None) => (None, None),
    };
    let paths = if let (Some(value), Some(name)) = (value, name) {
        ValueDeserializer::parse(value.trim_matches([' ', '\t', '\r', '\n']))
            .and_then(Vec::<String>::deserialize)
            .map_err(|err| invalid(path, name, err))?
    } else {
        Vec::new()
    };
    Ok(Includes {
        paths,
        name,
        ambiguous,
    })
}

enum Section {
    Root,
    Teravars,
    Other,
    Ambiguous,
}

fn inline_include<'a>(
    text: &'a str,
    tokens: &mut Peekable<Tokens<'a>>,
    path: &Path,
) -> Result<Option<&'a str>> {
    use TokenKind::*;

    let mut value = None;
    while let Some(first) = tokens.next() {
        if first.block_depth != 0 {
            continue;
        }
        match first.kind {
            RightCurlyBracket => return Ok(value),
            Newline => continue,
            _ => {}
        }
        let eq = if first.is_key(ROOT) {
            take(tokens, Equals)
        } else {
            None
        };
        if let Some(eq) = eq {
            let end = value_end(tokens, &eq, true);
            record(&mut value, &text[eq.end..end], path, NAMESPACED)?;
        } else {
            value_end(tokens, &first, true);
        }
        if take(tokens, Comma).is_none() {
            break;
        }
    }
    if take(tokens, RightCurlyBracket).is_some() || value.is_none() {
        Ok(value)
    } else {
        Err(invalid(path, NAMESPACED, "unclosed inline table"))
    }
}

fn record<'a>(slot: &mut Option<&'a str>, value: &'a str, path: &Path, name: &str) -> Result<()> {
    if slot.replace(value).is_some() {
        Err(invalid(path, name, "duplicate declaration"))
    } else {
        Ok(())
    }
}

fn invalid(path: &Path, name: &str, err: impl std::fmt::Display) -> Error {
    Error::Extract(format!("{} `{name}`: {err}", path.display()))
}

fn take<'a>(tokens: &mut Peekable<Tokens<'a>>, kind: TokenKind) -> Option<Lexeme<'a>> {
    // A directive's key and separators must be at the same static depth.
    tokens.next_if(|t| t.block_depth == 0 && t.kind == kind)
}

fn take_key(tokens: &mut Peekable<Tokens<'_>>, key: &str) -> bool {
    tokens
        .next_if(|t| t.block_depth == 0 && t.is_key(key))
        .is_some()
}

// Only a newline outside a value ends a document assignment. An inline
// table member ends at its own comma or closing brace instead.
fn value_end(tokens: &mut Peekable<Tokens<'_>>, start: &Lexeme<'_>, inline: bool) -> usize {
    use TokenKind::*;

    let mut end = start.end;
    let mut depth = 0usize;
    let mut nesting_block = start.block_depth;
    while let Some(token) = tokens.peek() {
        if token.block_depth < start.block_depth {
            break;
        }
        if token.block_depth < nesting_block {
            depth = 0;
        }
        if depth == 0
            && if inline {
                token.block_depth == start.block_depth
                    && matches!(token.kind, Comma | RightCurlyBracket)
            } else {
                token.kind == Newline
            }
        {
            break;
        }
        let token = tokens.next().unwrap();
        if depth == 0 {
            nesting_block = token.block_depth;
        }
        if token.block_depth != nesting_block || token.kind == Newline {
            continue;
        }
        match token.kind {
            LeftSquareBracket | LeftCurlyBracket => depth += 1,
            RightSquareBracket | RightCurlyBracket => depth = depth.saturating_sub(1),
            _ => {}
        }
        end = token.end;
    }
    end
}

struct Lexeme<'a> {
    kind: TokenKind,
    raw: &'a str,
    end: usize,
    block_depth: usize,
}

impl Lexeme<'_> {
    fn is_key(&self, key: &str) -> bool {
        match self.kind {
            TokenKind::Atom => self.raw == key,
            TokenKind::BasicString | TokenKind::LiteralString => {
                let raw = toml_parser::Raw::new_unchecked(
                    self.raw,
                    self.kind.encoding(),
                    Span::new_unchecked(0, self.raw.len()),
                );
                let mut decoded = Cow::Borrowed("");
                let mut bad = false;
                raw.decode_key(&mut decoded, &mut |_: ParseError| bad = true);
                !bad && decoded == key
            }
            _ => false,
        }
    }
}

struct Tokens<'a> {
    text: &'a str,
    lexer: Lexer<'a>,
    offset: usize,
    block_depth: usize,
    capture_depth: Option<usize>,
    branches: Vec<Branch>,
}

impl<'a> Tokens<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            lexer: Source::new(text).lex(),
            offset: 0,
            block_depth: 0,
            capture_depth: None,
            branches: Vec::new(),
        }
    }

    fn live(&self) -> bool {
        self.branches.last().is_none_or(|branch| branch.live)
    }

    fn branch_tag(&mut self, keyword: &str, args: &str) {
        match keyword {
            "if" => {
                let mut branch = Branch {
                    live: false,
                    fallthrough: self.live(),
                };
                branch.arm(args);
                self.branches.push(branch);
            }
            "elif" | "else" => {
                if let Some(branch) = self.branches.last_mut() {
                    branch.arm(if keyword == "else" { "true" } else { args });
                }
            }
            "endif" => {
                self.branches.pop();
            }
            _ => {}
        }
    }
}

// Unknown expressions may take either arm. Never evaluate them without the
// variables that the include itself might provide.
struct Branch {
    live: bool,
    fallthrough: bool,
}

impl Branch {
    fn arm(&mut self, condition: &str) {
        let condition = condition.trim();
        self.live = self.fallthrough && condition != "false";
        self.fallthrough &= condition != "true";
    }
}

impl<'a> Iterator for Tokens<'a> {
    type Item = Lexeme<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        use TokenKind::*;

        while let Some(token) = self.lexer.next() {
            let start = self.offset + token.span().start();
            let end = self.offset + token.span().end();
            let kind = token.kind();
            if kind == LeftCurlyBracket {
                let rest = &self.text[start..];
                let delim: &[u8] = match rest.as_bytes().get(1) {
                    Some(b'{') => b"}}",
                    Some(b'%') => b"%}",
                    Some(b'#') => b"#}",
                    _ => b"",
                };
                if !delim.is_empty() {
                    // Restart after the entire tag: TOML tokens inside a
                    // Tera expression have no bearing on TOML nesting.
                    let body = &rest[2..];
                    let tag_end = tera_tag_end(body, delim);
                    self.offset = start + 2 + tag_end.unwrap_or(body.len());
                    if let Some(end) = tag_end.filter(|_| delim == b"%}") {
                        let tag = trim_tera_tag(&body[..end - 2]);
                        let (keyword, args) =
                            tag.split_once(char::is_whitespace).unwrap_or((tag, ""));
                        self.branch_tag(keyword, args);
                        if keyword == "raw" {
                            self.offset = raw_block_end(self.text, self.offset);
                        } else {
                            match block_delta(keyword, args) {
                                1 => {
                                    self.block_depth += 1;
                                    if matches!(
                                        keyword,
                                        "set" | "set_global" | "macro" | "component"
                                    ) && self.capture_depth.is_none()
                                    {
                                        self.capture_depth = Some(self.block_depth);
                                    }
                                }
                                -1 => {
                                    self.block_depth = self.block_depth.saturating_sub(1);
                                    if self
                                        .capture_depth
                                        .is_some_and(|depth| self.block_depth < depth)
                                    {
                                        self.capture_depth = None;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    self.lexer = Source::new(&self.text[self.offset..]).lex();
                    if delim == b"}}" && self.capture_depth.is_none() && self.live() {
                        return Some(Lexeme {
                            kind: Atom,
                            raw: &self.text[start..self.offset],
                            end: self.offset,
                            block_depth: self.block_depth,
                        });
                    }
                    continue;
                }
            }
            if kind == Eof {
                return None;
            }
            if self.capture_depth.is_none() && self.live() && !matches!(kind, Whitespace | Comment)
            {
                return Some(Lexeme {
                    kind,
                    raw: &self.text[start..end],
                    end,
                    block_depth: self.block_depth,
                });
            }
        }
        None
    }
}

fn block_delta(keyword: &str, args: &str) -> i8 {
    match keyword {
        "component" => 1,
        "endcomponent" | "endset" => -1,
        "set" | "set_global" => {
            let args = args.trim_start();
            let name_end = args
                .find(|c: char| c.is_whitespace() || matches!(c, '=' | '|'))
                .unwrap_or(args.len());
            i8::from(!args[name_end..].trim_start().starts_with('='))
        }
        _ => tera_block_delta(keyword),
    }
}

fn raw_block_end(text: &str, mut offset: usize) -> usize {
    while let Some(open) = text[offset..].find("{%") {
        offset += open + 2;
        let tag = text[offset..].trim_start_matches(|c: char| c.is_whitespace() || c == '-');
        if let Some(close) = tag.strip_prefix("endraw") {
            let close = close.trim_start_matches(|c: char| c.is_whitespace() || c == '-');
            if close.starts_with("%}") {
                return text.len() - close.len() + 2;
            }
        }
    }
    text.len()
}
