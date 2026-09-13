//! Order-preserving physical key codec (SPEC-002 §11, key_codec_version 1).
//!
//! The semantic layer produces canonical bytes; the B+Tree compares them
//! lexicographically. Encodings are self-delimiting so tuples are concatenations:
//!
//! | value | encoding |
//! |---|---|
//! | tag byte | one byte identifying the element kind (keeps distinct kinds ordered and self-describing) |
//! | `u64` | `0x10` + 8 bytes big-endian |
//! | `i64` | `0x11` + 8 bytes big-endian of `x ^ i64::MIN` (sign flip keeps order) |
//! | `bool` | `0x12` + `0x00`/`0x01` |
//! | 16-byte id (UUID etc.) | `0x13` + 16 raw bytes |
//! | bytes / UTF-8 string | `0x14` + escaped bytes (`0x00` → `0x00 0xff`) + terminator `0x00 0x00` |
//! | decimal | `0x15` + 16 bytes big-endian of `coefficient ^ i128::MIN` (fixed scale is part of the type) |
//! | enum variant | `0x16` + 4 bytes big-endian variant index |
//! | option | `0x17` + `0x00` (None) or `0x01` + inner |
//! | tuple | `0x18` + 2-byte big-endian arity + elements |
//!
//! The namespace prefix and record id are written by callers with [`KeyWriter::namespace`]
//! and [`KeyWriter::u64`]. Decoding validates every byte; malformed input is a typed error.

use crate::error::{CoreError, CoreResult, ErrorCode};

pub const KEY_CODEC_VERSION: u32 = 1;

/// Logical namespaces (SPEC-002 §10). Reserved namespaces are not user-writable.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
#[repr(u8)]
pub enum Namespace {
    User = 0x01,
    Catalog = 0x02,
    IdcMeta = 0x03,
    Escrow = 0x04,
    TxnStatus = 0x05,
    Plan = 0x06,
    Replication = 0x07,
    System = 0x08,
    Index = 0x09,
    RequestBinding = 0x0a,
    Fact = 0x0b,
}

impl Namespace {
    pub fn from_byte(b: u8) -> CoreResult<Namespace> {
        Ok(match b {
            0x01 => Namespace::User,
            0x02 => Namespace::Catalog,
            0x03 => Namespace::IdcMeta,
            0x04 => Namespace::Escrow,
            0x05 => Namespace::TxnStatus,
            0x06 => Namespace::Plan,
            0x07 => Namespace::Replication,
            0x08 => Namespace::System,
            0x09 => Namespace::Index,
            0x0a => Namespace::RequestBinding,
            0x0b => Namespace::Fact,
            _ => {
                return Err(CoreError::new(
                    ErrorCode::Corruption,
                    format!("unknown namespace byte {b:#x}"),
                ))
            }
        })
    }
    pub fn is_user_writable(&self) -> bool {
        matches!(self, Namespace::User | Namespace::Index | Namespace::Fact)
    }
}

const T_U64: u8 = 0x10;
const T_I64: u8 = 0x11;
const T_BOOL: u8 = 0x12;
const T_ID16: u8 = 0x13;
const T_BYTES: u8 = 0x14;
const T_DEC: u8 = 0x15;
const T_ENUM: u8 = 0x16;
const T_OPT: u8 = 0x17;
const T_TUPLE: u8 = 0x18;

#[derive(Default, Debug, Clone)]
pub struct KeyWriter {
    buf: Vec<u8>,
}

impl KeyWriter {
    pub fn new() -> Self {
        KeyWriter {
            buf: Vec::with_capacity(64),
        }
    }
    pub fn namespace(mut self, ns: Namespace) -> Self {
        self.buf.push(ns as u8);
        self
    }
    pub fn u64(mut self, v: u64) -> Self {
        self.buf.push(T_U64);
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn i64(mut self, v: i64) -> Self {
        self.buf.push(T_I64);
        self.buf
            .extend_from_slice(&((v as u64) ^ (1u64 << 63)).to_be_bytes());
        self
    }
    pub fn bool(mut self, v: bool) -> Self {
        self.buf.push(T_BOOL);
        self.buf.push(v as u8);
        self
    }
    pub fn id16(mut self, v: &[u8; 16]) -> Self {
        self.buf.push(T_ID16);
        self.buf.extend_from_slice(v);
        self
    }
    pub fn bytes(mut self, v: &[u8]) -> Self {
        self.buf.push(T_BYTES);
        for &b in v {
            self.buf.push(b);
            if b == 0 {
                self.buf.push(0xff);
            }
        }
        self.buf.push(0);
        self.buf.push(0);
        self
    }
    pub fn str(self, v: &str) -> Self {
        self.bytes(v.as_bytes())
    }
    pub fn decimal(mut self, coefficient: i128) -> Self {
        self.buf.push(T_DEC);
        self.buf
            .extend_from_slice(&((coefficient as u128) ^ (1u128 << 127)).to_be_bytes());
        self
    }
    pub fn enum_variant(mut self, idx: u32) -> Self {
        self.buf.push(T_ENUM);
        self.buf.extend_from_slice(&idx.to_be_bytes());
        self
    }
    pub fn none(mut self) -> Self {
        self.buf.push(T_OPT);
        self.buf.push(0);
        self
    }
    pub fn some_marker(mut self) -> Self {
        self.buf.push(T_OPT);
        self.buf.push(1);
        self
    }
    pub fn tuple_header(mut self, arity: u16) -> Self {
        self.buf.push(T_TUPLE);
        self.buf.extend_from_slice(&arity.to_be_bytes());
        self
    }
    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
    pub fn as_slice(&self) -> &[u8] {
        &self.buf
    }
}

/// Decoded key element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyElem {
    U64(u64),
    I64(i64),
    Bool(bool),
    Id16([u8; 16]),
    Bytes(Vec<u8>),
    Decimal(i128),
    Enum(u32),
    None,
    Some,
    Tuple(u16),
}

pub struct KeyReader<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> KeyReader<'a> {
    pub fn new(src: &'a [u8]) -> Self {
        KeyReader { src, pos: 0 }
    }
    pub fn remaining(&self) -> usize {
        self.src.len() - self.pos
    }
    fn take(&mut self, n: usize) -> CoreResult<&'a [u8]> {
        if self.pos + n > self.src.len() {
            return Err(CoreError::new(ErrorCode::Corruption, "truncated key"));
        }
        let s = &self.src[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    pub fn namespace(&mut self) -> CoreResult<Namespace> {
        Namespace::from_byte(self.take(1)?[0])
    }
    pub fn elem(&mut self) -> CoreResult<KeyElem> {
        let tag = self.take(1)?[0];
        Ok(match tag {
            T_U64 => KeyElem::U64(u64::from_be_bytes(self.take(8)?.try_into().unwrap())),
            T_I64 => KeyElem::I64(
                (u64::from_be_bytes(self.take(8)?.try_into().unwrap()) ^ (1u64 << 63)) as i64,
            ),
            T_BOOL => match self.take(1)?[0] {
                0 => KeyElem::Bool(false),
                1 => KeyElem::Bool(true),
                _ => return Err(CoreError::new(ErrorCode::Corruption, "bad bool byte")),
            },
            T_ID16 => KeyElem::Id16(self.take(16)?.try_into().unwrap()),
            T_BYTES => {
                let mut out = Vec::new();
                loop {
                    let b = self.take(1)?[0];
                    if b == 0 {
                        let n = self.take(1)?[0];
                        match n {
                            0 => break,
                            0xff => out.push(0),
                            _ => {
                                return Err(CoreError::new(
                                    ErrorCode::Corruption,
                                    "bad escape in key bytes",
                                ))
                            }
                        }
                    } else {
                        out.push(b);
                    }
                }
                KeyElem::Bytes(out)
            }
            T_DEC => KeyElem::Decimal(
                (u128::from_be_bytes(self.take(16)?.try_into().unwrap()) ^ (1u128 << 127)) as i128,
            ),
            T_ENUM => KeyElem::Enum(u32::from_be_bytes(self.take(4)?.try_into().unwrap())),
            T_OPT => match self.take(1)?[0] {
                0 => KeyElem::None,
                1 => KeyElem::Some,
                _ => return Err(CoreError::new(ErrorCode::Corruption, "bad option byte")),
            },
            T_TUPLE => KeyElem::Tuple(u16::from_be_bytes(self.take(2)?.try_into().unwrap())),
            _ => {
                return Err(CoreError::new(
                    ErrorCode::Corruption,
                    format!("unknown key tag {tag:#x}"),
                ))
            }
        })
    }
    pub fn finish(&self) -> CoreResult<()> {
        if self.pos != self.src.len() {
            return Err(CoreError::new(
                ErrorCode::Corruption,
                "trailing bytes in key",
            ));
        }
        Ok(())
    }
}

/// Exclusive upper bound for a prefix scan: the smallest byte string greater than every key with this prefix.
/// Returns `None` if the prefix is all `0xff` (no upper bound).
pub fn prefix_upper_bound(prefix: &[u8]) -> Option<Vec<u8>> {
    let mut out = prefix.to_vec();
    while let Some(last) = out.pop() {
        if last < 0xff {
            out.push(last + 1);
            return Some(out);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::DetRng;

    #[test]
    fn i64_order_preserved() {
        let vals = [i64::MIN, -5, -1, 0, 1, 7, i64::MAX];
        let enc: Vec<Vec<u8>> = vals
            .iter()
            .map(|v| KeyWriter::new().i64(*v).finish())
            .collect();
        for w in enc.windows(2) {
            assert!(w[0] < w[1]);
        }
        for (v, e) in vals.iter().zip(enc.iter()) {
            let mut r = KeyReader::new(e);
            assert_eq!(r.elem().unwrap(), KeyElem::I64(*v));
            r.finish().unwrap();
        }
    }

    #[test]
    fn bytes_with_zero_order_and_roundtrip() {
        let a = KeyWriter::new().bytes(b"a").finish();
        let a0 = KeyWriter::new().bytes(b"a\0").finish();
        let ab = KeyWriter::new().bytes(b"ab").finish();
        assert!(a < a0 && a0 < ab);
        let mut r = KeyReader::new(&a0);
        assert_eq!(r.elem().unwrap(), KeyElem::Bytes(b"a\0".to_vec()));
        // tuple of (bytes, u64) keeps first-element order
        let t1 = KeyWriter::new().tuple_header(2).bytes(b"a").u64(9).finish();
        let t2 = KeyWriter::new()
            .tuple_header(2)
            .bytes(b"ab")
            .u64(0)
            .finish();
        assert!(t1 < t2);
    }

    #[test]
    fn decimal_order_preserved_random() {
        let mut rng = DetRng::new(7);
        let mut vals: Vec<i128> = (0..200)
            .map(|_| rng.next_u64() as i128 * if rng.chance(1, 2) { -1 } else { 1 })
            .collect();
        vals.push(i128::MIN);
        vals.push(i128::MAX);
        vals.push(0);
        vals.sort();
        let enc: Vec<Vec<u8>> = vals
            .iter()
            .map(|v| KeyWriter::new().decimal(*v).finish())
            .collect();
        for w in enc.windows(2) {
            assert!(w[0] <= w[1]);
        }
    }

    #[test]
    fn malformed_rejected() {
        assert!(KeyReader::new(&[T_U64, 1, 2]).elem().is_err());
        assert!(KeyReader::new(&[T_BYTES, 0, 7]).elem().is_err());
        assert!(KeyReader::new(&[0x99]).elem().is_err());
        assert!(KeyReader::new(&[0x42]).namespace().is_err());
    }

    #[test]
    fn upper_bound() {
        assert_eq!(prefix_upper_bound(&[1, 2, 3]).unwrap(), vec![1, 2, 4]);
        assert_eq!(prefix_upper_bound(&[1, 0xff]).unwrap(), vec![2]);
        assert_eq!(prefix_upper_bound(&[0xff, 0xff]), None);
    }
}
