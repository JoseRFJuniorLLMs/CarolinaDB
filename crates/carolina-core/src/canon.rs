//! Restricted canonical JSON (SPEC-003 §10, SPEC-012 §8).
//!
//! Rules enforced by encoder and decoder:
//!
//! * object keys are unique ASCII strings sorted bytewise; no insignificant whitespace;
//! * booleans and `null` use JSON literals; **JSON numbers are forbidden** — every
//!   integer, identifier and decimal coefficient is a canonical decimal string
//!   (`"0"`, `"-17"`, no `+`, no redundant leading zeros, no `-0`);
//! * byte arrays are lowercase hexadecimal strings;
//! * strings are valid UTF-8; only `"`/`\` and U+0000–U+001F are escaped, control
//!   characters as lowercase `\u00xx`; no other escapes are accepted;
//! * arrays keep order; sets are arrays sorted by canonical element bytes with duplicates rejected;
//! * decoders re-encode and compare bytes before accepting an artifact.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::error::{CoreError, CoreResult, ErrorCode};
use crate::hash::{hex_decode, hex_encode};
use crate::limits::Limits;

/// Canonical value model. Integers and bytes are represented as [`CanonValue::Str`]
/// in canonical text form; use the typed constructors/accessors.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CanonValue {
    Null,
    Bool(bool),
    Str(String),
    Array(Vec<CanonValue>),
    Object(BTreeMap<String, CanonValue>),
}

impl CanonValue {
    // ---- constructors -------------------------------------------------
    pub fn int(v: i128) -> CanonValue {
        CanonValue::Str(v.to_string())
    }
    pub fn uint(v: u64) -> CanonValue {
        CanonValue::Str(v.to_string())
    }
    pub fn u32(v: u32) -> CanonValue {
        CanonValue::Str(v.to_string())
    }
    pub fn bytes(b: &[u8]) -> CanonValue {
        CanonValue::Str(hex_encode(b))
    }
    pub fn str(s: impl Into<String>) -> CanonValue {
        CanonValue::Str(s.into())
    }
    pub fn obj() -> ObjBuilder {
        ObjBuilder(BTreeMap::new())
    }
    pub fn array(items: Vec<CanonValue>) -> CanonValue {
        CanonValue::Array(items)
    }
    /// Encode a mathematical set: sort elements by canonical bytes, reject duplicates.
    pub fn set(items: Vec<CanonValue>) -> CoreResult<CanonValue> {
        let mut encoded: Vec<(Vec<u8>, CanonValue)> =
            items.into_iter().map(|v| (v.encode(), v)).collect();
        encoded.sort_by(|a, b| a.0.cmp(&b.0));
        for w in encoded.windows(2) {
            if w[0].0 == w[1].0 {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    "duplicate element in canonical set",
                ));
            }
        }
        Ok(CanonValue::Array(
            encoded.into_iter().map(|(_, v)| v).collect(),
        ))
    }
    pub fn option<T: Canonical>(v: &Option<T>) -> CanonValue {
        match v {
            Some(x) => x.to_canon(),
            None => CanonValue::Null,
        }
    }

    // ---- accessors ------------------------------------------------------
    pub fn as_str(&self) -> CoreResult<&str> {
        match self {
            CanonValue::Str(s) => Ok(s),
            _ => Err(type_err("string")),
        }
    }
    pub fn as_bool(&self) -> CoreResult<bool> {
        match self {
            CanonValue::Bool(b) => Ok(*b),
            _ => Err(type_err("bool")),
        }
    }
    pub fn as_i128(&self) -> CoreResult<i128> {
        parse_canonical_int(self.as_str()?)
    }
    pub fn as_i64(&self) -> CoreResult<i64> {
        let v = self.as_i128()?;
        i64::try_from(v)
            .map_err(|_| CoreError::new(ErrorCode::NumericOverflow, "integer out of i64 range"))
    }
    pub fn as_u64(&self) -> CoreResult<u64> {
        let s = self.as_str()?;
        let v = parse_canonical_int(s)?;
        if v < 0 {
            return Err(CoreError::new(
                ErrorCode::NonCanonicalEncoding,
                "negative value where unsigned expected",
            ));
        }
        u64::try_from(v)
            .map_err(|_| CoreError::new(ErrorCode::NumericOverflow, "integer out of u64 range"))
    }
    pub fn as_u32(&self) -> CoreResult<u32> {
        let v = self.as_u64()?;
        u32::try_from(v)
            .map_err(|_| CoreError::new(ErrorCode::NumericOverflow, "integer out of u32 range"))
    }
    pub fn as_u16(&self) -> CoreResult<u16> {
        let v = self.as_u64()?;
        u16::try_from(v)
            .map_err(|_| CoreError::new(ErrorCode::NumericOverflow, "integer out of u16 range"))
    }
    pub fn as_bytes(&self) -> CoreResult<Vec<u8>> {
        hex_decode(self.as_str()?)
    }
    pub fn as_array(&self) -> CoreResult<&[CanonValue]> {
        match self {
            CanonValue::Array(a) => Ok(a),
            _ => Err(type_err("array")),
        }
    }
    pub fn as_object(&self) -> CoreResult<&BTreeMap<String, CanonValue>> {
        match self {
            CanonValue::Object(o) => Ok(o),
            _ => Err(type_err("object")),
        }
    }
    pub fn is_null(&self) -> bool {
        matches!(self, CanonValue::Null)
    }
    /// Required field lookup; missing required fields fail decoding.
    pub fn field(&self, name: &str) -> CoreResult<&CanonValue> {
        self.as_object()?.get(name).ok_or_else(|| {
            CoreError::new(
                ErrorCode::NonCanonicalEncoding,
                format!("missing required field `{name}`"),
            )
        })
    }
    /// Optional field: present-and-null or absent both map to `None`. v1 semantic
    /// records must encode absent optionals as explicit `null`; callers that need
    /// the strict form use [`CanonValue::field`] and check `is_null`.
    pub fn opt_field(&self, name: &str) -> CoreResult<Option<&CanonValue>> {
        Ok(self.as_object()?.get(name).filter(|v| !v.is_null()))
    }
    /// Verify an object has exactly the given field set (no unknown fields: SPEC-012 §10).
    pub fn expect_fields(&self, fields: &[&str]) -> CoreResult<()> {
        let o = self.as_object()?;
        for k in o.keys() {
            if !fields.contains(&k.as_str()) {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("unknown field `{k}`"),
                ));
            }
        }
        for f in fields {
            if !o.contains_key(*f) {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("missing required field `{f}`"),
                ));
            }
        }
        Ok(())
    }
    /// Decode a canonical set: verifies sorted-by-bytes and no duplicates.
    pub fn as_set(&self) -> CoreResult<&[CanonValue]> {
        let a = self.as_array()?;
        let mut prev: Option<Vec<u8>> = None;
        for v in a {
            let e = v.encode();
            if let Some(p) = &prev {
                if e <= *p {
                    return Err(CoreError::new(
                        ErrorCode::NonCanonicalEncoding,
                        "set not sorted or has duplicates",
                    ));
                }
            }
            prev = Some(e);
        }
        Ok(a)
    }

    // ---- encoding ---------------------------------------------------------
    pub fn encode(&self) -> Vec<u8> {
        let mut out = String::new();
        self.write_to(&mut out);
        out.into_bytes()
    }

    fn write_to(&self, out: &mut String) {
        match self {
            CanonValue::Null => out.push_str("null"),
            CanonValue::Bool(true) => out.push_str("true"),
            CanonValue::Bool(false) => out.push_str("false"),
            CanonValue::Str(s) => write_string(s, out),
            CanonValue::Array(a) => {
                out.push('[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    v.write_to(out);
                }
                out.push(']');
            }
            CanonValue::Object(o) => {
                out.push('{');
                for (i, (k, v)) in o.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_string(k, out);
                    out.push(':');
                    v.write_to(out);
                }
                out.push('}');
            }
        }
    }

    /// Strict decoder with the given limits. Re-encodes and compares bytes.
    pub fn decode(bytes: &[u8], limits: &Limits) -> CoreResult<CanonValue> {
        if bytes.len() > limits.max_payload_bytes {
            return Err(CoreError::new(
                ErrorCode::ResourceLimit,
                "payload exceeds max_payload_bytes",
            ));
        }
        let mut p = Parser {
            src: bytes,
            pos: 0,
            limits,
            depth: 0,
        };
        let v = p.parse_value()?;
        if p.pos != bytes.len() {
            return Err(CoreError::new(
                ErrorCode::NonCanonicalEncoding,
                "trailing bytes after canonical value",
            ));
        }
        // re-encode and compare (SPEC-003 §10)
        if v.encode() != bytes {
            return Err(CoreError::new(
                ErrorCode::NonCanonicalEncoding,
                "input is not in canonical form",
            ));
        }
        Ok(v)
    }
}

fn type_err(expected: &str) -> CoreError {
    CoreError::new(
        ErrorCode::NonCanonicalEncoding,
        format!("expected {expected}"),
    )
}

/// Parse a canonical decimal integer string: optional `-`, no `+`, no leading zeros, not `-0`.
pub fn parse_canonical_int(s: &str) -> CoreResult<i128> {
    let b = s.as_bytes();
    if b.is_empty() {
        return Err(CoreError::new(
            ErrorCode::NonCanonicalEncoding,
            "empty integer",
        ));
    }
    let (neg, digits) = if b[0] == b'-' {
        (true, &b[1..])
    } else {
        (false, b)
    };
    if digits.is_empty() || !digits.iter().all(|c| c.is_ascii_digit()) {
        return Err(CoreError::new(
            ErrorCode::NonCanonicalEncoding,
            format!("non-canonical integer `{s}`"),
        ));
    }
    if digits.len() > 1 && digits[0] == b'0' {
        return Err(CoreError::new(
            ErrorCode::NonCanonicalEncoding,
            format!("redundant leading zero in `{s}`"),
        ));
    }
    if neg && digits == b"0" {
        return Err(CoreError::new(
            ErrorCode::NonCanonicalEncoding,
            "signed zero is forbidden",
        ));
    }
    if digits.len() > 39 {
        return Err(CoreError::new(
            ErrorCode::NumericOverflow,
            "integer too large",
        ));
    }
    let mut v: i128 = 0;
    for &c in digits {
        v = v
            .checked_mul(10)
            .and_then(|x| x.checked_add((c - b'0') as i128))
            .ok_or_else(|| {
                CoreError::new(ErrorCode::NumericOverflow, "integer out of i128 range")
            })?;
    }
    Ok(if neg { -v } else { v })
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
    limits: &'a Limits,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }
    fn fail<T>(&self, msg: &str) -> CoreResult<T> {
        Err(CoreError::new(
            ErrorCode::NonCanonicalEncoding,
            format!("{msg} at byte {}", self.pos),
        ))
    }
    fn expect_lit(&mut self, lit: &[u8]) -> CoreResult<()> {
        if self.src[self.pos..].starts_with(lit) {
            self.pos += lit.len();
            Ok(())
        } else {
            self.fail("invalid literal")
        }
    }
    fn parse_value(&mut self) -> CoreResult<CanonValue> {
        match self.peek() {
            None => self.fail("unexpected end"),
            Some(b'n') => {
                self.expect_lit(b"null")?;
                Ok(CanonValue::Null)
            }
            Some(b't') => {
                self.expect_lit(b"true")?;
                Ok(CanonValue::Bool(true))
            }
            Some(b'f') => {
                self.expect_lit(b"false")?;
                Ok(CanonValue::Bool(false))
            }
            Some(b'"') => Ok(CanonValue::Str(self.parse_string()?)),
            Some(b'[') => self.parse_array(),
            Some(b'{') => self.parse_object(),
            Some(c) if c == b'-' || c.is_ascii_digit() => {
                self.fail("JSON numbers are forbidden; use canonical strings")
            }
            Some(c) if c.is_ascii_whitespace() => {
                self.fail("insignificant whitespace is forbidden")
            }
            Some(_) => self.fail("unexpected byte"),
        }
    }
    fn enter(&mut self) -> CoreResult<()> {
        self.depth += 1;
        if self.depth > self.limits.max_depth {
            return Err(CoreError::new(
                ErrorCode::ResourceLimit,
                "nesting depth exceeds limit",
            ));
        }
        Ok(())
    }
    fn parse_array(&mut self) -> CoreResult<CanonValue> {
        self.enter()?;
        self.pos += 1;
        let mut items = Vec::new();
        if self.peek() == Some(b']') {
            self.pos += 1;
            self.depth -= 1;
            return Ok(CanonValue::Array(items));
        }
        loop {
            items.push(self.parse_value()?);
            if items.len() > self.limits.max_collection_len {
                return Err(CoreError::new(
                    ErrorCode::ResourceLimit,
                    "collection length exceeds limit",
                ));
            }
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    break;
                }
                _ => return self.fail("expected , or ]"),
            }
        }
        self.depth -= 1;
        Ok(CanonValue::Array(items))
    }
    fn parse_object(&mut self) -> CoreResult<CanonValue> {
        self.enter()?;
        self.pos += 1;
        let mut map = BTreeMap::new();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            self.depth -= 1;
            return Ok(CanonValue::Object(map));
        }
        let mut last_key: Option<String> = None;
        loop {
            if self.peek() != Some(b'"') {
                return self.fail("expected object key");
            }
            let key = self.parse_string()?;
            if !key.is_ascii() {
                return self.fail("object keys must be ASCII");
            }
            if let Some(prev) = &last_key {
                if key.as_bytes() <= prev.as_bytes() {
                    return self
                        .fail("object keys must be strictly sorted (duplicate or unsorted key)");
                }
            }
            if self.peek() != Some(b':') {
                return self.fail("expected :");
            }
            self.pos += 1;
            let v = self.parse_value()?;
            map.insert(key.clone(), v);
            last_key = Some(key);
            if map.len() > self.limits.max_collection_len {
                return Err(CoreError::new(
                    ErrorCode::ResourceLimit,
                    "object size exceeds limit",
                ));
            }
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    break;
                }
                _ => return self.fail("expected , or }"),
            }
        }
        self.depth -= 1;
        Ok(CanonValue::Object(map))
    }
    fn parse_string(&mut self) -> CoreResult<String> {
        // opening quote
        self.pos += 1;
        let start = self.pos;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let c = match self.peek() {
                None => return self.fail("unterminated string"),
                Some(c) => c,
            };
            if self.pos - start > self.limits.max_scalar_bytes + 8 {
                return Err(CoreError::new(
                    ErrorCode::ResourceLimit,
                    "string exceeds scalar limit",
                ));
            }
            match c {
                b'"' => {
                    self.pos += 1;
                    break;
                }
                b'\\' => {
                    self.pos += 1;
                    match self.peek() {
                        Some(b'"') => {
                            out.push(b'"');
                            self.pos += 1;
                        }
                        Some(b'\\') => {
                            out.push(b'\\');
                            self.pos += 1;
                        }
                        Some(b'u') => {
                            self.pos += 1;
                            let hex = self.src.get(self.pos..self.pos + 4).ok_or_else(|| {
                                CoreError::new(
                                    ErrorCode::NonCanonicalEncoding,
                                    "truncated \\u escape",
                                )
                            })?;
                            let h = std::str::from_utf8(hex).map_err(|_| {
                                CoreError::new(
                                    ErrorCode::NonCanonicalEncoding,
                                    "invalid \\u escape",
                                )
                            })?;
                            if !h
                                .bytes()
                                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                            {
                                return self.fail("\\u escape must be lowercase hex");
                            }
                            let cp = u32::from_str_radix(h, 16).map_err(|_| {
                                CoreError::new(
                                    ErrorCode::NonCanonicalEncoding,
                                    "invalid \\u escape",
                                )
                            })?;
                            if cp >= 0x20 {
                                return self.fail("only control characters may use \\u escapes");
                            }
                            out.push(cp as u8);
                            self.pos += 4;
                        }
                        _ => {
                            return self.fail("invalid escape (only \\\" \\\\ and \\u00xx allowed)")
                        }
                    }
                }
                c if c < 0x20 => return self.fail("raw control character in string"),
                _ => {
                    out.push(c);
                    self.pos += 1;
                }
            }
        }
        String::from_utf8(out)
            .map_err(|_| CoreError::new(ErrorCode::NonCanonicalEncoding, "invalid UTF-8 in string"))
    }
}

/// Builder for canonical objects.
pub struct ObjBuilder(BTreeMap<String, CanonValue>);

impl ObjBuilder {
    pub fn f(mut self, key: &str, v: CanonValue) -> Self {
        debug_assert!(key.is_ascii(), "canonical object keys must be ASCII");
        let prev = self.0.insert(key.to_string(), v);
        debug_assert!(prev.is_none(), "duplicate key {key}");
        self
    }
    pub fn fc<T: Canonical>(self, key: &str, v: &T) -> Self {
        self.f(key, v.to_canon())
    }
    pub fn fopt<T: Canonical>(self, key: &str, v: &Option<T>) -> Self {
        self.f(key, CanonValue::option(v))
    }
    pub fn fu64(self, key: &str, v: u64) -> Self {
        self.f(key, CanonValue::uint(v))
    }
    pub fn fu32(self, key: &str, v: u32) -> Self {
        self.f(key, CanonValue::u32(v))
    }
    pub fn fstr(self, key: &str, v: &str) -> Self {
        self.f(key, CanonValue::str(v))
    }
    pub fn fbool(self, key: &str, v: bool) -> Self {
        self.f(key, CanonValue::Bool(v))
    }
    pub fn fbytes(self, key: &str, v: &[u8]) -> Self {
        self.f(key, CanonValue::bytes(v))
    }
    pub fn fvec<T: Canonical>(self, key: &str, v: &[T]) -> Self {
        self.f(
            key,
            CanonValue::Array(v.iter().map(|x| x.to_canon()).collect()),
        )
    }
    /// Sorted set of canonical items; panics on duplicates in debug (callers guarantee uniqueness).
    pub fn fset<T: Canonical>(self, key: &str, v: &[T]) -> Self {
        let items: Vec<CanonValue> = v.iter().map(|x| x.to_canon()).collect();
        let set = CanonValue::set(items).expect("duplicate element in set field");
        self.f(key, set)
    }
    pub fn build(self) -> CanonValue {
        CanonValue::Object(self.0)
    }
}

/// Types with a canonical representation.
pub trait Canonical: Sized {
    fn to_canon(&self) -> CanonValue;
    fn from_canon(v: &CanonValue) -> CoreResult<Self>;

    fn encode(&self) -> Vec<u8> {
        self.to_canon().encode()
    }
    /// Strict typed decode (SPEC-003 §10, SPEC-012 §8): the canonical value is parsed, the typed
    /// value is re-projected to canonical form and must equal the input exactly. Unknown fields,
    /// tolerated variants or lossy projections are therefore refused at every decoder without
    /// per-type field lists.
    fn decode(bytes: &[u8], limits: &Limits) -> CoreResult<Self> {
        let v = CanonValue::decode(bytes, limits)?;
        let t = Self::from_canon(&v)?;
        if t.to_canon() != v {
            return Err(CoreError::new(
                ErrorCode::NonCanonicalEncoding,
                "decoded value does not re-encode to the input (unknown or non-canonical fields)",
            ));
        }
        Ok(t)
    }
}

impl Canonical for CanonValue {
    fn to_canon(&self) -> CanonValue {
        self.clone()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(v.clone())
    }
}

impl Canonical for String {
    fn to_canon(&self) -> CanonValue {
        CanonValue::Str(self.clone())
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(v.as_str()?.to_string())
    }
}

impl Canonical for bool {
    fn to_canon(&self) -> CanonValue {
        CanonValue::Bool(*self)
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.as_bool()
    }
}

impl Canonical for u64 {
    fn to_canon(&self) -> CanonValue {
        CanonValue::uint(*self)
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.as_u64()
    }
}

impl Canonical for u32 {
    fn to_canon(&self) -> CanonValue {
        CanonValue::u32(*self)
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.as_u32()
    }
}

impl Canonical for i64 {
    fn to_canon(&self) -> CanonValue {
        CanonValue::int(*self as i128)
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.as_i64()
    }
}

impl Canonical for Vec<u8> {
    fn to_canon(&self) -> CanonValue {
        CanonValue::bytes(self)
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.as_bytes()
    }
}

impl<T: Canonical> Canonical for Vec<T> {
    fn to_canon(&self) -> CanonValue {
        CanonValue::Array(self.iter().map(|x| x.to_canon()).collect())
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.as_array()?.iter().map(T::from_canon).collect()
    }
}

impl<T: Canonical> Canonical for Option<T> {
    fn to_canon(&self) -> CanonValue {
        CanonValue::option(self)
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        if v.is_null() {
            Ok(None)
        } else {
            Ok(Some(T::from_canon(v)?))
        }
    }
}

/// Decode a sorted set field into a `Vec<T>` after verifying canonical set order.
/// The returned vector is in the type's natural order so that structs holding
/// naturally sorted vectors roundtrip to equal values.
pub fn decode_set<T: Canonical + Ord>(v: &CanonValue) -> CoreResult<Vec<T>> {
    let mut out: Vec<T> = v
        .as_set()?
        .iter()
        .map(T::from_canon)
        .collect::<CoreResult<_>>()?;
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lim() -> Limits {
        Limits::v1()
    }

    #[test]
    fn encodes_sorted_and_compact() {
        let v = CanonValue::obj()
            .fstr("b", "x")
            .fu64("a", 17)
            .fbool("c", true)
            .f("d", CanonValue::Null)
            .build();
        assert_eq!(
            String::from_utf8(v.encode()).unwrap(),
            r#"{"a":"17","b":"x","c":true,"d":null}"#
        );
    }

    #[test]
    fn roundtrip_and_reject_noncanonical() {
        let v = CanonValue::obj()
            .fstr("k", "line\nbreak \"q\" \\")
            .fbytes("h", &[0, 255])
            .build();
        let bytes = v.encode();
        let expected = "{\"h\":\"00ff\",\"k\":\"line\\u000abreak \\\"q\\\" \\\\\"}";
        assert_eq!(String::from_utf8(bytes.clone()).unwrap(), expected);
        assert_eq!(CanonValue::decode(&bytes, &lim()).unwrap(), v);

        let uppercase_escape = "{\"a\":\"\\u000A\"}";
        let noncontrol_escape = "{\"a\":\"\\u0041\"}";
        let shorthand_escape = "{\"a\":\"\\n\"}";
        for bad in [
            r#"{"b":"1","a":"2"}"#, // unsorted
            r#"{"a":"1","a":"2"}"#, // duplicate
            r#"{"a": "1"}"#,        // whitespace
            r#"{"a":1}"#,           // JSON number
            shorthand_escape,       // shorthand escape
            uppercase_escape,       // uppercase hex escape
            noncontrol_escape,      // escape of non-control
            r#"{"a":"1"} "#,        // trailing
            "{\"a\":\"\u{1}\"}",    // raw control
        ] {
            assert!(
                CanonValue::decode(bad.as_bytes(), &lim()).is_err(),
                "should reject {bad:?}"
            );
        }
    }

    #[test]
    fn canonical_ints() {
        assert_eq!(parse_canonical_int("0").unwrap(), 0);
        assert_eq!(parse_canonical_int("-17").unwrap(), -17);
        for bad in ["+1", "01", "-0", "", "-", "1.0", "1e3", " 1"] {
            assert!(parse_canonical_int(bad).is_err(), "{bad}");
        }
        assert!(CanonValue::str("18446744073709551616").as_u64().is_err());
        assert_eq!(
            CanonValue::str("18446744073709551615").as_u64().unwrap(),
            u64::MAX
        );
    }

    #[test]
    fn sets_sort_by_bytes_and_reject_dupes() {
        let s = CanonValue::set(vec![CanonValue::str("b"), CanonValue::str("a")]).unwrap();
        assert_eq!(String::from_utf8(s.encode()).unwrap(), r#"["a","b"]"#);
        assert!(CanonValue::set(vec![CanonValue::str("a"), CanonValue::str("a")]).is_err());
        assert!(
            CanonValue::Array(vec![CanonValue::str("b"), CanonValue::str("a")])
                .as_set()
                .is_err()
        );
    }

    #[test]
    fn limits_enforced() {
        let deep = "[".repeat(10) + &"]".repeat(10);
        assert!(CanonValue::decode(deep.as_bytes(), &Limits::tiny()).is_err());
        let ok = "[".repeat(5) + &"]".repeat(5);
        assert!(CanonValue::decode(ok.as_bytes(), &Limits::tiny()).is_ok());
        let wide = format!("[{}]", vec!["\"x\""; 20].join(","));
        assert!(CanonValue::decode(wide.as_bytes(), &Limits::tiny()).is_err());
    }

    #[test]
    fn invalid_utf8_rejected() {
        let mut b = b"{\"a\":\"".to_vec();
        b.push(0xff);
        b.extend_from_slice(b"\"}");
        assert!(CanonValue::decode(&b, &lim()).is_err());
    }

    #[test]
    fn unknown_fields_rejected() {
        let v = CanonValue::obj().fstr("a", "1").fstr("z", "2").build();
        assert!(v.expect_fields(&["a"]).is_err());
        assert!(v.expect_fields(&["a", "z"]).is_ok());
        assert!(v.expect_fields(&["a", "z", "m"]).is_err());
    }
}
