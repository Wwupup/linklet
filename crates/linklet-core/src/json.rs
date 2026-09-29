//! JSON, hand-written, because the core is not allowed a dependency.
//!
//! MCP speaks JSON over stdio, so something in this project has to read and
//! write it. The obvious answer is `serde_json`, and it is the wrong one here
//! for the reason in `docs/rationale.md` rule 1: the crate that decides things
//! cannot link against anything, so a JSON dependency would have to live in an
//! adapter -- and then the *decisions* about what a tool call means would live
//! next to the parsing instead of being testable on their own.
//!
//! The hand-written version is about 300 lines and is the least interesting code
//! in the repository. That is the trade: less interesting, more testable, and no
//! network needed to build it.
//!
//! # What it deliberately does not do
//!
//! - No serialisation of a struct into JSON. Writing produces a `String`, by
//!   hand, at the call site. A derive macro would be shorter and would also hide
//!   which fields go on the wire, which for a protocol is the part worth seeing.
//! - No arbitrary precision. Numbers carry `i64` or `f64`, and a number that
//!   fits neither is a parse error rather than a silently rounded value.
//! - No streaming. A message is parsed whole, because MCP over stdio is
//!   newline-delimited and a message is one line.

use std::collections::BTreeMap;

/// A JSON value.
///
/// `Object` is a `BTreeMap` rather than a `HashMap` so that writing a value
/// twice produces the same bytes. Key order is not meaningful in JSON, and an
/// encoder whose output depends on hashing is an encoder whose test flakes.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A number with no fractional part.
    Int(i64),
    /// A number with one.
    Float(f64),
    /// A string.
    Str(String),
    /// An array.
    Array(Vec<Json>),
    /// An object.
    Object(BTreeMap<String, Json>),
}

impl Json {
    /// Wraps a string.
    pub fn str(value: impl Into<String>) -> Self {
        Self::Str(value.into())
    }

    /// The value as a string, if it is one.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(text) => Some(text),
            _ => None,
        }
    }

    /// The value as a boolean, if it is one.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// The value as an integer, if it is one.
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(value) => Some(*value),
            _ => None,
        }
    }

    /// The value as an array, if it is one.
    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }

    /// One entry of an object, if this is an object with that key.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Self::Object(entries) => entries.get(key),
            _ => None,
        }
    }

    /// A string entry of an object, if it is there and is a string.
    ///
    /// The shape MCP arguments arrive in, and a shape that is wrong far more
    /// often than it is missing, so the two are not distinguished here: a caller
    /// decides what to say about a missing argument, and "absent, or not a
    /// string" is the fact it needs.
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Json::as_str)
    }
}

/// Builds an object from pairs, for writing replies.
///
/// A macro rather than a builder because the call sites are literals and the
/// shape should be visible: `object! { "jsonrpc" => "2.0", "id" => id }` reads
/// like the message it produces.
///
/// The value is borrowed, so a `Json` already in hand is not moved into the
/// message and can still be used afterwards -- which matters for a reply that
/// echoes part of its request.
#[macro_export]
macro_rules! object {
    ($($key:expr => $value:expr),* $(,)?) => {{
        let mut entries = ::std::collections::BTreeMap::new();
        $(
            entries.insert(($key).to_string(), $crate::json::to_json(&$value));
        )*
        $crate::json::Json::Object(entries)
    }};
}

/// Converts a value into [`Json`], for use inside [`object!`].
///
/// A free function rather than only a trait method, because the macro reaches it
/// through `$crate::json::to_json` and a method needs a `use` at every call
/// site, including inside the macro's expansion.
pub fn to_json<T: ToJson + ?Sized>(value: &T) -> Json {
    value.to_json()
}

/// The conversion behind [`to_json`].
///
/// Implemented for the handful of types the protocol actually carries. There is
/// deliberately no blanket conversion: a type that goes on the wire should say
/// so, once, here.
pub trait ToJson {
    /// The JSON form of this value.
    fn to_json(&self) -> Json;
}

impl ToJson for Json {
    fn to_json(&self) -> Json {
        self.clone()
    }
}

impl ToJson for str {
    fn to_json(&self) -> Json {
        Json::Str(self.to_string())
    }
}

impl ToJson for String {
    fn to_json(&self) -> Json {
        Json::Str(self.clone())
    }
}

/// So that a `&String` field of a struct can go into [`object!`] without being
/// copied into a `&str` first. The macro borrows what it is given, which makes
/// the expression `&&String`, and without this the call site has to know that.
impl ToJson for &String {
    fn to_json(&self) -> Json {
        Json::Str((*self).clone())
    }
}

impl ToJson for &str {
    fn to_json(&self) -> Json {
        Json::Str((*self).to_string())
    }
}

impl ToJson for bool {
    fn to_json(&self) -> Json {
        Json::Bool(*self)
    }
}

impl ToJson for i64 {
    fn to_json(&self) -> Json {
        Json::Int(*self)
    }
}

impl ToJson for usize {
    fn to_json(&self) -> Json {
        Json::Int(*self as i64)
    }
}

impl ToJson for u8 {
    fn to_json(&self) -> Json {
        Json::Int(i64::from(*self))
    }
}

impl ToJson for Vec<Json> {
    fn to_json(&self) -> Json {
        Json::Array(self.clone())
    }
}

impl<T: ToJson> ToJson for Option<T> {
    fn to_json(&self) -> Json {
        match self {
            Some(value) => value.to_json(),
            None => Json::Null,
        }
    }
}

/// Why a JSON document could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonError {
    /// What went wrong, in words.
    pub message: String,
    /// The byte offset where it went wrong.
    ///
    /// Carried because the caller is a protocol reader: "unexpected end of
    /// input" on a 4000-byte line is not actionable, and "at byte 402" is.
    pub at: usize,
}

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (at byte {})", self.message, self.at)
    }
}

impl std::error::Error for JsonError {}

fn error<T>(message: impl Into<String>, at: usize) -> Result<T, JsonError> {
    Err(JsonError {
        message: message.into(),
        at,
    })
}

/// Reads one JSON value from the whole of `text`.
///
/// Trailing whitespace is allowed; trailing anything else is not, because a
/// stdio protocol that silently ignores half a line is a protocol that ignores
/// the half carrying the mistake.
///
/// # Errors
///
/// Returns [`JsonError`] with the byte offset of the first thing that did not
/// fit. It never panics and never loops: a hand-written parser fed by a network
/// peer is the classic place for a hang, and the tests here include inputs built
/// to provoke one.
pub fn parse(text: &str) -> Result<Json, JsonError> {
    let bytes = text.as_bytes();
    let mut parser = Parser { bytes, at: 0 };
    parser.skip_whitespace();
    let value = parser.value()?;
    parser.skip_whitespace();
    if parser.at != bytes.len() {
        return error("trailing data after the value", parser.at);
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn literal(&mut self, rest: &str, value: Json) -> Result<Json, JsonError> {
        let start = self.at;
        self.at += 1; // the first character, already matched by the caller
        for byte in rest.bytes() {
            match self.peek() {
                Some(found) if found == byte => self.at += 1,
                _ => return error("incomplete literal", start),
            }
        }
        // A literal followed by a letter is a different, invalid word: `nulll`
        // must not parse as `null` with a trailing `l` that the caller then has
        // to notice.
        if matches!(self.peek(), Some(c) if c.is_ascii_alphanumeric()) {
            return error("literal followed by a letter", start);
        }
        Ok(value)
    }

    fn value(&mut self) -> Result<Json, JsonError> {
        match self.peek() {
            None => error("input ended where a value was expected", self.at),
            Some(b'n') => self.literal("ull", Json::Null),
            Some(b't') => self.literal("rue", Json::Bool(true)),
            Some(b'f') => self.literal("alse", Json::Bool(false)),
            Some(b'"') => self.string().map(Json::Str),
            Some(b'[') => self.array(),
            Some(b'{') => self.object(),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(other) => error(
                format!(
                    "unexpected byte {:?} where a value was expected",
                    other as char
                ),
                self.at,
            ),
        }
    }

    fn string(&mut self) -> Result<String, JsonError> {
        let start = self.at;
        self.at += 1; // opening quote
        let mut out = String::new();

        loop {
            let Some(byte) = self.peek() else {
                return error("string was never closed", start);
            };
            match byte {
                b'"' => {
                    self.at += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.at += 1;
                    let Some(escape) = self.peek() else {
                        return error("escape at end of input", self.at);
                    };
                    self.at += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => out.push(self.unicode_escape()?),
                        other => {
                            return error(
                                format!("unknown escape \\{}", other as char),
                                self.at - 1,
                            );
                        }
                    }
                }
                // A raw control character is invalid in JSON. Rejecting it
                // matters for this protocol: a bare newline inside a string
                // would split one message into two on a line-delimited
                // transport, which is a bug that shows up as a parse failure on
                // the *next* message.
                0x00..=0x1f => {
                    return error("unescaped control character in string", self.at);
                }
                _ => {
                    // Multi-byte UTF-8 is copied byte by byte and validated by
                    // `from_utf8` at the end, so an invalid sequence is one
                    // error rather than a byte-by-byte guess.
                    let start_of_char = self.at;
                    let len = utf8_len(byte);
                    self.at += len;
                    if self.at > self.bytes.len() {
                        return error("string ended inside a character", start_of_char);
                    }
                    match std::str::from_utf8(&self.bytes[start_of_char..self.at]) {
                        Ok(text) => out.push_str(text),
                        Err(_) => return error("invalid UTF-8 in string", start_of_char),
                    }
                }
            }
        }
    }

    /// Reads the four hex digits after `\u`, joining a surrogate pair if one
    /// follows.
    fn unicode_escape(&mut self) -> Result<char, JsonError> {
        let first = self.hex4()?;

        // A high surrogate is only half a character; the low half must follow,
        // and treating it as a character on its own would put a lone surrogate
        // into a `String`, which `char` cannot represent.
        if (0xd800..0xdc00).contains(&first) {
            if self.peek() != Some(b'\\') {
                return error("high surrogate not followed by a low one", self.at);
            }
            self.at += 1;
            if self.peek() != Some(b'u') {
                return error("high surrogate not followed by \\u", self.at);
            }
            self.at += 1;
            let second = self.hex4()?;
            if !(0xdc00..0xe000).contains(&second) {
                return error(
                    "second half of a surrogate pair is not a low surrogate",
                    self.at,
                );
            }
            let combined = 0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00);
            return char::from_u32(combined).ok_or_else(|| JsonError {
                message: "surrogate pair is not a character".into(),
                at: self.at,
            });
        }

        if (0xdc00..0xe000).contains(&first) {
            return error("low surrogate with no high one before it", self.at);
        }

        char::from_u32(first).ok_or_else(|| JsonError {
            message: "escape is not a character".into(),
            at: self.at,
        })
    }

    fn hex4(&mut self) -> Result<u32, JsonError> {
        let start = self.at;
        let mut value = 0u32;
        for _ in 0..4 {
            let Some(byte) = self.peek() else {
                return error("\\u escape was cut short", start);
            };
            let digit = (byte as char).to_digit(16).ok_or_else(|| JsonError {
                message: format!("{:?} is not a hex digit in a \\u escape", byte as char),
                at: self.at,
            })?;
            value = value * 16 + digit;
            self.at += 1;
        }
        Ok(value)
    }

    fn number(&mut self) -> Result<Json, JsonError> {
        let start = self.at;
        if self.peek() == Some(b'-') {
            self.at += 1;
        }

        let digits_start = self.at;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.at += 1;
        }
        if self.at == digits_start {
            return error("a minus sign with no digits after it", start);
        }

        let mut is_float = false;
        if self.peek() == Some(b'.') {
            is_float = true;
            self.at += 1;
            let fraction_start = self.at;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.at += 1;
            }
            if self.at == fraction_start {
                return error("a decimal point with no digits after it", start);
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            is_float = true;
            self.at += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.at += 1;
            }
            let exponent_start = self.at;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.at += 1;
            }
            if self.at == exponent_start {
                return error("an exponent with no digits after it", start);
            }
        }

        let text = std::str::from_utf8(&self.bytes[start..self.at]).map_err(|_| JsonError {
            message: "number is not text".into(),
            at: start,
        })?;

        if is_float {
            return text.parse::<f64>().map(Json::Float).map_err(|_| JsonError {
                message: "number is out of range for f64".into(),
                at: start,
            });
        }

        // An integer too large for i64 becomes a float rather than an error:
        // MCP carries byte counts and identifiers, and a peer that sends
        // 18446744073709551615 has sent a number, not a mistake. Losing
        // precision is reported by the type, which is why this is not silent.
        text.parse::<i64>().map(Json::Int).or_else(|_| {
            text.parse::<f64>().map(Json::Float).map_err(|_| JsonError {
                message: "number is out of range".into(),
                at: start,
            })
        })
    }

    fn array(&mut self) -> Result<Json, JsonError> {
        let start = self.at;
        self.at += 1; // [
        let mut items = Vec::new();
        self.skip_whitespace();

        if self.peek() == Some(b']') {
            self.at += 1;
            return Ok(Json::Array(items));
        }

        loop {
            self.skip_whitespace();
            items.push(self.value()?);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b']') => {
                    self.at += 1;
                    return Ok(Json::Array(items));
                }
                Some(_) => return error("expected ',' or ']' in array", self.at),
                None => return error("array was never closed", start),
            }
        }
    }

    fn object(&mut self) -> Result<Json, JsonError> {
        let start = self.at;
        self.at += 1; // {
        let mut entries = BTreeMap::new();
        self.skip_whitespace();

        if self.peek() == Some(b'}') {
            self.at += 1;
            return Ok(Json::Object(entries));
        }

        loop {
            self.skip_whitespace();
            if self.peek() != Some(b'"') {
                return error("object key must be a string", self.at);
            }
            let key = self.string()?;
            self.skip_whitespace();
            if self.peek() != Some(b':') {
                return error("expected ':' after object key", self.at);
            }
            self.at += 1;
            self.skip_whitespace();
            let value = self.value()?;
            // A duplicate key keeps the last value, as most parsers do. Recorded
            // rather than silent: the alternative, rejecting the message, would
            // break a peer that sent one by accident and nothing would be gained.
            entries.insert(key, value);

            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b'}') => {
                    self.at += 1;
                    return Ok(Json::Object(entries));
                }
                Some(_) => return error("expected ',' or '}' in object", self.at),
                None => return error("object was never closed", start),
            }
        }
    }
}

/// How many bytes the UTF-8 character starting with `first` occupies.
///
/// A leading byte that cannot start a character reports 1, so the slice is
/// non-empty and `from_utf8` produces the error rather than this function
/// inventing one.
fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => 1,
    }
}

/// Writes a value as JSON text, with no trailing newline.
///
/// # Errors
///
/// Never. The signature returns a `String` because there is no input for which
/// writing fails: any [`Json`] has a JSON form. A `Result` here would be a
/// `Result` that is always `Ok`, which teaches callers to ignore it.
pub fn write(value: &Json) -> String {
    let mut out = String::new();
    write_into(value, &mut out);
    out
}

fn write_into(value: &Json, out: &mut String) {
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(true) => out.push_str("true"),
        Json::Bool(false) => out.push_str("false"),
        Json::Int(number) => out.push_str(&number.to_string()),
        Json::Float(number) => {
            // JSON has no NaN or infinity, and `to_string` would emit exactly
            // those words, producing a document no parser accepts. `null` is the
            // honest thing to put on the wire for a number that has no JSON
            // form.
            if number.is_finite() {
                out.push_str(&number.to_string());
            } else {
                out.push_str("null");
            }
        }
        Json::Str(text) => write_string(text, out),
        Json::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_into(item, out);
            }
            out.push(']');
        }
        Json::Object(entries) => {
            out.push('{');
            for (index, (key, item)) in entries.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_string(key, out);
                out.push(':');
                write_into(item, out);
            }
            out.push('}');
        }
    }
}

fn write_string(text: &str, out: &mut String) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            // Every other control character has no short escape, and JSON
            // forbids them raw. This is the branch that keeps a line-delimited
            // transport intact: one raw newline here would be two messages.
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}
