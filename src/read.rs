//! The list's JSON, read by hand: one small parser into a [`Value`] tree, and a [`Read`] for every schema type, in place of serde's derived `Deserialize`, which cost the module about a fifth of its code.
//!
//! The parser takes RFC 8259 JSON and nothing looser: no leading zero, no trailing comma, no control character in a string, nothing after the value, and no nesting deeper than serde_json's limit of 128. Its three sharp edges are the list's:
//!
//! - **A number is the double JavaScript parsed.** The literal is checked against JSON's grammar, then read by Rust's `f64::from_str`, which rounds correctly, as serde_json with `float_roundtrip` does (`tests/corpus.rs` holds the two equal, bit for bit, on every number of the corpus). A literal past the largest double is refused, as serde_json refuses it.
//! - **A string may hold a lone surrogate**, which `JSON.stringify` writes as `\udXXX` and a DOM text can hold. Such a string is a [`Value::Lone`], kept as UTF-16 code units, and only a [`crate::list::Utf16`] field takes it; every other string field refuses it, as serde_json's `String` did.
//! - **An object keeps its members in the order written**, so a struct can refuse the first key it does not name and an attribute object can be written in its own order.
//!
//! A refusal names where it is as a JSON pointer, `at /pages/0/items/3: missing field `kind``; a syntax error names its byte.

use std::collections::BTreeMap;

/// A JSON value as the reader holds it.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    /// A string holding a lone surrogate, as UTF-16 code units.
    Lone(Vec<u16>),
    Array(Vec<Value>),
    /// The members in the order written.
    Object(Vec<(String, Value)>),
}

impl Value {
    /// The value as a refusal names its type.
    pub fn what(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "a boolean",
            Value::Num(_) => "a number",
            Value::Str(_) | Value::Lone(_) => "a string",
            Value::Array(_) => "an array",
            Value::Object(_) => "an object",
        }
    }

    /// The member `key` of an object, where it is one and has it.
    pub fn member(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(members) => members.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Whether a lone surrogate is anywhere in it.
    pub fn has_lone(&self) -> bool {
        match self {
            Value::Lone(_) => true,
            Value::Array(items) => items.iter().any(Value::has_lone),
            Value::Object(members) => members.iter().any(|(_, v)| v.has_lone()),
            _ => false,
        }
    }
}

/// Parse a JSON text into its value.
///
/// # Errors
/// What is not JSON, with the byte it stopped at.
pub fn parse(text: &str) -> Result<Value, String> {
    let mut p = Parser { text, at: 0 };
    let value = p.value(0)?;
    p.ws();
    if p.at < text.len() {
        return Err(p.err("trailing characters"));
    }
    Ok(value)
}

struct Parser<'a> {
    text: &'a str,
    at: usize,
}

impl Parser<'_> {
    fn err(&self, what: &str) -> String {
        format!("{what} at byte {}", self.at)
    }

    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.at).copied()
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn eat(&mut self, byte: u8) -> bool {
        let found = self.peek() == Some(byte);
        self.at += usize::from(found);
        found
    }

    fn value(&mut self, depth: usize) -> Result<Value, String> {
        self.ws();
        if depth > 128 {
            return Err(self.err("recursion limit exceeded"));
        }
        match self.peek() {
            Some(b'{') => {
                self.at += 1;
                let mut members = Vec::new();
                self.ws();
                if !self.eat(b'}') {
                    loop {
                        self.ws();
                        if self.peek() != Some(b'"') {
                            return Err(self.err("expected a key"));
                        }
                        let Value::Str(key) = self.string()? else { return Err(self.err("a lone surrogate in a key")) };
                        self.ws();
                        if !self.eat(b':') {
                            return Err(self.err("expected `:`"));
                        }
                        members.push((key, self.value(depth + 1)?));
                        self.ws();
                        if self.eat(b'}') {
                            break;
                        }
                        if !self.eat(b',') {
                            return Err(self.err("expected `,` or `}`"));
                        }
                    }
                }
                Ok(Value::Object(members))
            }
            Some(b'[') => {
                self.at += 1;
                let mut items = Vec::new();
                self.ws();
                if !self.eat(b']') {
                    loop {
                        items.push(self.value(depth + 1)?);
                        self.ws();
                        if self.eat(b']') {
                            break;
                        }
                        if !self.eat(b',') {
                            return Err(self.err("expected `,` or `]`"));
                        }
                    }
                }
                Ok(Value::Array(items))
            }
            Some(b'"') => self.string(),
            Some(b't') => self.word("true", Value::Bool(true)),
            Some(b'f') => self.word("false", Value::Bool(false)),
            Some(b'n') => self.word("null", Value::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            None => Err(self.err("EOF while parsing a value")),
            Some(_) => Err(self.err("expected a value")),
        }
    }

    fn word(&mut self, word: &str, value: Value) -> Result<Value, String> {
        if self.text[self.at..].starts_with(word) {
            self.at += word.len();
            Ok(value)
        } else {
            Err(self.err("expected a value"))
        }
    }

    fn digits(&mut self) -> usize {
        let from = self.at;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.at += 1;
        }
        self.at - from
    }

    /// A number literal by JSON's grammar, read as the correctly rounded double.
    fn number(&mut self) -> Result<Value, String> {
        let start = self.at;
        self.eat(b'-');
        let int = self.at;
        let n = self.digits();
        if n == 0 || (n > 1 && self.text.as_bytes()[int] == b'0') || (self.eat(b'.') && self.digits() == 0) {
            return Err(self.err("invalid number"));
        }
        if (self.eat(b'e') || self.eat(b'E')) && {
            let _ = self.eat(b'+') || self.eat(b'-');
            self.digits() == 0
        } {
            return Err(self.err("invalid number"));
        }
        match self.text[start..self.at].parse::<f64>() {
            Ok(n) if n.is_finite() => Ok(Value::Num(n)),
            _ => Err(self.err("number out of range")),
        }
    }

    /// A string literal: borrowed whole where it has no escape, else decoded into code units, a `\uXXXX` kept as the unit it names whether or not it pairs.
    fn string(&mut self) -> Result<Value, String> {
        self.at += 1;
        let start = self.at;
        loop {
            match self.peek() {
                Some(b'"') => {
                    self.at += 1;
                    return Ok(Value::Str(self.text[start..self.at - 1].to_string()));
                }
                Some(b'\\') => break,
                Some(0..0x20) => return Err(self.err("control character (\\u0000-\\u001F) found while parsing a string")),
                Some(_) => self.at += 1,
                None => return Err(self.err("EOF while parsing a string")),
            }
        }
        let mut units: Vec<u16> = self.text[start..self.at].encode_utf16().collect();
        loop {
            match self.peek() {
                Some(b'"') => {
                    self.at += 1;
                    break;
                }
                Some(b'\\') => {
                    self.at += 1;
                    let unit = match self.peek() {
                        Some(b'"') => 0x22,
                        Some(b'\\') => 0x5C,
                        Some(b'/') => 0x2F,
                        Some(b'b') => 0x08,
                        Some(b'f') => 0x0C,
                        Some(b'n') => 0x0A,
                        Some(b'r') => 0x0D,
                        Some(b't') => 0x09,
                        Some(b'u') => {
                            let hex = self.text.get(self.at + 1..self.at + 5).filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()));
                            let unit = hex.and_then(|h| u16::from_str_radix(h, 16).ok()).ok_or_else(|| self.err("invalid \\u escape"))?;
                            self.at += 4;
                            unit
                        }
                        _ => return Err(self.err("invalid escape")),
                    };
                    self.at += 1;
                    units.push(unit);
                }
                Some(0..0x20) => return Err(self.err("control character (\\u0000-\\u001F) found while parsing a string")),
                Some(_) => {
                    let c = self.text[self.at..].chars().next().unwrap_or_default();
                    let mut pair = [0u16; 2];
                    units.extend_from_slice(c.encode_utf16(&mut pair));
                    self.at += c.len_utf8();
                }
                None => return Err(self.err("EOF while parsing a string")),
            }
        }
        Ok(match String::from_utf16(&units) {
            Ok(text) => Value::Str(text),
            Err(_) => Value::Lone(units),
        })
    }
}

/// A refusal from inside `segment`, which names it as one more step of the JSON pointer.
pub fn within(segment: &str, error: String) -> String {
    if error.starts_with('/') { format!("/{segment}{error}") } else { format!("/{segment}: {error}") }
}

/// The refusal of a value of the wrong type.
pub fn invalid(found: &Value, expected: &str) -> String {
    format!("invalid type: {}, expected {expected}", found.what())
}

/// A string value, as a `&str`.
///
/// # Errors
/// A value that is not a string, or holds a lone surrogate.
pub fn str_of(v: &Value) -> Result<&str, String> {
    match v {
        Value::Str(s) => Ok(s),
        Value::Lone(_) => Err("a lone surrogate in a string that is not a text".into()),
        other => Err(invalid(other, "a string")),
    }
}

/// A schema type read from its value.
pub trait Read: Sized {
    /// Read the value.
    ///
    /// # Errors
    /// A value that is not this type, in a sentence naming where.
    fn read(v: &Value) -> Result<Self, String>;

    /// What a member that is not there reads as: `None` for an `Option`, and a refusal for anything else.
    ///
    /// # Errors
    /// The member is required.
    fn absent(key: &str) -> Result<Self, String> {
        Err(format!("missing field `{key}`"))
    }
}

/// An object's members, held to the keys a struct names.
pub struct Members<'a>(&'a [(String, Value)]);

/// The members of an object whose every key is one of `keys`, each once.
///
/// # Errors
/// A value that is not an object, its first key the struct does not name, or a key written twice.
pub fn object<'a>(v: &'a Value, keys: &[&str]) -> Result<Members<'a>, String> {
    let Value::Object(members) = v else { return Err(invalid(v, "an object")) };
    for (at, (key, _)) in members.iter().enumerate() {
        if !keys.contains(&key.as_str()) {
            return Err(format!("unknown field `{key}`"));
        }
        if members[..at].iter().any(|(k, _)| k == key) {
            return Err(format!("duplicate field `{key}`"));
        }
    }
    Ok(Members(members))
}

impl Members<'_> {
    /// The member `key`, read, or what its absence reads as.
    ///
    /// # Errors
    /// The member's own refusal, named by its key.
    pub fn get<T: Read>(&self, key: &str) -> Result<T, String> {
        match self.0.iter().find(|(k, _)| k == key) {
            Some((_, v)) => T::read(v).map_err(|e| within(key, e)),
            None => T::absent(key),
        }
    }
}

impl<T: Read> Read for Option<T> {
    fn read(v: &Value) -> Result<Self, String> {
        match v {
            Value::Null => Ok(None),
            v => T::read(v).map(Some),
        }
    }

    fn absent(_: &str) -> Result<Self, String> {
        Ok(None)
    }
}

impl Read for bool {
    fn read(v: &Value) -> Result<Self, String> {
        match v {
            Value::Bool(b) => Ok(*b),
            other => Err(invalid(other, "a boolean")),
        }
    }
}

impl Read for String {
    fn read(v: &Value) -> Result<Self, String> {
        str_of(v).map(str::to_string)
    }
}

impl<T: Read> Read for Vec<T> {
    fn read(v: &Value) -> Result<Self, String> {
        let Value::Array(items) = v else { return Err(invalid(v, "an array")) };
        items.iter().enumerate().map(|(at, item)| T::read(item).map_err(|e| within(&at.to_string(), e))).collect()
    }
}

impl<T: Read, const N: usize> Read for [T; N] {
    fn read(v: &Value) -> Result<Self, String> {
        let items = Vec::<T>::read(v)?;
        let found = items.len();
        items.try_into().map_err(|_| format!("invalid length {found}, expected an array of {N}"))
    }
}

impl<T: Read> Read for BTreeMap<String, T> {
    fn read(v: &Value) -> Result<Self, String> {
        let Value::Object(members) = v else { return Err(invalid(v, "an object")) };
        members.iter().map(|(k, v)| Ok((k.clone(), T::read(v).map_err(|e| within(k, e))?))).collect()
    }
}

/// An integer-keyed table: each key an array index as JavaScript writes one, `0` or digits with no leading zero.
impl<T: Read> Read for BTreeMap<u32, T> {
    fn read(v: &Value) -> Result<Self, String> {
        let Value::Object(members) = v else { return Err(invalid(v, "an object")) };
        members
            .iter()
            .map(|(k, v)| {
                let key = Some(k).filter(|k| k.bytes().all(|b| b.is_ascii_digit()) && (*k == "0" || !k.starts_with('0'))).and_then(|k| k.parse().ok());
                let key = key.ok_or_else(|| format!("expected an integer key, found {k:?}"))?;
                Ok((key, T::read(v).map_err(|e| within(k, e))?))
            })
            .collect()
    }
}

/// A struct read member by member: every field named with its key, every other key refused.
#[macro_export]
macro_rules! read_struct {
    ($name:ident { $($field:ident: $key:literal),* $(,)? }) => {
        impl $crate::read::Read for $name {
            fn read(v: &$crate::read::Value) -> Result<Self, String> {
                let m = $crate::read::object(v, &[$($key),*])?;
                Ok($name { $($field: m.get($key)?),* })
            }
        }
    };
}

/// An enum of unit variants read from the strings that name them.
#[macro_export]
macro_rules! read_enum {
    ($name:ident { $($variant:ident: $key:literal),* $(,)? }) => {
        impl $crate::read::Read for $name {
            fn read(v: &$crate::read::Value) -> Result<Self, String> {
                match $crate::read::str_of(v)? {
                    $($key => Ok($name::$variant),)*
                    other => Err(format!("unknown variant `{other}`")),
                }
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_parser_takes_json_and_refuses_the_rest() {
        assert_eq!(parse(r#" {"a":[1,-0.5e2,true,null],"b":"x\"\u00e9\ud83d\ude00"} "#).unwrap(), Value::Object(vec![("a".into(), Value::Array(vec![Value::Num(1.0), Value::Num(-50.0), Value::Bool(true), Value::Null])), ("b".into(), Value::Str("x\"é😀".into()))]));
        assert_eq!(parse(r#""a\ud800bé\n""#).unwrap(), Value::Lone(vec![0x61, 0xD800, 0x62, 0xE9, 0x0A]));
        assert!(parse("-0").unwrap() == Value::Num(0.0) && matches!(parse("-0").unwrap(), Value::Num(n) if n.is_sign_negative()));
        for bad in ["", "01", "1.", ".5", "+1", "1e", "-", "1e400", "[1,]", "{\"a\":1,}", "{\"a\" 1}", "\"\t\"", "\"\\x\"", "\"\\u12\"", "\"\\u+123\"", "tru", "1 2", "[", "{\"a\":", "\"abc", &"[".repeat(130)] {
            assert!(parse(bad).is_err(), "{bad:?} is not JSON");
        }
    }
}
