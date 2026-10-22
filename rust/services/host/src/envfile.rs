//! `.env` files: `KEY=value` lines, as Node's `--env-file` reads them
//! (`legacy/ReforgedHost/src/envfile.ts`). Values are written unquoted when they are plain and
//! double-quoted otherwise. [`parse_env`] and [`format_env`] give the Node kit's values and bytes;
//! [`EnvDocument`] keeps a file's every line (comments, blank lines, order, line endings) for
//! changing a value in place.

use service_kit::js;

use crate::error::HostError;

/// Parsed variables in file order. A key set twice keeps its first place and its last value, as a
/// JavaScript object does.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvMap(Vec<(String, String)>);

impl EnvMap {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let (key, value) = (key.into(), value.into());
        match self.0.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => self.0.push((key, value)),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// `/^[A-Za-z_][A-Za-z0-9_]*$/`
pub fn valid_key(key: &str) -> bool {
    let mut chars = key.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// One line's variable, or `None` for a comment, a blank line or a line Node skips.
fn parse_line(raw: &str) -> Option<(String, String)> {
    let line = js::trim(raw);
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let eq = line.find('=').filter(|&eq| eq > 0)?;
    let key = js::trim(&line[..eq]);
    // `.replace(/^export\s+/, "")`
    let key = match key.strip_prefix("export") {
        Some(rest) if rest.starts_with(js::is_space) => rest.trim_start_matches(js::is_space),
        _ => key,
    };
    if !valid_key(key) {
        return None;
    }
    let mut value = js::trim(&line[eq + 1..]).to_owned();
    let bytes = value.as_bytes();
    if bytes.len() >= 2 && (bytes[0] == b'"' || bytes[0] == b'\'') && bytes[bytes.len() - 1] == bytes[0] {
        let quote = bytes[0];
        value = value[1..value.len() - 1].to_owned();
        if quote == b'"' {
            // Three replacements one after the other, as the Node kit does them.
            value = value.replace("\\n", "\n").replace("\\\"", "\"").replace("\\\\", "\\");
        }
    }
    Some((key.to_owned(), value))
}

/// The lines of a text as `split(/\r?\n/)` gives them.
fn split_lines(text: &str) -> impl Iterator<Item = &str> {
    text.split('\n').map(|line| line.strip_suffix('\r').unwrap_or(line))
}

pub fn parse_env(text: &str) -> EnvMap {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut out = EnvMap::default();
    for (key, value) in split_lines(text).filter_map(parse_line) {
        out.insert(key, value);
    }
    out
}

/// `/^[A-Za-z0-9_./:@,+=%-]*$/`
fn plain(value: &str) -> bool {
    value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "_./:@,+=%-".contains(c))
}

pub fn format_value(value: &str) -> Result<String, HostError> {
    if plain(value) {
        return Ok(value.to_owned());
    }
    if value.contains('\n') || value.contains('\r') {
        return Err(HostError::message("multi-line values are not supported in .env files"));
    }
    Ok(format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\"")))
}

/// Lines to write: optional comments (`# ...`) and the values in the given order.
pub fn format_env<K: AsRef<str>, V: AsRef<str>>(header: &[&str], entries: &[(K, V)]) -> Result<String, HostError> {
    let mut lines: Vec<String> = header
        .iter()
        .map(|h| if h.is_empty() { "#".to_owned() } else { format!("# {h}") })
        .collect();
    if !lines.is_empty() {
        lines.push(String::new());
    }
    for (key, value) in entries {
        let key = key.as_ref();
        if !valid_key(key) {
            return Err(HostError::message(format!("bad env key {key}")));
        }
        lines.push(format!("{key}={}", format_value(value.as_ref())?));
    }
    Ok(lines.join("\n") + "\n")
}

/// A `.env` file kept line for line: [`EnvDocument::to_text`] gives back the text it was read from
/// until a value is changed, and a change touches only that variable's line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvDocument {
    /// The text split at `\n`; each piece keeps its `\r`. The last piece is what follows the last
    /// newline (empty for a file that ends with one).
    lines: Vec<String>,
}

impl EnvDocument {
    pub fn parse(text: &str) -> Self {
        Self {
            lines: text.split('\n').map(str::to_owned).collect(),
        }
    }

    /// The variables, as [`parse_env`] reads them.
    pub fn values(&self) -> EnvMap {
        parse_env(&self.to_text())
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.values().get(key).map(str::to_owned)
    }

    /// Sets a variable: its last line (the one Node reads) is rewritten in place, with the same line
    /// ending; a new variable is added at the end.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), HostError> {
        if !valid_key(key) {
            return Err(HostError::message(format!("bad env key {key}")));
        }
        let line = format!("{key}={}", format_value(value)?);
        let first = |index: usize| index == 0;
        let at = self.lines.iter().enumerate().rposition(|(index, raw)| {
            let raw = if first(index) {
                raw.strip_prefix('\u{feff}').unwrap_or(raw)
            } else {
                raw
            };
            parse_line(raw.strip_suffix('\r').unwrap_or(raw)).is_some_and(|(k, _)| k == key)
        });
        match at {
            Some(index) => {
                let old = &self.lines[index];
                let bom = if first(index) && old.starts_with('\u{feff}') {
                    "\u{feff}"
                } else {
                    ""
                };
                let cr = if old.ends_with('\r') { "\r" } else { "" };
                self.lines[index] = format!("{bom}{line}{cr}");
            }
            None => {
                let cr = if self.lines.iter().any(|l| l.ends_with('\r')) {
                    "\r"
                } else {
                    ""
                };
                match self.lines.last_mut() {
                    // The text ends with a newline: the new line goes before the empty last piece.
                    Some(last) if last.is_empty() => {
                        let at = self.lines.len() - 1;
                        self.lines.insert(at, format!("{line}{cr}"));
                    }
                    Some(last) => {
                        last.push_str(cr);
                        self.lines.push(line);
                    }
                    None => self.lines = vec![line, String::new()],
                }
            }
        }
        Ok(())
    }

    pub fn to_text(&self) -> String {
        self.lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> EnvMap {
        let mut m = EnvMap::default();
        for (k, v) in pairs {
            m.insert(*k, *v);
        }
        m
    }

    // config.test.ts: "env files round-trip plain, quoted and base64 values"
    #[test]
    fn round_trip_plain_quoted_and_base64_values() {
        let text = format_env(
            &["a comment"],
            &[
                ("A", "plain"),
                ("B", "has space \"and\" quotes"),
                ("C", "abc+/=="),
                ("D", ""),
                ("E", "C:\\Games\\Dauntless"),
            ],
        )
        .unwrap();
        assert_eq!(
            parse_env(&text),
            map(&[
                ("A", "plain"),
                ("B", "has space \"and\" quotes"),
                ("C", "abc+/=="),
                ("D", ""),
                ("E", "C:\\Games\\Dauntless"),
            ])
        );
        assert_eq!(
            parse_env("# x\n\nexport K=v\nBAD LINE\n=x\nQ='single'\n"),
            map(&[("K", "v"), ("Q", "single")])
        );
        assert!(format_env::<&str, &str>(&[], &[("X", "a\nb")]).is_err());
    }

    #[test]
    fn writes_the_node_kits_bytes() {
        let text = format_env(&["first", "", "second"], &[("A", "x y"), ("B", "1")]).unwrap();
        assert_eq!(text, "# first\n#\n# second\n\nA=\"x y\"\nB=1\n");
        assert_eq!(format_env::<&str, &str>(&[], &[]).unwrap(), "\n");
        assert_eq!(format_env(&[], &[("K", "a\\\"b")]).unwrap(), "K=\"a\\\\\\\"b\"\n");
        assert!(format_env(&[], &[("1BAD", "x")]).is_err());
    }

    #[test]
    fn reads_as_node_does() {
        let parsed =
            parse_env("\u{feff}A=1\r\nA=2\r\n  B = \" spaced \"  \nC=\"a\\nb\"\nD=\"x\nexport  E=e\nexportF=f\n");
        assert_eq!(
            parsed,
            map(&[
                ("A", "2"),
                ("B", " spaced "),
                ("C", "a\nb"),
                ("D", "\"x"),
                ("E", "e"),
                ("exportF", "f")
            ])
        );
        assert_eq!(parsed.iter().next(), Some(("A", "2")), "first place, last value");
        // The three replacements run one after another: `\\n` becomes a backslash and a newline, so
        // the Node kit reads its own `CONTENT_NEWS_FILE="C:\\rr\\...\\news.json"` back with a newline.
        assert_eq!(parse_env("K=\"a\\\\nb\"").get("K"), Some("a\\\nb"));
        assert_eq!(
            parse_env("F=\"C:\\\\rr\\\\config\\\\news.json\"").get("F"),
            Some("C:\\rr\\config\\\news.json")
        );
    }

    #[test]
    fn a_document_reads_back_unchanged_and_changes_one_line() {
        let text = "\u{feff}# Written by hand\r\nA=1\r\n\r\n# keep me\r\nB=\"two words\"\r\nA=3\r\n";
        let mut doc = EnvDocument::parse(text);
        assert_eq!(doc.to_text(), text);
        assert_eq!(doc.get("A").as_deref(), Some("3"));
        doc.set("A", "4").unwrap();
        assert_eq!(
            doc.to_text(),
            "\u{feff}# Written by hand\r\nA=1\r\n\r\n# keep me\r\nB=\"two words\"\r\nA=4\r\n"
        );
        doc.set("C", "new value").unwrap();
        assert!(doc.to_text().ends_with("A=4\r\nC=\"new value\"\r\n"));
        let mut bare = EnvDocument::parse("X=1");
        bare.set("Y", "2").unwrap();
        assert_eq!(bare.to_text(), "X=1\nY=2");
        let mut empty = EnvDocument::default();
        empty.set("Z", "z").unwrap();
        assert_eq!(empty.to_text(), "Z=z\n");
    }
}
