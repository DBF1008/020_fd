mod input;

use std::ffi::OsString;
use std::fmt::{self, Display, Formatter};
use std::path::Path;
use std::sync::OnceLock;

use aho_corasick::AhoCorasick;

use self::input::{basename, dirname, remove_extension};
use crate::filesystem::replace_path_separator;

/// Designates what should be written to a buffer
///
/// Each `Token` contains either text, or a placeholder variant, which will be used to generate
/// commands after all tokens for a given command template have been collected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token {
    Placeholder,
    Basename,
    Parent,
    NoExt,
    BasenameNoExt,
    Text(String),
}

impl Display for Token {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match *self {
            Token::Placeholder => f.write_str("{}")?,
            Token::Basename => f.write_str("{/}")?,
            Token::Parent => f.write_str("{//}")?,
            Token::NoExt => f.write_str("{.}")?,
            Token::BasenameNoExt => f.write_str("{/.}")?,
            Token::Text(ref string) => f.write_str(string)?,
        }
        Ok(())
    }
}

/// A parsed format string
///
/// This is either a collection of `Token`s including at least one placeholder variant,
/// or a fixed text.
#[derive(Clone, Debug, PartialEq)]
pub enum FormatTemplate {
    Tokens(Vec<Token>),
    Text(String),
}

static PLACEHOLDERS: OnceLock<AhoCorasick> = OnceLock::new();

impl FormatTemplate {
    pub fn has_tokens(&self) -> bool {
        matches!(self, FormatTemplate::Tokens(_))
    }

    pub fn parse(fmt: &str) -> Self {
        // NOTE: we assume that { and } have the same length
        const BRACE_LEN: usize = '{'.len_utf8();
        let mut tokens = Vec::new();
        let mut remaining = fmt;
        let mut buf = String::new();
        let placeholders = PLACEHOLDERS.get_or_init(|| {
            AhoCorasick::new(["{{", "}}", "{}", "{/}", "{//}", "{.}", "{/.}"]).unwrap()
        });
        while let Some(m) = placeholders.find(remaining) {
            match m.pattern().as_u32() {
                0 | 1 => {
                    // we found an escaped {{ or }}, so add
                    // everything up to the first char to the buffer
                    // then skip the second one.
                    buf += &remaining[..m.start() + BRACE_LEN];
                    remaining = &remaining[m.end()..];
                }
                id if !remaining[m.end()..].starts_with('}') => {
                    buf += &remaining[..m.start()];
                    if !buf.is_empty() {
                        tokens.push(Token::Text(std::mem::take(&mut buf)));
                    }
                    tokens.push(token_from_pattern_id(id));
                    remaining = &remaining[m.end()..];
                }
                _ => {
                    // We got a normal pattern, but the final "}"
                    // is escaped, so add up to that to the buffer, then
                    // skip the final }
                    buf += &remaining[..m.end()];
                    remaining = &remaining[m.end() + BRACE_LEN..];
                }
            }
        }
        // Add the rest of the string to the buffer, and add the final buffer to the tokens
        if !remaining.is_empty() {
            buf += remaining;
        }
        if tokens.is_empty() {
            // No placeholders were found, so just return the text
            return FormatTemplate::Text(buf);
        }
        // Add final text segment
        if !buf.is_empty() {
            tokens.push(Token::Text(buf));
        }
        debug_assert!(!tokens.is_empty());
        FormatTemplate::Tokens(tokens)
    }

    /// Generate a result string from this template. If path_separator is Some, then it will replace
    /// the path separator in all placeholder tokens. Fixed text and tokens are not affected by
    /// path separator substitution.
    pub fn generate(&self, path: impl AsRef<Path>, path_separator: Option<&str>) -> OsString {
        use Token::*;
        let path = path.as_ref();

        match *self {
            Self::Tokens(ref tokens) => {
                let mut s = OsString::new();
                for token in tokens {
                    match token {
                        Basename => {
                            s.push(replace_path_separator(basename(path), path_separator))
                        }
                        BasenameNoExt => s.push(replace_path_separator(
                            &remove_extension(basename(path).as_ref()),
                            path_separator,
                        )),
                        NoExt => s.push(replace_path_separator(
                            &remove_extension(path),
                            path_separator,
                        )),
                        Parent => {
                            s.push(replace_path_separator(&dirname(path), path_separator))
                        }
                        Placeholder => {
                            s.push(replace_path_separator(path.as_ref(), path_separator))
                        }
                        Text(string) => s.push(string),
                    }
                }
                s
            }
            Self::Text(ref text) => OsString::from(text),
        }
    }
}

// Convert the id from an aho-corasick match to the
// appropriate token
fn token_from_pattern_id(id: u32) -> Token {
    use Token::*;
    match id {
        2 => Placeholder,
        3 => Basename,
        4 => Parent,
        5 => NoExt,
        6 => BasenameNoExt,
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod fmt_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parse_no_placeholders() {
        let templ = FormatTemplate::parse("This string has no placeholders");
        assert_eq!(
            templ,
            FormatTemplate::Text("This string has no placeholders".into())
        );
    }

    #[test]
    fn parse_only_brace_escapes() {
        let templ = FormatTemplate::parse("This string only has escapes like {{ and }}");
        assert_eq!(
            templ,
            FormatTemplate::Text("This string only has escapes like { and }".into())
        );
    }

    #[test]
    fn all_placeholders() {
        use Token::*;

        let templ = FormatTemplate::parse(
            "{{path={} \
            basename={/} \
            parent={//} \
            noExt={.} \
            basenameNoExt={/.} \
            }}",
        );
        assert_eq!(
            templ,
            FormatTemplate::Tokens(vec![
                Text("{path=".into()),
                Placeholder,
                Text(" basename=".into()),
                Basename,
                Text(" parent=".into()),
                Parent,
                Text(" noExt=".into()),
                NoExt,
                Text(" basenameNoExt=".into()),
                BasenameNoExt,
                Text(" }".into()),
            ])
        );

        let mut path = PathBuf::new();
        path.push("a");
        path.push("folder");
        path.push("file.txt");

        let expanded = templ.generate(&path, Some("/")).into_string().unwrap();

        assert_eq!(
            expanded,
            "{path=a/folder/file.txt \
            basename=file.txt \
            parent=a/folder \
            noExt=a/folder/file \
            basenameNoExt=file }"
        );
    }
}
