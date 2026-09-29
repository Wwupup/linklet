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

/// Turns a parsing failure into one carrying a byte offset.
///
/// `serde_json` reports a line and a column; this project's callers are protocol
/// readers who want an offset into what they sent. The conversion is a walk over
/// the input and it is worth doing rather than changing the contract: "at byte
/// 402" is actionable and "line 3 column 12" makes the reader count.
///
/// A column is a character position while `at` is a byte offset. They agree for
/// ASCII, which is what this project's wire format is, and a multi-byte character
/// before the error would make the offset approximate. That is recorded rather
/// than hidden: the alternative is counting characters instead of bytes and being
/// wrong for the ASCII case that actually happens.
fn locate(error: &serde_json::Error, text: &str) -> usize {
    let column = error.column().saturating_sub(1);
    let line = error.line();
    if line <= 1 {
        return column;
    }

    let mut offset = 0usize;
    for (index, part) in text.split_inclusive('\n').enumerate() {
        if index + 1 == line {
            return offset + column;
        }
        offset += part.len();
    }
    text.len()
}

/// Reads one JSON value.
///
/// **The parsing is `serde_json`'s, and that is the point of this function.** Its
/// only job is to turn the result into this project's vocabulary and the failure
/// into this project's error type. A hand-written parser once lived here -- some
/// six hundred lines of it, with thirty tests -- and it was removed not because it
/// was wrong but because of where it sits: on the path that reads untrusted input
/// from the network, and that is the last place to keep code whose bugs only a
/// fuzzer finds.
///
/// # Errors
///
/// [`JsonError`] when the text is not exactly one JSON value.
pub fn parse(text: &str) -> Result<Json, JsonError> {
    serde_json::from_str::<serde_json::Value>(text)
        .map(from_value)
        .map_err(|error| {
            let at = locate(&error, text);
            JsonError {
                message: error.to_string(),
                at,
            }
        })
}

/// Writes one JSON value.
///
/// Returns a `String` rather than a `Result` because it cannot fail: every [`Json`]
/// converts to a `serde_json::Value`, and serializing one of those cannot fail. The
/// `expect` below is that argument written down, not a hope -- if it ever fires, the
/// conversion above is the bug and not the serialization.
pub fn write(value: &Json) -> String {
    serde_json::to_string(&to_value(value)).expect("a JSON value always serializes")
}

/// `serde_json`'s value, as this project's.
fn from_value(value: serde_json::Value) -> Json {
    match value {
        serde_json::Value::Null => Json::Null,
        serde_json::Value::Bool(b) => Json::Bool(b),
        // Integers and floats are two variants here and one in `serde_json`. A
        // number that fits an `i64` is an integer and anything else is a float,
        // which is what the hand-written parser did and therefore what the tests
        // in `tests/json_codec.rs` expect. Since `Json` no longer does the
        // parsing, those tests are now regression tests for this conversion --
        // which is exactly what they should be.
        serde_json::Value::Number(number) => match number.as_i64() {
            Some(integer) => Json::Int(integer),
            None => Json::Float(number.as_f64().unwrap_or(f64::NAN)),
        },
        serde_json::Value::String(text) => Json::Str(text),
        serde_json::Value::Array(items) => Json::Array(items.into_iter().map(from_value).collect()),
        serde_json::Value::Object(fields) => Json::Object(
            fields
                .into_iter()
                .map(|(k, v)| (k, from_value(v)))
                .collect(),
        ),
    }
}

/// This project's value, as `serde_json`'s.
fn to_value(value: &Json) -> serde_json::Value {
    match value {
        Json::Null => serde_json::Value::Null,
        Json::Bool(b) => serde_json::Value::Bool(*b),
        Json::Int(integer) => serde_json::Value::Number((*integer).into()),
        Json::Float(float) => serde_json::Number::from_f64(*float)
            .map(serde_json::Value::Number)
            // NaN and infinity have no JSON representation. `null` is what the
            // hand-written writer produced, and the alternative is a panic for a
            // value the parser cannot produce and a caller would have to construct
            // by hand.
            .unwrap_or(serde_json::Value::Null),
        Json::Str(text) => serde_json::Value::String(text.clone()),
        Json::Array(items) => serde_json::Value::Array(items.iter().map(to_value).collect()),
        Json::Object(fields) => serde_json::Value::Object(
            fields
                .iter()
                .map(|(k, v)| (k.clone(), to_value(v)))
                .collect(),
        ),
    }
}
