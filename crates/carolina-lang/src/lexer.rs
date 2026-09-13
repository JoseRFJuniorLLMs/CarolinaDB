//! Lexer for the restricted DSL.

use crate::ast::Span;
use carolina_core::error::{CoreError, CoreResult, ErrorCode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tok {
    Ident(String),
    Int(i128),
    /// decimal literal text such as `10.50`
    Dec(String),
    Str(String),
    /// `UUID"..."` 32 hex chars
    Uuid([u8; 16]),
    /// `BYTES"..."`
    Bytes(Vec<u8>),
    LBrace,
    RBrace,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    Colon,
    DoubleColon,
    Dot,
    Arrow,
    Plus,
    Minus,
    Star,
    EqEq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Assign,
    AndAnd,
    OrOr,
    Bang,
    Hash,
    QuestionQuestion,
    Eof,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

fn err(line: u32, col: u32, msg: &str) -> CoreError {
    CoreError::new(ErrorCode::ParseError, format!("{msg} at {line}:{col}"))
}

/// Scan one token starting at `chars[i]` (not whitespace/comment). Returns the token and its length in chars.
fn scan_token(chars: &[char], i: usize, line: u32, col: u32) -> CoreResult<(Tok, usize)> {
    let c = chars[i];
    let two: String = chars[i..chars.len().min(i + 2)].iter().collect();
    let two_tok = match two.as_str() {
        "::" => Some(Tok::DoubleColon),
        "->" => Some(Tok::Arrow),
        "==" => Some(Tok::EqEq),
        "!=" => Some(Tok::Ne),
        "<=" => Some(Tok::Le),
        ">=" => Some(Tok::Ge),
        "&&" => Some(Tok::AndAnd),
        "||" => Some(Tok::OrOr),
        "??" => Some(Tok::QuestionQuestion),
        _ => None,
    };
    if let Some(t) = two_tok {
        return Ok((t, 2));
    }
    let single = match c {
        '{' => Some(Tok::LBrace),
        '}' => Some(Tok::RBrace),
        '(' => Some(Tok::LParen),
        ')' => Some(Tok::RParen),
        '[' => Some(Tok::LBracket),
        ']' => Some(Tok::RBracket),
        ',' => Some(Tok::Comma),
        ':' => Some(Tok::Colon),
        '.' => Some(Tok::Dot),
        '+' => Some(Tok::Plus),
        '-' => Some(Tok::Minus),
        '*' => Some(Tok::Star),
        '<' => Some(Tok::Lt),
        '>' => Some(Tok::Gt),
        '=' => Some(Tok::Assign),
        '!' => Some(Tok::Bang),
        '#' => Some(Tok::Hash),
        _ => None,
    };
    if let Some(t) = single {
        return Ok((t, 1));
    }
    if c == '"' {
        let mut j = i + 1;
        let mut s = String::new();
        loop {
            let ch = *chars
                .get(j)
                .ok_or_else(|| err(line, col, "unterminated string"))?;
            match ch {
                '"' => break,
                '\\' => {
                    j += 1;
                    match chars.get(j) {
                        Some('"') => s.push('"'),
                        Some('\\') => s.push('\\'),
                        Some('n') => s.push('\n'),
                        _ => return Err(err(line, col, "invalid string escape")),
                    }
                }
                '\n' => return Err(err(line, col, "newline in string literal")),
                ch => s.push(ch),
            }
            j += 1;
        }
        return Ok((Tok::Str(s), j + 1 - i));
    }
    if c.is_ascii_digit() {
        let mut j = i;
        while j < chars.len() && chars[j].is_ascii_digit() {
            j += 1;
        }
        if j < chars.len()
            && chars[j] == '.'
            && chars.get(j + 1).is_some_and(|d| d.is_ascii_digit())
        {
            j += 1;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            let text: String = chars[i..j].iter().collect();
            return Ok((Tok::Dec(text), j - i));
        }
        let text: String = chars[i..j].iter().collect();
        if text.len() > 1 && text.starts_with('0') {
            return Err(err(line, col, "redundant leading zero in integer literal"));
        }
        let v: i128 = text
            .parse()
            .map_err(|_| err(line, col, "integer literal too large"))?;
        return Ok((Tok::Int(v), j - i));
    }
    if c.is_ascii_alphabetic() || c == '_' {
        let mut j = i;
        while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
            j += 1;
        }
        let word: String = chars[i..j].iter().collect();
        if (word == "UUID" || word == "BYTES") && chars.get(j) == Some(&'"') {
            let mut k = j + 1;
            let mut hex = String::new();
            while k < chars.len() && chars[k] != '"' {
                hex.push(chars[k]);
                k += 1;
            }
            if k >= chars.len() {
                return Err(err(line, col, "unterminated typed literal"));
            }
            let bytes =
                carolina_core::hash::hex_decode(&hex).map_err(|e| err(line, col, &e.message))?;
            let len = k + 1 - i;
            if word == "UUID" {
                if bytes.len() != 16 {
                    return Err(err(line, col, "UUID literal must be 32 hex chars"));
                }
                let mut a = [0u8; 16];
                a.copy_from_slice(&bytes);
                return Ok((Tok::Uuid(a), len));
            }
            return Ok((Tok::Bytes(bytes), len));
        }
        return Ok((Tok::Ident(word), j - i));
    }
    Err(err(line, col, &format!("unexpected character `{c}`")))
}

pub fn lex(src: &str) -> CoreResult<Vec<Token>> {
    let mut out = Vec::new();
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut line = 1u32;
    let mut col = 1u32;
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            i += 1;
            line += 1;
            col = 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            col += 1;
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        let span = Span { line, col };
        let (tok, len) = scan_token(&chars, i, line, col)?;
        out.push(Token { tok, span });
        i += len;
        col += len as u32;
    }
    out.push(Token {
        tok: Tok::Eof,
        span: Span { line, col },
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexes_basic() {
        let t = lex("RECORD P { id: Uuid PRIMARY KEY, stock: I64 } // c\n x >= 10.50 UUID\"00000000000000000000000000000001\"").unwrap();
        assert!(matches!(t[0].tok, Tok::Ident(ref s) if s == "RECORD"));
        assert!(t.iter().any(|x| matches!(x.tok, Tok::Ge)));
        assert!(t
            .iter()
            .any(|x| matches!(x.tok, Tok::Dec(ref d) if d == "10.50")));
        assert!(t.iter().any(|x| matches!(x.tok, Tok::Uuid(_))));
        assert!(lex("007").is_err());
        assert!(lex("\"abc").is_err());
    }
}
