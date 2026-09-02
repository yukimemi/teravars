//! TOML-aware comment stripping.
//!
//! teravars renders a config file as a Tera template *before* handing the
//! result to a TOML parser, so Tera also sees whatever lives inside the
//! file's `#` comments. That is almost never what the author meant, and it
//! turns two extremely common comment styles into hard load failures:
//!
//! ```text
//! # example: port = "{{ vars.not_yet_defined }}"   -> Error::Render (field not defined)
//! # to disable this block: {% if false %}          -> Error::Render (unexpected end of input)
//! ```
//!
//! A commented-out sample line and a note that quotes teravars syntax are
//! exactly what a documented config file is made of, so every consumer hit
//! this. [`strip_toml_comments`] removes comment text before rendering, which
//! makes comments inert: whatever they say, they neither render nor need to
//! balance.
//!
//! The scanner is TOML-aware, so a `#` that is *not* a comment survives:
//! inside basic / literal strings (`"…#…"`, `'…#…'`), inside multi-line
//! strings (`"""…"""`, `'''…'''` — including the 3–5-quote terminating runs
//! TOML permits, so `"""ends with a quote""""` closes exactly once), and
//! inside Tera delimiters
//! (`{{ … }}`, `{% … %}`, `{# … #}`) — a URL fragment or a `replace(from="#")`
//! filter argument keeps working. Inside an expression or statement it also
//! follows Tera's own string literals (`"…"`, `'…'`, `` `…` ``, no escape
//! sequences), so a closing token that appears *within* one —
//! `{{ x | replace(from="}}") }}` — does not end the tag early.
//!
//! Line structure is preserved: only the bytes from the `#` to the end of the
//! line are dropped, so Tera / TOML error locations still point at the right
//! line. In `load_merged` the removal is unobservable —
//! the rendered text is parsed into a `toml::Table` and the source discarded —
//! whereas [`Engine::render_toml`](crate::Engine::render_toml) hands the
//! rendered `String` back, so a caller that inspects it sees comment-free
//! output.

use std::borrow::Cow;

/// Remove every TOML comment from `text`, keeping its line structure.
///
/// Returns the input untouched (as [`Cow::Borrowed`]) when it contains no `#`
/// at all.
///
/// ```
/// use teravars::strip_toml_comments;
///
/// // `concat!` rather than a raw string: rustdoc treats a doctest line that
/// // starts with `# ` as a hidden line and eats the marker, even inside a
/// // string literal.
/// let src = concat!(
///     "# sample: url = \"{{ vars.missing }}\"\n",
///     "url = \"https://example.com/#anchor\"  # trailing note\n",
/// );
///
/// let stripped = strip_toml_comments(src);
/// assert!(!stripped.contains("vars.missing"));
/// assert!(stripped.contains("https://example.com/#anchor"));
/// assert_eq!(stripped.lines().count(), src.lines().count());
/// ```
pub fn strip_toml_comments(text: &str) -> Cow<'_, str> {
    if !text.contains('#') {
        return Cow::Borrowed(text);
    }

    let mut out = String::with_capacity(text.len());
    let mut scanner = Scanner::default();
    let mut rest = text;

    loop {
        let (line, terminator, next) = match rest.find('\n') {
            Some(idx) => (&rest[..idx], "\n", &rest[idx + 1..]),
            None => (rest, "", ""),
        };

        match scanner.scan_line(line) {
            // Drop the comment and any `\r` that trailed it; the line
            // terminator is re-emitted below, so the line count is stable.
            Some(idx) => out.push_str(&line[..idx]),
            None => out.push_str(line),
        }
        out.push_str(terminator);

        if terminator.is_empty() {
            break;
        }
        rest = next;
    }

    Cow::Owned(out)
}

#[derive(Default, Clone, Copy)]
enum State {
    #[default]
    Normal,
    /// `"…"` — honours backslash escapes.
    BasicString,
    /// `'…'` — no escapes.
    LiteralString,
    /// `"""…"""`, spans lines.
    MlBasic,
    /// `'''…'''`, spans lines.
    MlLiteral,
    /// `{{ … }}`, may span lines.
    TeraExpr,
    /// `{% … %}`, may span lines.
    TeraStmt,
    /// `{# … #}`, may span lines.
    TeraComment,
}

#[derive(Default)]
struct Scanner {
    state: State,
    /// Quote byte of the string literal currently open *inside* a Tera
    /// expression or statement, if any. Tera's own lexer accepts `"…"`,
    /// `'…'` and `` `…` `` and has no escape sequences (that is why it has
    /// three quote styles), so the matching quote byte always closes it.
    tera_quote: Option<u8>,
}

impl Scanner {
    /// Advance over one line (without its terminator) and return the byte
    /// offset at which a TOML comment starts, if any.
    ///
    /// Byte-wise scanning is UTF-8 safe here: every pattern is ASCII, and a
    /// UTF-8 continuation byte is always >= 0x80, so a multi-byte character
    /// can never be mistaken for a delimiter — and the only offset returned
    /// points at an ASCII `#`.
    fn scan_line(&mut self, line: &str) -> Option<usize> {
        let b = line.as_bytes();
        let mut i = 0;

        while i < b.len() {
            match self.state {
                State::Normal => {
                    if b[i] == b'#' {
                        return Some(i);
                    }
                    if let Some((state, width)) = open_at(b, i) {
                        self.state = state;
                        i += width;
                        continue;
                    }
                    i += 1;
                }
                State::BasicString => {
                    if b[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if b[i] == b'"' {
                        self.state = State::Normal;
                    }
                    i += 1;
                }
                State::LiteralString => {
                    if b[i] == b'\'' {
                        self.state = State::Normal;
                    }
                    i += 1;
                }
                State::MlBasic => {
                    if b[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    i += self.close_multiline(b, i, b'"');
                }
                State::MlLiteral => {
                    i += self.close_multiline(b, i, b'\'');
                }
                // Tera string literals must be tracked, or a closing token
                // inside one (`{{ x | replace(from="}}") }}`) reads as the
                // end of the tag — and the `#` after that false close then
                // looks like a TOML comment, silently truncating the line.
                State::TeraExpr | State::TeraStmt => {
                    let delim: &[u8] = if matches!(self.state, State::TeraExpr) {
                        b"}}"
                    } else {
                        b"%}"
                    };
                    match self.tera_quote {
                        Some(quote) => {
                            if b[i] == quote {
                                self.tera_quote = None;
                            }
                            i += 1;
                        }
                        None if matches!(b[i], b'"' | b'\'' | b'`') => {
                            self.tera_quote = Some(b[i]);
                            i += 1;
                        }
                        None => i += self.close(b, i, delim),
                    }
                }
                // A Tera comment's body is arbitrary text — no string
                // literals to honour, it just runs to the first `#}`.
                State::TeraComment => {
                    i += self.close(b, i, b"#}");
                }
            }
        }

        // A single-line TOML string cannot span lines, so one still open at
        // end of line means malformed TOML. Reset instead of letting a stray
        // quote swallow every comment in the rest of the file.
        if matches!(self.state, State::BasicString | State::LiteralString) {
            self.state = State::Normal;
        }

        None
    }

    /// Consume the closing delimiter of the current multi-line-capable state
    /// if it starts at `i`; returns how far to advance.
    fn close(&mut self, b: &[u8], i: usize, delim: &[u8]) -> usize {
        if at(b, i, delim) {
            self.state = State::Normal;
            delim.len()
        } else {
            1
        }
    }

    /// Consume a run of `quote` bytes inside a multi-line string and return
    /// how far to advance, closing the string when the run terminates it.
    ///
    /// TOML allows one or two quote characters immediately inside the
    /// delimiters (`x = """ends with a quote""""`, `x = """""two inside"""""`),
    /// so a terminating run is 3–5 quotes long and only its last three are the
    /// delimiter. Matching the *first* three of a four-quote run would leave a
    /// stray quote in `Normal` state, open a bogus single-line string, and
    /// hide the rest of the line's real comment from the stripper.
    fn close_multiline(&mut self, b: &[u8], i: usize, quote: u8) -> usize {
        if b[i] != quote {
            return 1;
        }
        let run = b[i..].iter().take_while(|&&c| c == quote).count();
        if run >= 3 {
            self.state = State::Normal;
        }
        run
    }
}

/// Recognize a delimiter that opens a non-`Normal` state at `i`.
///
/// Order matters: the Tera delimiters are checked before the TOML quotes so a
/// `{#` reads as a Tera comment rather than a `{` followed by a `#` comment,
/// and the triple quotes before the single ones.
fn open_at(b: &[u8], i: usize) -> Option<(State, usize)> {
    const OPENERS: &[(&[u8], State)] = &[
        (b"{{", State::TeraExpr),
        (b"{%", State::TeraStmt),
        (b"{#", State::TeraComment),
        (b"\"\"\"", State::MlBasic),
        (b"'''", State::MlLiteral),
        (b"\"", State::BasicString),
        (b"'", State::LiteralString),
    ];

    OPENERS
        .iter()
        .find(|(delim, _)| at(b, i, delim))
        .map(|(delim, state)| (*state, delim.len()))
}

fn at(b: &[u8], i: usize, needle: &[u8]) -> bool {
    b.len() >= i + needle.len() && &b[i..i + needle.len()] == needle
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_hash_is_borrowed_untouched() {
        let src = "[vars]\na = \"1\"\n";
        let got = strip_toml_comments(src);
        assert!(matches!(got, Cow::Borrowed(_)));
        assert_eq!(got, src);
    }

    #[test]
    fn full_line_and_trailing_comments_are_removed() {
        let src = "# note\na = \"1\" # trailing\n";
        assert_eq!(strip_toml_comments(src), "\na = \"1\" \n");
    }

    #[test]
    fn tera_syntax_inside_comments_becomes_inert() {
        let src = "# sample: a = \"{{ vars.missing }}\"\n# disable: {% if false %}\nb = 1\n";
        let got = strip_toml_comments(src);
        assert_eq!(got, "\n\nb = 1\n");
    }

    #[test]
    fn hash_inside_strings_survives() {
        let src = concat!(
            "basic = \"https://example.com/#anchor\"\n",
            "literal = 'a#b'\n",
            "escaped = \"quote\\\" then #not-a-comment\"\n",
        );
        assert_eq!(strip_toml_comments(src), src);
    }

    #[test]
    fn hash_inside_multiline_strings_survives() {
        let src = concat!(
            "ml_basic = \"\"\"\n",
            "# still content\n",
            "\"\"\"\n",
            "ml_literal = '''\n",
            "# also content\n",
            "'''\n",
            "after = 1 # gone\n",
        );
        let got = strip_toml_comments(src);
        assert!(got.contains("# still content"));
        assert!(got.contains("# also content"));
        assert!(got.ends_with("after = 1 \n"));
    }

    #[test]
    fn multiline_terminator_with_content_quotes_closes_the_string() {
        // TOML allows one or two quotes immediately inside the delimiters, so
        // the terminating run is 3-5 quotes and only its last three close the
        // string. Matching the first three of a four-quote run left a stray
        // quote behind, which opened a bogus string and hid the real comment.
        let src = concat!(
            "basic = \"\"\"ends with a quote\"\"\"\" # {% if %}\n",
            "wide = \"\"\"\"\"two inside\"\"\"\"\" # {{ vars.missing }}\n",
            "literal = '''ends with a quote'''' # dropped\n",
            "plain = 1 # dropped\n",
        );
        let got = strip_toml_comments(src);
        assert_eq!(
            got,
            concat!(
                "basic = \"\"\"ends with a quote\"\"\"\" \n",
                "wide = \"\"\"\"\"two inside\"\"\"\"\" \n",
                "literal = '''ends with a quote'''' \n",
                "plain = 1 \n",
            )
        );

        // The stripped text is still the TOML the author wrote.
        let parsed: toml::Table = got.parse().unwrap();
        assert_eq!(parsed["basic"].as_str(), Some("ends with a quote\""));
        assert_eq!(parsed["wide"].as_str(), Some("\"\"two inside\"\""));
        assert_eq!(parsed["literal"].as_str(), Some("ends with a quote'"));
    }

    #[test]
    fn hash_inside_tera_delimiters_survives() {
        let src = concat!(
            "a = {{ \"x#y\" }}\n",
            "{% if sep == \"#\" %}\n",
            "b = 1\n",
            "{% endif %}\n",
            "{# tera comment with a # inside #}\n",
            "c = 2 # dropped\n",
        );
        let got = strip_toml_comments(src);
        assert!(got.contains("{{ \"x#y\" }}"));
        assert!(got.contains("sep == \"#\""));
        assert!(got.contains("{# tera comment with a # inside #}"));
        assert!(got.ends_with("c = 2 \n"));
    }

    #[test]
    fn closing_token_inside_a_tera_string_does_not_end_the_tag() {
        // Without tracking Tera's own string literals, the `}}` / `%}` inside
        // one closes the tag early — and the `#` that follows then reads as a
        // TOML comment, silently truncating the line instead of failing.
        let src = concat!(
            "x = {{ \"a}}b#c\" }}\n",
            "y = {{ v | replace(from=\"}}\", to=\"#\") }}\n",
            "{% if v == \"%}#\" %}\n",
            "z = {{ `back}}tick#` }}\n",
            "w = {{ v | default(value='}}#') }}\n",
        );
        assert_eq!(strip_toml_comments(src), src);
    }

    #[test]
    fn tera_string_state_does_not_leak_past_the_tag() {
        // The quote tracking must be cleared with the tag: a trailing comment
        // on the same line is still a comment.
        let src = "x = {{ v | replace(from=\"}}\") }} # dropped\n";
        assert_eq!(
            strip_toml_comments(src),
            "x = {{ v | replace(from=\"}}\") }} \n"
        );
    }

    #[test]
    fn multiline_tera_statement_keeps_hash_across_lines() {
        let src = concat!(
            "{% if a == \"x\"\n",
            "     and b == \"#\" %}\n",
            "v = 1\n",
            "{% endif %}\n",
        );
        assert_eq!(strip_toml_comments(src), src);
    }

    #[test]
    fn line_structure_is_preserved() {
        let src = "# a\n# b\nc = 1\n# d";
        let got = strip_toml_comments(src);
        assert_eq!(got, "\n\nc = 1\n");
        assert_eq!(got.split('\n').count(), src.split('\n').count());
    }

    #[test]
    fn crlf_lines_keep_their_content() {
        let src = "a = \"1\"\r\nb = 2 # note\r\n";
        assert_eq!(strip_toml_comments(src), "a = \"1\"\r\nb = 2 \n");
    }

    #[test]
    fn unterminated_single_line_string_does_not_swallow_later_comments() {
        // Malformed TOML (the parser will reject it later), but the scanner
        // must not treat the rest of the file as string content.
        let src = "broken = \"oops\na = 1 # gone\n";
        assert_eq!(strip_toml_comments(src), "broken = \"oops\na = 1 \n");
    }

    #[test]
    fn inline_table_is_not_mistaken_for_a_tera_tag() {
        let src = "t = { a = 1, b = \"x\" } # gone\n";
        assert_eq!(strip_toml_comments(src), "t = { a = 1, b = \"x\" } \n");
    }
}
