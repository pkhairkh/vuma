//! Minimal hand-written TOML parser and serializer.
//!
//! Replaces the third-party `toml` crate. Supports the subset of TOML used by
//! VUMA `vuma.pkg` manifests and the registry `index.toml` file:
//!
//! - Top-level `[section]` headers, including dotted `[a.b.c]` paths.
//! - Array-of-tables `[[section]]` headers (e.g. `[[target]]`).
//! - `key = value` pairs where the value is one of:
//!   - A basic string `"..."` with `\"`, `\\`, `\n`, `\t`, `\r`, `\b`, `\f`,
//!     `\uXXXX`, and `\UXXXXXXXX` escapes.
//!   - A literal string `'...'` (no escapes).
//!   - An array `[v1, v2, ...]` of values (may span multiple lines, may be
//!     heterogeneous, trailing comma allowed).
//!   - An inline table `{ k1 = v1, k2 = v2 }`.
//! - `#` comments to end of line.
//! - Dotted keys (`a.b.c = "value"`).
//!
//! Unsupported (not used by the VUMA manifest format): integers, floats,
//! booleans, datetimes, multi-line basic/literal strings
//! (`"""..."""`/`'''...'''`), hex/octal/binary integers, array-of-tables at
//! non-root positions inside inline tables.
//!
//! The on-disk TOML format produced by [`to_string_pretty`] is compatible
//! with the format previously produced by the third-party `toml` crate's
//! pretty printer, so existing `vuma.pkg` and `index.toml` files continue to
//! round-trip correctly.

use std::collections::BTreeMap;
use std::fmt;

// ---------------------------------------------------------------------------
// Value
// ---------------------------------------------------------------------------

/// A TOML value (subset).
///
/// Tables use `BTreeMap` so that serialization is deterministic: keys are
/// emitted in lexicographic order, matching the behaviour of the previous
/// third-party `toml` crate's `Map` type (which is also a `BTreeMap` by
/// default).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// A string value.
    String(String),
    /// An array of values (heterogeneous; preserves insertion order).
    Array(Vec<Value>),
    /// A table (key-value map). Keys are sorted lexicographically.
    Table(BTreeMap<String, Value>),
}

impl Value {
    /// Construct an empty `Value::Table`.
    pub fn table() -> Self {
        Value::Table(BTreeMap::new())
    }

    /// Construct an empty `Value::Array`.
    pub fn array() -> Self {
        Value::Array(Vec::new())
    }

    /// Returns `Some(&str)` if this is a `Value::String`, else `None`.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Returns `Some(&[Value])` if this is a `Value::Array`, else `None`.
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(arr) => Some(arr.as_slice()),
            _ => None,
        }
    }

    /// Returns `Some(&BTreeMap)` if this is a `Value::Table`, else `None`.
    pub fn as_table(&self) -> Option<&BTreeMap<String, Value>> {
        match self {
            Value::Table(t) => Some(t),
            _ => None,
        }
    }

    /// Looks up `key` in a table; returns `None` for non-tables or missing
    /// keys.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Table(t) => t.get(key),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// An error produced while parsing a TOML document.
#[derive(Debug, Clone)]
pub struct ParseError {
    /// 1-based line number where the error was detected.
    pub line: usize,
    /// Human-readable error description.
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "toml parse error at line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

/// An error produced while serializing a `Value` to TOML.
#[derive(Debug, Clone)]
pub struct SerializeError {
    /// Human-readable error description.
    pub message: String,
}

impl fmt::Display for SerializeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "toml serialize error: {}", self.message)
    }
}

impl std::error::Error for SerializeError {}

// ---------------------------------------------------------------------------
// Parse
// ---------------------------------------------------------------------------

/// Parse a TOML document into a `Value::Table`.
pub fn parse(input: &str) -> Result<Value, ParseError> {
    Parser::new(input).parse_document()
}

struct Parser<'a> {
    chars: Vec<char>,
    pos: usize,
    line: usize,
    _input: &'a str,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            chars: input.chars().collect(),
            pos: 0,
            line: 1,
            _input: input,
        }
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.chars.len()
    }

    fn peek(&self) -> char {
        if self.pos < self.chars.len() {
            self.chars[self.pos]
        } else {
            '\0'
        }
    }

    fn advance(&mut self) -> char {
        let c = self.peek();
        if c == '\n' {
            self.line += 1;
        }
        self.pos += 1;
        c
    }

    fn skip_inline_ws(&mut self) {
        while !self.is_eof() {
            match self.peek() {
                ' ' | '\t' | '\r' => {
                    self.advance();
                }
                _ => break,
            }
        }
    }

    fn skip_ws_and_comments(&mut self) {
        loop {
            match self.peek() {
                ' ' | '\t' | '\r' | '\n' => {
                    self.advance();
                }
                '#' => {
                    while !self.is_eof() && self.peek() != '\n' {
                        self.advance();
                    }
                }
                _ => break,
            }
        }
    }

    fn skip_to_end_of_line(&mut self) {
        self.skip_inline_ws();
        if self.peek() == '#' {
            while !self.is_eof() && self.peek() != '\n' {
                self.advance();
            }
        }
    }

    fn err<T>(&self, msg: impl Into<String>) -> Result<T, ParseError> {
        Err(ParseError {
            line: self.line,
            message: msg.into(),
        })
    }

    fn parse_document(&mut self) -> Result<Value, ParseError> {
        let mut root: BTreeMap<String, Value> = BTreeMap::new();
        let mut current_path: Vec<String> = Vec::new();

        loop {
            self.skip_ws_and_comments();
            if self.is_eof() {
                break;
            }
            let c = self.peek();
            if c == '[' {
                self.advance();
                let is_array = self.peek() == '[';
                if is_array {
                    self.advance();
                }
                self.skip_inline_ws();
                let path = self.parse_key_path()?;
                self.skip_inline_ws();
                if self.peek() != ']' {
                    return self.err("expected ']' to close section header");
                }
                self.advance();
                if is_array {
                    if self.peek() != ']' {
                        return self.err("expected ']]' to close array-of-tables header");
                    }
                    self.advance();
                }
                self.skip_to_end_of_line();
                if is_array {
                    push_array_table(&mut root, &path, self.line)?;
                } else {
                    ensure_table(&mut root, &path, self.line)?;
                }
                current_path = path;
            } else {
                let key_path = self.parse_key_path()?;
                self.skip_inline_ws();
                if self.peek() != '=' {
                    return self.err(format!(
                        "expected '=' after key, found {:?}",
                        self.peek()
                    ));
                }
                self.advance();
                let value = self.parse_value()?;
                self.skip_to_end_of_line();
                insert_value(&mut root, &current_path, &key_path, value, self.line)?;
            }
        }
        Ok(Value::Table(root))
    }

    fn parse_key_path(&mut self) -> Result<Vec<String>, ParseError> {
        let mut path = Vec::new();
        loop {
            self.skip_inline_ws();
            path.push(self.parse_single_key()?);
            self.skip_inline_ws();
            if self.peek() == '.' {
                self.advance();
                continue;
            }
            break;
        }
        Ok(path)
    }

    fn parse_single_key(&mut self) -> Result<String, ParseError> {
        let c = self.peek();
        if c == '"' {
            self.parse_basic_string()
        } else if c == '\'' {
            self.parse_literal_string()
        } else if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            let mut s = String::new();
            while !self.is_eof() {
                let c = self.peek();
                if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                    s.push(c);
                    self.advance();
                } else {
                    break;
                }
            }
            if s.is_empty() {
                return self.err("empty key");
            }
            Ok(s)
        } else {
            self.err(format!("expected key, found {:?}", c))
        }
    }

    fn parse_basic_string(&mut self) -> Result<String, ParseError> {
        // Assumes current char is '"'.
        self.advance();
        let mut s = String::new();
        loop {
            if self.is_eof() {
                return self.err("unterminated basic string");
            }
            let c = self.peek();
            match c {
                '"' => {
                    self.advance();
                    break;
                }
                '\\' => {
                    self.advance();
                    let esc = self.peek();
                    match esc {
                        '"' => {
                            s.push('"');
                            self.advance();
                        }
                        '\\' => {
                            s.push('\\');
                            self.advance();
                        }
                        'n' => {
                            s.push('\n');
                            self.advance();
                        }
                        't' => {
                            s.push('\t');
                            self.advance();
                        }
                        'r' => {
                            s.push('\r');
                            self.advance();
                        }
                        'b' => {
                            s.push('\u{0008}');
                            self.advance();
                        }
                        'f' => {
                            s.push('\u{000C}');
                            self.advance();
                        }
                        '0' => {
                            s.push('\0');
                            self.advance();
                        }
                        'u' => {
                            self.advance();
                            let code = self.read_hex(4)?;
                            match char::from_u32(code) {
                                Some(ch) => s.push(ch),
                                None => return self.err("invalid unicode codepoint"),
                            }
                        }
                        'U' => {
                            self.advance();
                            let code = self.read_hex(8)?;
                            match char::from_u32(code) {
                                Some(ch) => s.push(ch),
                                None => return self.err("invalid unicode codepoint"),
                            }
                        }
                        _ => return self.err(format!("invalid escape \\{}", esc)),
                    }
                }
                '\n' => {
                    return self.err("unterminated basic string (newline)");
                }
                _ => {
                    s.push(c);
                    self.advance();
                }
            }
        }
        Ok(s)
    }

    fn read_hex(&mut self, n: usize) -> Result<u32, ParseError> {
        let mut code: u32 = 0;
        for _ in 0..n {
            let c = self.peek();
            let d = c.to_digit(16).ok_or_else(|| ParseError {
                line: self.line,
                message: format!("invalid hex digit {:?}", c),
            })?;
            code = code * 16 + d;
            self.advance();
        }
        Ok(code)
    }

    fn parse_literal_string(&mut self) -> Result<String, ParseError> {
        // Assumes current char is '\''.
        self.advance();
        let mut s = String::new();
        loop {
            if self.is_eof() {
                return self.err("unterminated literal string");
            }
            let c = self.peek();
            if c == '\'' {
                self.advance();
                break;
            } else if c == '\n' {
                return self.err("unterminated literal string (newline)");
            } else {
                s.push(c);
                self.advance();
            }
        }
        Ok(s)
    }

    fn parse_value(&mut self) -> Result<Value, ParseError> {
        self.skip_inline_ws();
        let c = self.peek();
        match c {
            '"' => Ok(Value::String(self.parse_basic_string()?)),
            '\'' => Ok(Value::String(self.parse_literal_string()?)),
            '[' => self.parse_array(),
            '{' => self.parse_inline_table(),
            _ => self.err(format!("expected value, found {:?}", c)),
        }
    }

    fn parse_array(&mut self) -> Result<Value, ParseError> {
        // Assumes current char is '['.
        self.advance();
        let mut arr: Vec<Value> = Vec::new();
        loop {
            self.skip_ws_and_comments();
            if self.is_eof() {
                return self.err("unterminated array");
            }
            if self.peek() == ']' {
                self.advance();
                break;
            }
            arr.push(self.parse_value()?);
            self.skip_ws_and_comments();
            match self.peek() {
                ',' => {
                    self.advance();
                    continue;
                }
                ']' => {
                    self.advance();
                    break;
                }
                _ => return self.err(format!("expected ',' or ']' in array, found {:?}", self.peek())),
            }
        }
        Ok(Value::Array(arr))
    }

    fn parse_inline_table(&mut self) -> Result<Value, ParseError> {
        // Assumes current char is '{'.
        self.advance();
        let mut table: BTreeMap<String, Value> = BTreeMap::new();
        loop {
            self.skip_inline_ws();
            if self.is_eof() {
                return self.err("unterminated inline table");
            }
            if self.peek() == '}' {
                self.advance();
                break;
            }
            let key_path = self.parse_key_path()?;
            self.skip_inline_ws();
            if self.peek() != '=' {
                return self.err("expected '=' in inline table");
            }
            self.advance();
            let value = self.parse_value()?;
            insert_into_table(&mut table, &key_path, value, self.line)?;
            self.skip_inline_ws();
            match self.peek() {
                ',' => {
                    self.advance();
                    continue;
                }
                '}' => {
                    self.advance();
                    break;
                }
                _ => return self.err(format!(
                    "expected ',' or '}}' in inline table, found {:?}",
                    self.peek()
                )),
            }
        }
        Ok(Value::Table(table))
    }
}

// ---------------------------------------------------------------------------
// Parse helpers
// ---------------------------------------------------------------------------

/// Navigate from `root` down `path`, creating intermediate tables as needed.
/// Returns a mutable reference to the table at `path`. If a path component
/// refers to an existing array-of-tables, descends into the most recently
/// pushed table element.
fn navigate_mut<'a>(
    root: &'a mut BTreeMap<String, Value>,
    path: &[String],
    line: usize,
) -> Result<&'a mut BTreeMap<String, Value>, ParseError> {
    let mut current = root;
    for key in path {
        let entry = current
            .entry(key.clone())
            .or_insert_with(Value::table);
        match entry {
            Value::Table(t) => current = t,
            Value::Array(arr) => match arr.last_mut() {
                Some(Value::Table(t)) => current = t,
                _ => {
                    return Err(ParseError {
                        line,
                        message: format!("key '{}' is not a table", key),
                    })
                }
            },
            _ => {
                return Err(ParseError {
                    line,
                    message: format!("key '{}' is not a table", key),
                })
            }
        }
    }
    Ok(current)
}

/// Ensure a table exists at `path` (creating intermediate tables as needed).
fn ensure_table(
    root: &mut BTreeMap<String, Value>,
    path: &[String],
    line: usize,
) -> Result<(), ParseError> {
    navigate_mut(root, path, line)?;
    Ok(())
}

/// Push a new empty table onto the array-of-tables at `path`, creating
/// intermediate tables as needed. Returns a mutable reference to the new
/// table.
fn push_array_table<'a>(
    root: &'a mut BTreeMap<String, Value>,
    path: &[String],
    line: usize,
) -> Result<&'a mut BTreeMap<String, Value>, ParseError> {
    if path.is_empty() {
        return Err(ParseError {
            line,
            message: "empty array-of-tables path".to_string(),
        });
    }
    let (last, parents) = path.split_last().unwrap();
    let parent = navigate_mut(root, parents, line)?;
    let entry = parent
        .entry(last.clone())
        .or_insert_with(Value::array);
    match entry {
        Value::Array(arr) => {
            arr.push(Value::table());
            match arr.last_mut().unwrap() {
                Value::Table(t) => Ok(t),
                _ => unreachable!("pushed Value::table()"),
            }
        }
        _ => Err(ParseError {
            line,
            message: format!("key '{}' is not an array", last),
        }),
    }
}

/// Insert `value` at `key_path` inside the table at `current_path` in `root`.
fn insert_value(
    root: &mut BTreeMap<String, Value>,
    current_path: &[String],
    key_path: &[String],
    value: Value,
    line: usize,
) -> Result<(), ParseError> {
    let parent = navigate_mut(root, current_path, line)?;
    insert_into_table(parent, key_path, value, line)
}

/// Insert `value` at `key_path` (possibly dotted) inside `table`.
fn insert_into_table(
    table: &mut BTreeMap<String, Value>,
    key_path: &[String],
    value: Value,
    line: usize,
) -> Result<(), ParseError> {
    if key_path.is_empty() {
        return Err(ParseError {
            line,
            message: "empty key".to_string(),
        });
    }
    if key_path.len() == 1 {
        table.insert(key_path[0].clone(), value);
        return Ok(());
    }
    let (first, rest) = key_path.split_first().unwrap();
    let entry = table
        .entry(first.clone())
        .or_insert_with(Value::table);
    match entry {
        Value::Table(t) => insert_into_table(t, rest, value, line),
        _ => Err(ParseError {
            line,
            message: format!("key '{}' is not a table", first),
        }),
    }
}

// ---------------------------------------------------------------------------
// Serialize
// ---------------------------------------------------------------------------

/// Serialize a `Value::Table` to a TOML string in a pretty (multi-line)
/// format. Top-level non-table values are rejected.
///
/// The output uses `[section]` headers for sub-tables and `[[section]]`
/// headers for arrays of tables. Scalar key-value pairs are emitted inline.
/// Keys within each table are emitted in lexicographic order (because the
/// in-memory representation is a `BTreeMap`).
pub fn to_string_pretty(value: &Value) -> Result<String, SerializeError> {
    let mut out = String::new();
    match value {
        Value::Table(t) => {
            serialize_table_contents(&mut out, t, &[])?;
        }
        _ => {
            return Err(SerializeError {
                message: "top-level value must be a table".to_string(),
            })
        }
    }
    Ok(out)
}

fn serialize_table_contents(
    out: &mut String,
    table: &BTreeMap<String, Value>,
    path: &[String],
) -> Result<(), SerializeError> {
    // Pass 1: emit scalar key-value pairs (strings and arrays-of-strings)
    // directly under the current section header.
    for (k, v) in table {
        match v {
            Value::String(s) => {
                out.push_str(&format_key(k));
                out.push_str(" = ");
                out.push_str(&format_string(s));
                out.push('\n');
            }
            Value::Array(arr)
                if arr.is_empty() || arr.iter().all(|v| matches!(v, Value::String(_))) =>
            {
                out.push_str(&format_key(k));
                out.push_str(" = [");
                for (i, v) in arr.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    if let Value::String(s) = v {
                        out.push_str(&format_string(s));
                    }
                }
                out.push_str("]\n");
            }
            _ => {}
        }
    }
    // Pass 2: emit sub-tables and arrays-of-tables as separate sections.
    for (k, v) in table {
        let mut new_path: Vec<String> = path.to_vec();
        new_path.push(k.clone());
        match v {
            Value::Table(inner) => {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push('[');
                out.push_str(&new_path.join("."));
                out.push_str("]\n");
                serialize_table_contents(out, inner, &new_path)?;
            }
            Value::Array(arr) if arr.iter().any(|v| matches!(v, Value::Table(_))) => {
                for item in arr {
                    if let Value::Table(inner) = item {
                        if !out.is_empty() {
                            out.push('\n');
                        }
                        out.push_str("[[");
                        out.push_str(&new_path.join("."));
                        out.push_str("]]\n");
                        serialize_table_contents(out, inner, &new_path)?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Format a key as a bare key if it matches `[A-Za-z0-9_-]+`, otherwise as
/// a quoted basic string.
fn format_key(k: &str) -> String {
    if !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        k.to_string()
    } else {
        format_string(k)
    }
}

/// Format a string as a basic TOML string with escapes.
fn format_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_section() {
        let input = r#"
[package]
name = "hello"
version = "0.1.0"
"#;
        let v = parse(input).unwrap();
        let t = v.as_table().unwrap();
        let pkg = t.get("package").unwrap().as_table().unwrap();
        assert_eq!(pkg.get("name").unwrap().as_str(), Some("hello"));
        assert_eq!(pkg.get("version").unwrap().as_str(), Some("0.1.0"));
    }

    #[test]
    fn test_parse_array_of_tables() {
        let input = r#"
[[target]]
name = "a"
kind = "bin"

[[target]]
name = "b"
kind = "lib"
"#;
        let v = parse(input).unwrap();
        let arr = v
            .get("target")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0].get("name").unwrap().as_str(), Some("a"));
        assert_eq!(arr[1].get("name").unwrap().as_str(), Some("b"));
    }

    #[test]
    fn test_parse_array_of_strings() {
        let input = r#"
[packages]
vuma-std = ["0.1.0", "0.2.0"]
"#;
        let v = parse(input).unwrap();
        let arr = v
            .get("packages")
            .unwrap()
            .get("vuma-std")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0].as_str(), Some("0.1.0"));
        assert_eq!(arr[1].as_str(), Some("0.2.0"));
    }

    #[test]
    fn test_parse_dotted_section() {
        let input = r#"
[dependencies.vuma-crypto]
version = "0.2"
registry = "custom"
"#;
        let v = parse(input).unwrap();
        let inner = v
            .get("dependencies")
            .unwrap()
            .get("vuma-crypto")
            .unwrap()
            .as_table()
            .unwrap();
        assert_eq!(inner.get("version").unwrap().as_str(), Some("0.2"));
        assert_eq!(inner.get("registry").unwrap().as_str(), Some("custom"));
    }

    #[test]
    fn test_parse_comments_and_blank_lines() {
        let input = r#"
# This is a comment
[package]
name = "hello" # inline comment

# another comment
version = "0.1.0"
"#;
        let v = parse(input).unwrap();
        let pkg = v.get("package").unwrap().as_table().unwrap();
        assert_eq!(pkg.get("name").unwrap().as_str(), Some("hello"));
        assert_eq!(pkg.get("version").unwrap().as_str(), Some("0.1.0"));
    }

    #[test]
    fn test_parse_literal_string() {
        let input = r#"
key = 'literal\nvalue'
"#;
        let v = parse(input).unwrap();
        // In a literal string, \n is two characters, not a newline.
        assert_eq!(v.get("key").unwrap().as_str(), Some("literal\\nvalue"));
    }

    #[test]
    fn test_parse_escape_sequences() {
        let input = r#"
key = "tab\there\nnewline"
"#;
        let v = parse(input).unwrap();
        assert_eq!(
            v.get("key").unwrap().as_str(),
            Some("tab\there\nnewline")
        );
    }

    #[test]
    fn test_parse_multiline_array() {
        let input = r#"
key = [
    "a",
    "b",
    "c",
]
"#;
        let v = parse(input).unwrap();
        let arr = v.get("key").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0].as_str(), Some("a"));
        assert_eq!(arr[2].as_str(), Some("c"));
    }

    #[test]
    fn test_parse_inline_table() {
        let input = r#"
key = { version = "0.2", registry = "custom" }
"#;
        let v = parse(input).unwrap();
        let t = v.get("key").unwrap().as_table().unwrap();
        assert_eq!(t.get("version").unwrap().as_str(), Some("0.2"));
        assert_eq!(t.get("registry").unwrap().as_str(), Some("custom"));
    }

    #[test]
    fn test_roundtrip_simple() {
        let mut root: BTreeMap<String, Value> = BTreeMap::new();
        let mut pkg: BTreeMap<String, Value> = BTreeMap::new();
        pkg.insert("name".to_string(), Value::String("hello".to_string()));
        pkg.insert("version".to_string(), Value::String("0.1.0".to_string()));
        root.insert("package".to_string(), Value::Table(pkg));
        let v = Value::Table(root);
        let s = to_string_pretty(&v).unwrap();
        let parsed = parse(&s).unwrap();
        assert_eq!(v, parsed);
    }

    #[test]
    fn test_roundtrip_with_array_of_tables() {
        let mut root: BTreeMap<String, Value> = BTreeMap::new();
        let mut t1: BTreeMap<String, Value> = BTreeMap::new();
        t1.insert("name".to_string(), Value::String("a".to_string()));
        let mut t2: BTreeMap<String, Value> = BTreeMap::new();
        t2.insert("name".to_string(), Value::String("b".to_string()));
        root.insert(
            "target".to_string(),
            Value::Array(vec![Value::Table(t1), Value::Table(t2)]),
        );
        let v = Value::Table(root);
        let s = to_string_pretty(&v).unwrap();
        let parsed = parse(&s).unwrap();
        assert_eq!(v, parsed);
    }

    #[test]
    fn test_roundtrip_with_nested_table() {
        let mut root: BTreeMap<String, Value> = BTreeMap::new();
        let mut deps: BTreeMap<String, Value> = BTreeMap::new();
        deps.insert(
            "vuma-std".to_string(),
            Value::String("0.1".to_string()),
        );
        let mut inner: BTreeMap<String, Value> = BTreeMap::new();
        inner.insert("version".to_string(), Value::String("0.2".to_string()));
        inner.insert(
            "registry".to_string(),
            Value::String("custom".to_string()),
        );
        deps.insert("vuma-crypto".to_string(), Value::Table(inner));
        root.insert("dependencies".to_string(), Value::Table(deps));
        let v = Value::Table(root);
        let s = to_string_pretty(&v).unwrap();
        let parsed = parse(&s).unwrap();
        assert_eq!(v, parsed);
    }

    #[test]
    fn test_roundtrip_empty_table() {
        let mut root: BTreeMap<String, Value> = BTreeMap::new();
        root.insert("empty".to_string(), Value::table());
        let v = Value::Table(root);
        let s = to_string_pretty(&v).unwrap();
        let parsed = parse(&s).unwrap();
        assert_eq!(v, parsed);
    }

    #[test]
    fn test_roundtrip_array_of_strings() {
        let mut root: BTreeMap<String, Value> = BTreeMap::new();
        let mut packages: BTreeMap<String, Value> = BTreeMap::new();
        packages.insert(
            "vuma-std".to_string(),
            Value::Array(vec![
                Value::String("0.1.0".to_string()),
                Value::String("0.2.0".to_string()),
            ]),
        );
        root.insert("packages".to_string(), Value::Table(packages));
        let v = Value::Table(root);
        let s = to_string_pretty(&v).unwrap();
        let parsed = parse(&s).unwrap();
        assert_eq!(v, parsed);
    }

    #[test]
    fn test_serialize_escapes_special_chars() {
        let mut root: BTreeMap<String, Value> = BTreeMap::new();
        root.insert(
            "key".to_string(),
            Value::String("a\"b\\c\nd\te".to_string()),
        );
        let v = Value::Table(root);
        let s = to_string_pretty(&v).unwrap();
        let parsed = parse(&s).unwrap();
        assert_eq!(v, parsed);
    }

    #[test]
    fn test_parse_error_unterminated_string() {
        let input = r#"
key = "unterminated
"#;
        let result = parse(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_error_missing_equals() {
        let input = r#"
key value
"#;
        let result = parse(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_serialize_top_level_non_table_rejected() {
        let v = Value::String("hello".to_string());
        let result = to_string_pretty(&v);
        assert!(result.is_err());
    }
}
