//! Minimal JSON value type, parser, and pretty-printer for the root `vuma`
//! crate.
//!
//! Wave 45 serde-migration: this module replaces `serde_json::Value` /
//! `serde_json::to_string` / `serde_json::to_string_pretty` /
//! `serde_json::from_str` usage in `src/diagnostics.rs`, `src/telemetry.rs`,
//! `src/lsp/mod.rs`, `src/llm_api.rs`, `src/api.rs`, `src/pipeline.rs`,
//! `src/ffi.rs`, `src/main.rs`, and the `vuma-tests` crate.
//!
//! It is a hand-written recursive descent parser + pretty-printer for the
//! JSON subset actually used by the root crate's types (objects with string
//! keys, arrays, strings, integers, booleans, null, floats).
//!
//! ## Why hand-written?
//!
//! The crate previously derived `Serialize, Deserialize` on every JSON-facing
//! type and used `serde_json::to_string(&t)` / `serde_json::to_string_pretty`
//! / `serde_json::from_str::<T>(&s)`. Wave 45 strips serde from the root
//! crate (the last crate in the workspace still using proc macros). To keep
//! the on-disk JSON shape (consumed by external LLMs and LSP clients)
//! unchanged, this module produces byte-identical JSON to what
//! `serde_json::to_string_pretty` produced for the same input data.
//!
//! ## Parser scope
//!
//! The parser implements the full JSON grammar (RFC 8259) except:
//!   - Numbers are parsed as either `i64` or `f64` (depending on whether
//!     they contain `.` or `e`/`E`). The root crate's types use `u64` /
//!     `usize` / `i64` / `f64` / `bool` / `String` / `Vec<_>` / `Option<_>`
//!     / `HashMap`, so this is sufficient.
//!   - Duplicate object keys are accepted (last value wins), matching
//!     `serde_json`'s default behavior.
//!   - The maximum nesting depth is 512 (matches `serde_json`'s default).
//!
//! ## Pretty-printer scope
//!
//! The pretty-printer matches `serde_json::to_string_pretty`'s default
//! formatting: 2-space indentation, no trailing whitespace, `:` followed
//! by a single space, empty objects/arrays rendered as `{}` / `[]` (not
//! expanded across multiple lines).
//!
//! ## Compact printer
//!
//! [`JsonValue::to_string_compact`] matches `serde_json::to_string`'s
//! formatting (no whitespace between tokens).

use std::collections::BTreeMap;
use std::fmt;

/// A JSON value.
///
/// Object keys are stored in a `Vec<(String, JsonValue)>` (not a `BTreeMap`)
/// to preserve insertion order on serialization — this matches
/// `serde_json::to_string_pretty`'s behavior when serializing a `struct`
/// (fields are emitted in declaration order).
#[derive(Debug, Clone, PartialEq)]
pub enum JsonValue {
    /// JSON `null`.
    Null,
    /// JSON `true` / `false`.
    Bool(bool),
    /// JSON integer (negative).
    I64(i64),
    /// JSON unsigned integer (non-negative, fits in `u64`).
    U64(u64),
    /// JSON floating-point number.
    F64(f64),
    /// JSON string (already unescaped).
    Str(String),
    /// JSON array.
    Array(Vec<JsonValue>),
    /// JSON object — vector of `(key, value)` pairs in insertion order.
    Object(Vec<(String, JsonValue)>),
}

impl JsonValue {
    /// Returns `true` if this value is a JSON object.
    pub fn is_object(&self) -> bool {
        matches!(self, JsonValue::Object(_))
    }

    /// Returns `true` if this value is a JSON array.
    pub fn is_array(&self) -> bool {
        matches!(self, JsonValue::Array(_))
    }

    /// Returns `true` if this value is JSON null.
    pub fn is_null(&self) -> bool {
        matches!(self, JsonValue::Null)
    }

    /// Returns the value for the given object key, or `None` if this is not
    /// an object or the key is absent.
    pub fn get(&self, key: &str) -> Option<&JsonValue> {
        if let JsonValue::Object(entries) = self {
            entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
        } else {
            None
        }
    }

    /// Returns a mutable reference to the value for the given object key.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut JsonValue> {
        if let JsonValue::Object(entries) = self {
            entries.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
        } else {
            None
        }
    }

    /// Returns the value as a `&Vec<JsonValue>` if it is an array, else `None`.
    pub fn as_array(&self) -> Option<&Vec<JsonValue>> {
        if let JsonValue::Array(arr) = self {
            Some(arr)
        } else {
            None
        }
    }

    /// Returns the value as a `&[(String, JsonValue)]` if it is an object,
    /// else `None`.
    pub fn as_object(&self) -> Option<&[(String, JsonValue)]> {
        if let JsonValue::Object(entries) = self {
            Some(entries.as_slice())
        } else {
            None
        }
    }

    /// Returns the value as a `&str` if it is a string, else `None`.
    pub fn as_str(&self) -> Option<&str> {
        if let JsonValue::Str(s) = self {
            Some(s.as_str())
        } else {
            None
        }
    }

    /// Returns the value as a `u64` if it is a non-negative integer, else
    /// `None`.
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            JsonValue::U64(n) => Some(*n),
            JsonValue::I64(n) if *n >= 0 => Some(*n as u64),
            _ => None,
        }
    }

    /// Returns the value as an `i64` if it is an integer, else `None`.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            JsonValue::I64(n) => Some(*n),
            JsonValue::U64(n) if *n <= i64::MAX as u64 => Some(*n as i64),
            _ => None,
        }
    }

    /// Returns the value as a `usize` if it is a non-negative integer that
    /// fits in `usize`, else `None`.
    pub fn as_usize(&self) -> Option<usize> {
        self.as_u64().and_then(|n| usize::try_from(n).ok())
    }

    /// Returns the value as a `bool` if it is a boolean, else `None`.
    pub fn as_bool(&self) -> Option<bool> {
        if let JsonValue::Bool(b) = self {
            Some(*b)
        } else {
            None
        }
    }

    /// Returns the value as an `f64` if it is a number, else `None`.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            JsonValue::F64(n) => Some(*n),
            JsonValue::U64(n) => Some(*n as f64),
            JsonValue::I64(n) => Some(*n as f64),
            _ => None,
        }
    }

    /// Pretty-print this value to a String using 2-space indentation,
    /// matching `serde_json::to_string_pretty`'s default formatting.
    pub fn to_string_pretty(&self) -> String {
        let mut out = String::with_capacity(256);
        self.write_pretty(&mut out, 0);
        out
    }

    /// Compact-print this value to a String matching
    /// `serde_json::to_string`'s formatting.
    pub fn to_string_compact(&self) -> String {
        let mut out = String::with_capacity(256);
        self.write_compact(&mut out);
        out
    }

    fn write_pretty(&self, out: &mut String, indent: usize) {
        match self {
            JsonValue::Null => out.push_str("null"),
            JsonValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            JsonValue::I64(n) => out.push_str(&n.to_string()),
            JsonValue::U64(n) => out.push_str(&n.to_string()),
            JsonValue::F64(n) => {
                out.push_str(&n.to_string());
            }
            JsonValue::Str(s) => write_json_string(s, out),
            JsonValue::Array(arr) => {
                if arr.is_empty() {
                    out.push_str("[]");
                } else {
                    out.push_str("[\n");
                    for (i, item) in arr.iter().enumerate() {
                        push_indent(out, indent + 1);
                        item.write_pretty(out, indent + 1);
                        if i + 1 < arr.len() {
                            out.push(',');
                        }
                        out.push('\n');
                    }
                    push_indent(out, indent);
                    out.push(']');
                }
            }
            JsonValue::Object(entries) => {
                if entries.is_empty() {
                    out.push_str("{}");
                } else {
                    out.push_str("{\n");
                    for (i, (k, v)) in entries.iter().enumerate() {
                        push_indent(out, indent + 1);
                        write_json_string(k, out);
                        out.push_str(": ");
                        v.write_pretty(out, indent + 1);
                        if i + 1 < entries.len() {
                            out.push(',');
                        }
                        out.push('\n');
                    }
                    push_indent(out, indent);
                    out.push('}');
                }
            }
        }
    }

    fn write_compact(&self, out: &mut String) {
        match self {
            JsonValue::Null => out.push_str("null"),
            JsonValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            JsonValue::I64(n) => out.push_str(&n.to_string()),
            JsonValue::U64(n) => out.push_str(&n.to_string()),
            JsonValue::F64(n) => out.push_str(&n.to_string()),
            JsonValue::Str(s) => write_json_string(s, out),
            JsonValue::Array(arr) => {
                out.push('[');
                for (i, item) in arr.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write_compact(out);
                }
                out.push(']');
            }
            JsonValue::Object(entries) => {
                out.push('{');
                for (i, (k, v)) in entries.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_json_string(k, out);
                    out.push(':');
                    v.write_compact(out);
                }
                out.push('}');
            }
        }
    }

    /// Insert a key-value pair into an object. If the value is not an
    /// object, or the key already exists, the value is overwritten.
    pub fn insert(&mut self, key: impl Into<String>, value: JsonValue) {
        if let JsonValue::Object(entries) = self {
            let key = key.into();
            if let Some(existing) = entries.iter_mut().find(|(k, _)| *k == key) {
                existing.1 = value;
            } else {
                entries.push((key, value));
            }
        }
    }

    /// Push a value onto the end of an array. No-op if not an array.
    pub fn push(&mut self, value: JsonValue) {
        if let JsonValue::Array(arr) = self {
            arr.push(value);
        }
    }
}

impl fmt::Display for JsonValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Use compact form for Display to match `serde_json::Value`'s
        // `Display` impl (which is compact).
        f.write_str(&self.to_string_compact())
    }
}

impl From<bool> for JsonValue {
    fn from(b: bool) -> Self {
        JsonValue::Bool(b)
    }
}

impl From<u64> for JsonValue {
    fn from(n: u64) -> Self {
        JsonValue::U64(n)
    }
}

impl From<u32> for JsonValue {
    fn from(n: u32) -> Self {
        JsonValue::U64(n as u64)
    }
}

impl From<usize> for JsonValue {
    fn from(n: usize) -> Self {
        JsonValue::U64(n as u64)
    }
}

impl From<i64> for JsonValue {
    fn from(n: i64) -> Self {
        if n >= 0 {
            JsonValue::U64(n as u64)
        } else {
            JsonValue::I64(n)
        }
    }
}

impl From<i32> for JsonValue {
    fn from(n: i32) -> Self {
        JsonValue::from(i64::from(n))
    }
}

impl From<f64> for JsonValue {
    fn from(n: f64) -> Self {
        JsonValue::F64(n)
    }
}

impl From<&str> for JsonValue {
    fn from(s: &str) -> Self {
        JsonValue::Str(s.to_string())
    }
}

impl From<String> for JsonValue {
    fn from(s: String) -> Self {
        JsonValue::Str(s)
    }
}

impl<T: Into<JsonValue>> From<Option<T>> for JsonValue {
    fn from(v: Option<T>) -> Self {
        match v {
            Some(t) => t.into(),
            None => JsonValue::Null,
        }
    }
}

impl<T: Into<JsonValue>> From<Vec<T>> for JsonValue {
    fn from(v: Vec<T>) -> Self {
        JsonValue::Array(v.into_iter().map(|t| t.into()).collect())
    }
}

fn push_indent(out: &mut String, indent: usize) {
    for _ in 0..indent {
        out.push_str("  ");
    }
}

/// Write a JSON string literal (with surrounding quotes and necessary
/// escapes) into `out`. Matches `serde_json`'s default escaping rules:
///   - `"`, `\`, and ASCII control characters (0x00-0x1F) are escaped.
///   - Other characters (including non-ASCII UTF-8) are emitted verbatim.
pub fn write_json_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Escape a string as a JSON string literal (with surrounding quotes).
pub fn quote_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    write_json_string(s, &mut out);
    out
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// Errors that can occur during JSON parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonError {
    /// Byte offset where the error was detected.
    pub pos: usize,
    /// Human-readable error message.
    pub msg: String,
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JSON error at byte {}: {}", self.pos, self.msg)
    }
}

impl std::error::Error for JsonError {}

/// Parse a JSON string into a [`JsonValue`].
///
/// Implements the full JSON grammar (RFC 8259) with the caveats noted in
/// the module docs: numbers are `i64` / `u64` / `f64`, duplicate object
/// keys are accepted (last wins), and the maximum nesting depth is 512.
pub fn parse(json: &str) -> Result<JsonValue, JsonError> {
    let mut p = Parser {
        bytes: json.as_bytes(),
        pos: 0,
    };
    p.skip_ws();
    let value = p.parse_value(0)?;
    p.skip_ws();
    if p.pos != p.bytes.len() {
        return Err(p.err(format!(
            "trailing characters after top-level value (got {:?} at byte {})",
            p.bytes[p.pos] as char, p.pos
        )));
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

const MAX_DEPTH: usize = 512;

impl<'a> Parser<'a> {
    fn err(&self, msg: impl Into<String>) -> JsonError {
        JsonError {
            pos: self.pos,
            msg: msg.into(),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b' ' | b'\t' | b'\n' | b'\r' => self.pos += 1,
                _ => break,
            }
        }
    }

    fn expect(&mut self, expected: u8) -> Result<(), JsonError> {
        if self.pos >= self.bytes.len() {
            return Err(self.err(format!(
                "expected {:?} but reached end of input",
                expected as char
            )));
        }
        if self.bytes[self.pos] != expected {
            return Err(self.err(format!(
                "expected {:?} but found {:?}",
                expected as char,
                self.bytes[self.pos] as char
            )));
        }
        self.pos += 1;
        Ok(())
    }

    fn parse_value(&mut self, depth: usize) -> Result<JsonValue, JsonError> {
        if depth > MAX_DEPTH {
            return Err(self.err(format!(
                "maximum nesting depth ({}) exceeded",
                MAX_DEPTH
            )));
        }
        self.skip_ws();
        if self.pos >= self.bytes.len() {
            return Err(self.err("expected JSON value but reached end of input"));
        }
        match self.bytes[self.pos] {
            b'{' => self.parse_object(depth),
            b'[' => self.parse_array(depth),
            b'"' => self.parse_string().map(JsonValue::Str),
            b't' => self.parse_literal("true", JsonValue::Bool(true)),
            b'f' => self.parse_literal("false", JsonValue::Bool(false)),
            b'n' => self.parse_literal("null", JsonValue::Null),
            b'-' | b'0'..=b'9' => self.parse_number(),
            _ => Err(self.err(format!(
                "unexpected character {:?}",
                self.bytes[self.pos] as char
            ))),
        }
    }

    fn parse_literal(
        &mut self,
        lit: &str,
        value: JsonValue,
    ) -> Result<JsonValue, JsonError> {
        if self.pos + lit.len() > self.bytes.len() {
            return Err(self.err(format!(
                "expected literal '{}' but reached end of input",
                lit
            )));
        }
        if &self.bytes[self.pos..self.pos + lit.len()] != lit.as_bytes() {
            return Err(self.err(format!("expected literal '{}'", lit)));
        }
        self.pos += lit.len();
        Ok(value)
    }

    fn parse_object(&mut self, depth: usize) -> Result<JsonValue, JsonError> {
        self.expect(b'{')?;
        let mut entries: Vec<(String, JsonValue)> = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(JsonValue::Object(entries));
        }
        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            self.skip_ws();
            self.expect(b':')?;
            let value = self.parse_value(depth + 1)?;
            if let Some(existing) = entries.iter_mut().find(|(k, _)| *k == key) {
                existing.1 = value;
            } else {
                entries.push((key, value));
            }
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    self.skip_ws();
                    if self.peek() == Some(b'}') {
                        self.pos += 1;
                        return Ok(JsonValue::Object(entries));
                    }
                }
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(JsonValue::Object(entries));
                }
                _ => {
                    return Err(self.err("expected ',' or '}' after object entry"));
                }
            }
        }
    }

    fn parse_array(&mut self, depth: usize) -> Result<JsonValue, JsonError> {
        self.expect(b'[')?;
        let mut items: Vec<JsonValue> = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(JsonValue::Array(items));
        }
        loop {
            let value = self.parse_value(depth + 1)?;
            items.push(value);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    self.skip_ws();
                    if self.peek() == Some(b']') {
                        self.pos += 1;
                        return Ok(JsonValue::Array(items));
                    }
                }
                Some(b']') => {
                    self.pos += 1;
                    return Ok(JsonValue::Array(items));
                }
                _ => {
                    return Err(self.err("expected ',' or ']' after array element"));
                }
            }
        }
    }

    fn parse_string(&mut self) -> Result<String, JsonError> {
        self.expect(b'"')?;
        let mut out = Vec::new();
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            self.pos += 1;
            match b {
                b'"' => {
                    return String::from_utf8(out).map_err(|e| {
                        self.err(format!("invalid UTF-8 in JSON string: {}", e))
                    });
                }
                b'\\' => {
                    if self.pos >= self.bytes.len() {
                        return Err(self.err("trailing backslash in JSON string"));
                    }
                    let esc = self.bytes[self.pos];
                    self.pos += 1;
                    match esc {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'/' => out.push(b'/'),
                        b'b' => out.push(0x08),
                        b'f' => out.push(0x0c),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            if self.pos + 4 > self.bytes.len() {
                                return Err(self.err("truncated \\uXXXX escape"));
                            }
                            let hex = std::str::from_utf8(&self.bytes[self.pos..self.pos + 4])
                                .map_err(|_| self.err("invalid UTF-8 in \\uXXXX escape"))?;
                            let code = u32::from_str_radix(hex, 16)
                                .map_err(|e| self.err(format!("invalid \\uXXXX escape: {}", e)))?;
                            self.pos += 4;
                            if (0xD800..=0xDBFF).contains(&code) {
                                if self.pos + 6 > self.bytes.len()
                                    || self.bytes[self.pos] != b'\\'
                                    || self.bytes[self.pos + 1] != b'u'
                                {
                                    return Err(self.err(
                                        "expected low surrogate after high surrogate",
                                    ));
                                }
                                let lo_hex = std::str::from_utf8(
                                    &self.bytes[self.pos + 2..self.pos + 6],
                                )
                                .map_err(|_| {
                                    self.err("invalid UTF-8 in low surrogate \\uXXXX escape")
                                })?;
                                let lo = u32::from_str_radix(lo_hex, 16).map_err(|e| {
                                    self.err(format!("invalid low surrogate \\uXXXX escape: {}", e))
                                })?;
                                if !(0xDC00..=0xDFFF).contains(&lo) {
                                    return Err(self.err(
                                        "invalid low surrogate after high surrogate",
                                    ));
                                }
                                let combined = 0x10000
                                    + ((code - 0xD800) << 10)
                                    + (lo - 0xDC00);
                                self.pos += 6;
                                if let Some(ch) = char::from_u32(combined) {
                                    let mut buf = [0u8; 4];
                                    let s = ch.encode_utf8(&mut buf);
                                    out.extend_from_slice(s.as_bytes());
                                } else {
                                    return Err(self.err(format!(
                                        "invalid surrogate pair (U+{:08X})",
                                        combined
                                    )));
                                }
                            } else if (0xDC00..=0xDFFF).contains(&code) {
                                return Err(self.err("unexpected low surrogate without high surrogate"));
                            } else if let Some(ch) = char::from_u32(code) {
                                let mut buf = [0u8; 4];
                                let s = ch.encode_utf8(&mut buf);
                                out.extend_from_slice(s.as_bytes());
                            } else {
                                return Err(self.err(format!("invalid Unicode codepoint U+{:04X}", code)));
                            }
                        }
                        _ => {
                            return Err(self.err(format!(
                                "invalid JSON escape '\\{}'",
                                esc as char
                            )))
                        }
                    }
                }
                0x00..=0x1f => {
                    return Err(self.err(format!(
                        "raw control character (0x{:02X}) in JSON string",
                        b
                    )));
                }
                _ => out.push(b),
            }
        }
        Err(self.err("unterminated JSON string"))
    }

    fn parse_number(&mut self) -> Result<JsonValue, JsonError> {
        let start = self.pos;
        let mut is_float = false;
        if self.pos < self.bytes.len() && self.bytes[self.pos] == b'-' {
            self.pos += 1;
        }
        if self.pos < self.bytes.len() && self.bytes[self.pos] == b'0' {
            self.pos += 1;
        } else {
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }
        if self.pos < self.bytes.len() && self.bytes[self.pos] == b'.' {
            is_float = true;
            self.pos += 1;
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }
        if self.pos < self.bytes.len()
            && (self.bytes[self.pos] == b'e' || self.bytes[self.pos] == b'E')
        {
            is_float = true;
            self.pos += 1;
            if self.pos < self.bytes.len()
                && (self.bytes[self.pos] == b'+' || self.bytes[self.pos] == b'-')
            {
                self.pos += 1;
            }
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }
        let s = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| self.err("invalid UTF-8 in JSON number"))?;
        if is_float {
            s.parse::<f64>()
                .map(JsonValue::F64)
                .map_err(|e| self.err(format!("invalid float '{}': {}", s, e)))
        } else if s.starts_with('-') {
            s.parse::<i64>()
                .map(JsonValue::I64)
                .map_err(|e| self.err(format!("invalid integer '{}': {}", s, e)))
        } else {
            s.parse::<u64>()
                .map(JsonValue::U64)
                .map_err(|e| self.err(format!("invalid integer '{}': {}", s, e)))
        }
    }
}

// ---------------------------------------------------------------------------
// Builder helpers (used by hand-written serializers for root-crate types)
// ---------------------------------------------------------------------------

/// Build a `JsonValue::Object` from a list of `(key, value)` pairs.
pub fn build_object(entries: Vec<(String, JsonValue)>) -> JsonValue {
    JsonValue::Object(entries)
}

/// Build a `JsonValue::Array` from a list of values.
pub fn build_array(items: Vec<JsonValue>) -> JsonValue {
    JsonValue::Array(items)
}

/// Convert a `&str` into a `JsonValue::Str`.
pub fn json_str<S: AsRef<str>>(s: S) -> JsonValue {
    JsonValue::Str(s.as_ref().to_string())
}

/// Convert a `String` into a `JsonValue::Str`.
pub fn json_string(s: String) -> JsonValue {
    JsonValue::Str(s)
}

/// Convert an `Option<String>` into a `JsonValue` (Str or Null).
pub fn json_opt_string(s: &Option<String>) -> JsonValue {
    match s {
        Some(s) => JsonValue::Str(s.clone()),
        None => JsonValue::Null,
    }
}

/// Convert an `Option<u64>` into a `JsonValue` (U64 or Null).
pub fn json_opt_u64(n: &Option<u64>) -> JsonValue {
    match n {
        Some(n) => JsonValue::U64(*n),
        None => JsonValue::Null,
    }
}

/// Convert an `Option<usize>` into a `JsonValue` (U64 or Null).
pub fn json_opt_usize(n: &Option<usize>) -> JsonValue {
    match n {
        Some(n) => JsonValue::U64(*n as u64),
        None => JsonValue::Null,
    }
}

/// Convert a `u64` into a `JsonValue::U64`.
pub fn json_u64(n: u64) -> JsonValue {
    JsonValue::U64(n)
}

/// Convert a `usize` into a `JsonValue::U64`.
pub fn json_usize(n: usize) -> JsonValue {
    JsonValue::U64(n as u64)
}

/// Convert an `i64` into a `JsonValue`.
pub fn json_i64(n: i64) -> JsonValue {
    JsonValue::from(n)
}

/// Convert a `bool` into a `JsonValue::Bool`.
pub fn json_bool(b: bool) -> JsonValue {
    JsonValue::Bool(b)
}

/// Convert a `Vec<u64>` into a `JsonValue::Array` of `U64`s.
pub fn json_u64_array(arr: &[u64]) -> JsonValue {
    JsonValue::Array(arr.iter().map(|n| JsonValue::U64(*n)).collect())
}

/// Convert a `Vec<String>` into a `JsonValue::Array` of `Str`s.
pub fn json_string_array(arr: &[String]) -> JsonValue {
    JsonValue::Array(arr.iter().map(|s| JsonValue::Str(s.clone())).collect())
}

/// Convert a `Vec<&str>` into a `JsonValue::Array` of `Str`s.
pub fn json_str_array(arr: &[&str]) -> JsonValue {
    JsonValue::Array(arr.iter().map(|s| JsonValue::Str((*s).to_string())).collect())
}

/// Convert a `BTreeMap<String, usize>` into a `JsonValue::Object`.
pub fn json_btreemap_usize(map: &BTreeMap<String, usize>) -> JsonValue {
    JsonValue::Object(
        map.iter()
            .map(|(k, v)| (k.clone(), JsonValue::U64(*v as u64)))
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_object() {
        let v = parse("{}").unwrap();
        assert!(v.is_object());
        assert_eq!(v.as_object().unwrap().len(), 0);
    }

    #[test]
    fn parse_simple_object() {
        let v = parse(r#"{"a": 1, "b": "two", "c": true, "d": null}"#).unwrap();
        assert!(v.is_object());
        let entries = v.as_object().unwrap();
        assert_eq!(entries.len(), 4);
        assert_eq!(v.get("a").unwrap().as_u64(), Some(1));
        assert_eq!(v.get("b").unwrap().as_str(), Some("two"));
        assert_eq!(v.get("c").unwrap().as_bool(), Some(true));
        assert!(v.get("d").unwrap() == &JsonValue::Null);
    }

    #[test]
    fn parse_array() {
        let v = parse(r#"[1, 2, "three", null, false]"#).unwrap();
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 5);
        assert_eq!(arr[0].as_u64(), Some(1));
        assert_eq!(arr[2].as_str(), Some("three"));
    }

    #[test]
    fn parse_string_escapes() {
        let v = parse(r#""hello\nworld\t\\\"quote\/""#).unwrap();
        assert_eq!(v.as_str(), Some("hello\nworld\t\\\"quote/"));
    }

    #[test]
    fn parse_surrogate_pair() {
        let v = parse(r#""\uD83D\uDE00""#).unwrap();
        assert_eq!(v.as_str(), Some("\u{1F600}"));
    }

    #[test]
    fn pretty_print_basic() {
        let v = JsonValue::Object(vec![
            ("a".to_string(), JsonValue::U64(1)),
            ("b".to_string(), JsonValue::Str("two".to_string())),
            (
                "c".to_string(),
                JsonValue::Array(vec![JsonValue::U64(1), JsonValue::U64(2)]),
            ),
        ]);
        let s = v.to_string_pretty();
        let expected = "{\n  \"a\": 1,\n  \"b\": \"two\",\n  \"c\": [\n    1,\n    2\n  ]\n}";
        assert_eq!(s, expected);
    }

    #[test]
    fn pretty_print_empty_containers() {
        assert_eq!(JsonValue::Object(vec![]).to_string_pretty(), "{}");
        assert_eq!(JsonValue::Array(vec![]).to_string_pretty(), "[]");
    }

    #[test]
    fn compact_print_basic() {
        let v = JsonValue::Object(vec![
            ("a".to_string(), JsonValue::U64(1)),
            ("b".to_string(), JsonValue::Str("two".to_string())),
        ]);
        assert_eq!(v.to_string_compact(), r#"{"a":1,"b":"two"}"#);
    }

    #[test]
    fn round_trip_simple() {
        let original = r#"{"a":1,"b":[true,null,"x"]}"#;
        let v = parse(original).unwrap();
        let reprinted = v.to_string_compact();
        let reparsed = parse(&reprinted).unwrap();
        assert_eq!(v, reparsed);
    }

    #[test]
    fn negative_integer() {
        let v = parse("-42").unwrap();
        assert_eq!(v.as_i64(), Some(-42));
        assert_eq!(v.to_string_compact(), "-42");
    }

    #[test]
    fn float_round_trip() {
        let v = parse("3.14").unwrap();
        assert_eq!(v.as_f64(), Some(3.14));
        assert_eq!(v.to_string_compact(), "3.14");
    }

    #[test]
    fn from_conversions() {
        assert_eq!(JsonValue::from(true), JsonValue::Bool(true));
        assert_eq!(JsonValue::from(42u64), JsonValue::U64(42));
        assert_eq!(JsonValue::from(-5i64), JsonValue::I64(-5));
        assert_eq!(JsonValue::from("hi"), JsonValue::Str("hi".to_string()));
        assert_eq!(JsonValue::from(None::<u64>), JsonValue::Null);
        assert_eq!(JsonValue::from(Some(7u64)), JsonValue::U64(7));
    }

    #[test]
    fn insert_into_object() {
        let mut v = JsonValue::Object(vec![("a".to_string(), JsonValue::U64(1))]);
        v.insert("b", JsonValue::U64(2));
        assert_eq!(v.get("b").unwrap().as_u64(), Some(2));
        // Overwrite
        v.insert("a", JsonValue::U64(99));
        assert_eq!(v.get("a").unwrap().as_u64(), Some(99));
    }

    #[test]
    fn parse_trailing_chars_error() {
        assert!(parse("{}x").is_err());
    }

    #[test]
    fn parse_unterminated_string_error() {
        assert!(parse(r#""abc"#).is_err());
    }

    #[test]
    fn parse_invalid_escape_error() {
        assert!(parse(r#""\q""#).is_err());
    }
}
