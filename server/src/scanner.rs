use regex::Regex;

/// One TODO/BUG/FIXME/NOTE occurrence. Columns are UTF-16 code units, as LSP expects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    /// Index into the configured tag list.
    pub kind: usize,
    pub line: u32,
    /// Start and length of the keyword, including an optional `(author)` and `:`.
    pub start: u32,
    pub len: u32,
    /// Start and length of the text that follows the keyword (may be empty).
    pub text_start: u32,
    pub text_len: u32,
    pub text: String,
}

pub struct Scanner {
    tags: Vec<String>,
    regex: Regex,
}

/// File extensions that are prose rather than code, so every tag counts, not just tags in comments.
const PROSE_EXTENSIONS: [&str; 7] = ["md", "markdown", "mdx", "txt", "rst", "org", "adoc"];

pub fn is_prose_path(path: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, ext)| PROSE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

impl Scanner {
    pub fn new(tags: &[String]) -> Self {
        let mut sorted: Vec<&String> = tags.iter().collect();
        sorted.sort_by_key(|t| std::cmp::Reverse(t.len()));
        let alternatives = sorted
            .iter()
            .map(|t| regex::escape(t))
            .collect::<Vec<_>>()
            .join("|");
        let regex = Regex::new(&format!(r"\b({alternatives})\b(\([^)\n]*\))?:?")).unwrap();
        Self {
            tags: tags.to_vec(),
            regex,
        }
    }

    pub fn tags(&self) -> &[String] {
        &self.tags
    }

    /// Finds all tags in `text`. With `comment_only`, a tag only counts when it sits inside
    /// a comment, as detected by [`comment_ranges`].
    pub fn scan(&self, text: &str, comment_only: bool) -> Vec<Tag> {
        let mut found = Vec::new();
        let mut in_block: Option<&'static str> = None;

        for (line_no, line) in text.lines().enumerate() {
            let ranges = if comment_only {
                comment_ranges(line, &mut in_block)
            } else {
                vec![(0, line.len())]
            };
            if ranges.is_empty() {
                continue;
            }

            for m in self.regex.captures_iter(line) {
                let whole = m.get(0).unwrap();
                let Some(&(_, range_end)) = ranges
                    .iter()
                    .find(|(s, e)| whole.start() >= *s && whole.start() < *e)
                else {
                    continue;
                };
                let Some(kind) = self.tags.iter().position(|t| t == &m[1]) else {
                    continue;
                };

                let rest = &line[whole.end()..range_end];
                let lead = rest.len() - rest.trim_start_matches([' ', '\t', ':', '-']).len();
                let body = clean_text(&rest[lead..]);
                let text_start = whole.end() + lead;

                found.push(Tag {
                    kind,
                    line: line_no as u32,
                    start: utf16_len(&line[..whole.start()]),
                    len: utf16_len(whole.as_str()),
                    text_start: utf16_len(&line[..text_start]),
                    text_len: utf16_len(body),
                    text: body.to_string(),
                });
            }
        }
        found
    }
}

/// Strips trailing comment closers and whitespace from the text after a tag.
fn clean_text(s: &str) -> &str {
    let mut s = s.trim_end();
    for closer in ["*/", "-->", "-}", "*)"] {
        if let Some(stripped) = s.strip_suffix(closer) {
            s = stripped.trim_end();
        }
    }
    s
}

fn utf16_len(s: &str) -> u32 {
    s.encode_utf16().count() as u32
}

const BLOCK_COMMENTS: [(&str, &str); 4] = [("/*", "*/"), ("<!--", "-->"), ("{-", "-}"), ("(*", "*)")];

/// Byte ranges of `line` that are inside a comment. This is a language-agnostic heuristic:
/// it knows the common line comment markers (`//`, `#`, `--`, `;`, `%`) and block comments
/// (`/* */`, `<!-- -->`, `{- -}`, `(* *)`), carrying block state across lines via `in_block`.
/// It does not understand string literals.
pub fn comment_ranges(line: &str, in_block: &mut Option<&'static str>) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut i = 0;

    while i <= line.len() {
        if let Some(closer) = *in_block {
            match line[i..].find(closer) {
                Some(pos) => {
                    let end = i + pos + closer.len();
                    ranges.push((i, end));
                    *in_block = None;
                    i = end;
                    continue;
                }
                None => {
                    ranges.push((i, line.len()));
                    break;
                }
            }
        }

        let Some((pos, marker)) = next_marker(line, i) else {
            break;
        };
        match marker {
            Marker::Line => {
                ranges.push((pos, line.len()));
                break;
            }
            Marker::Block(opener, closer) => {
                *in_block = Some(closer);
                i = pos + opener.len();
                // Range starts at the opener so `/* TODO */` includes the tag.
                match line[i..].find(closer) {
                    Some(p) => {
                        let end = i + p + closer.len();
                        ranges.push((pos, end));
                        *in_block = None;
                        i = end;
                    }
                    None => {
                        ranges.push((pos, line.len()));
                        break;
                    }
                }
            }
        }
    }

    ranges
}

enum Marker {
    Line,
    Block(&'static str, &'static str),
}

/// Finds the earliest comment marker at or after byte `from`.
fn next_marker(line: &str, from: usize) -> Option<(usize, Marker)> {
    let bytes = line.as_bytes();
    let first_non_space = line.len() - line.trim_start().len();

    for i in from..bytes.len() {
        if !line.is_char_boundary(i) {
            continue;
        }
        let rest = &line[i..];
        let prev_is_space = i == 0 || bytes[i - 1].is_ascii_whitespace();
        let next = bytes.get(i + 1).copied();
        let next2 = bytes.get(i + 2).copied();

        for (opener, closer) in BLOCK_COMMENTS {
            if rest.starts_with(opener) {
                // `(*` only counts as a comment when followed by a space, to skip `(*ptr)`.
                if opener == "(*" && !next2.is_none_or(|c| c.is_ascii_whitespace()) {
                    continue;
                }
                // `{-` only when followed by a space or `-`, to skip things like `{-1}`.
                if opener == "{-" && !next2.is_none_or(|c| c.is_ascii_whitespace() || c == b'-') {
                    continue;
                }
                return Some((i, Marker::Block(opener, closer)));
            }
        }
        if rest.starts_with("//") {
            return Some((i, Marker::Line));
        }
        // `#` at the start of a line or after whitespace: shell, Python, Ruby, YAML, TOML...
        // Not `#[` / `#!` / `#{` (Rust attributes, shebang-like, interpolation).
        if bytes[i] == b'#' && prev_is_space && !matches!(next, Some(b'[' | b'!' | b'{')) {
            return Some((i, Marker::Line));
        }
        // `--` followed by whitespace: SQL, Lua, Haskell, Elm.
        if rest.starts_with("--") && prev_is_space && next2.is_none_or(|c| c.is_ascii_whitespace()) {
            return Some((i, Marker::Line));
        }
        // `;` and `%` only as the first thing on the line: Lisp, assembly, INI, LaTeX, Erlang.
        if i == first_non_space && matches!(bytes[i], b';' | b'%') {
            return Some((i, Marker::Line));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scanner() -> Scanner {
        Scanner::new(&["TODO", "BUG", "FIXME", "NOTE"].map(String::from))
    }

    fn texts(src: &str) -> Vec<(String, String)> {
        let s = scanner();
        s.scan(src, true)
            .into_iter()
            .map(|t| (s.tags()[t.kind].clone(), t.text))
            .collect()
    }

    #[test]
    fn line_comments() {
        let src = "let x = 1; // TODO: fix this\n# FIXME broken\n-- NOTE: sql note\n; BUG lisp";
        assert_eq!(
            texts(src),
            vec![
                ("TODO".into(), "fix this".into()),
                ("FIXME".into(), "broken".into()),
                ("NOTE".into(), "sql note".into()),
                ("BUG".into(), "lisp".into()),
            ]
        );
    }

    #[test]
    fn ignores_code() {
        assert!(texts("let NOTE_COUNT = TODO_LIST;\nfoo(TODO);\ntodo!();").is_empty());
        assert!(texts("#[derive(Debug)] // nothing here").is_empty());
        assert!(texts("i--; x = TODO").is_empty());
    }

    #[test]
    fn block_comments() {
        let src = "/* TODO: one */ code();\n/**\n * FIXME: two\n   NOTE three\n */\nBUG not here";
        assert_eq!(
            texts(src),
            vec![
                ("TODO".into(), "one".into()),
                ("FIXME".into(), "two".into()),
                ("NOTE".into(), "three".into()),
            ]
        );
        assert_eq!(texts("<!-- TODO: html -->"), vec![("TODO".into(), "html".into())]);
    }

    #[test]
    fn author_and_positions() {
        let s = scanner();
        let tags = s.scan("    // TODO(lenni): add tests", true);
        assert_eq!(tags.len(), 1);
        let t = &tags[0];
        assert_eq!((t.line, t.start, t.len), (0, 7, 12)); // "TODO(lenni):"
        assert_eq!(t.text, "add tests");
        assert_eq!((t.text_start, t.text_len), (20, 9));
    }

    #[test]
    fn utf16_columns() {
        let s = scanner();
        let t = &s.scan("// äö😀 TODO: x", true)[0];
        // "// " = 3, "äö" = 2, "😀" = 2 (surrogate pair), " " = 1
        assert_eq!(t.start, 8);
    }

    #[test]
    fn prose_mode_matches_everywhere() {
        let s = scanner();
        assert_eq!(s.scan("TODO: write docs\n- NOTE: hi", false).len(), 2);
        assert!(is_prose_path("README.md"));
        assert!(!is_prose_path("main.rs"));
    }

    #[test]
    fn custom_tags() {
        let s = Scanner::new(&["HACK".to_string(), "TODO".to_string()]);
        let tags = s.scan("// HACK: quick fix\n// FIXME: not configured", true);
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].kind, 0);
    }
}
