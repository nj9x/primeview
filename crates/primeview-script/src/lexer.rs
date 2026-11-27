//! Tokenizer. Produces a flat token list terminated by [`Tok::Eof`].
//!
//! Newlines terminate statements, except inside `( )` / `[ ]` where they are
//! dropped so long argument lists can span several lines.

use crate::Diagnostic;
use crate::limits::{MAX_STRING_LEN, MAX_TOKENS};

/// 1-based source position (column counted in characters).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Num(f64),
    Ident(String),
    Str(String),
    Color([u8; 4]),
    Let,
    And,
    Or,
    Not,
    True,
    False,
    Na,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    Assign,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Lt,
    Le,
    Gt,
    Ge,
    EqEq,
    Ne,
    Question,
    Colon,
    DotDot,
    Newline,
    Eof,
}

impl Tok {
    /// Human-readable description for diagnostics.
    pub fn describe(&self) -> String {
        let sym = match self {
            Tok::Num(n) => return format!("number `{n}`"),
            Tok::Ident(s) => return format!("`{s}`"),
            Tok::Str(_) => return "a string".into(),
            Tok::Color(_) => return "a color literal".into(),
            Tok::Newline => return "end of line".into(),
            Tok::Eof => return "end of script".into(),
            Tok::Let => "let",
            Tok::And => "and",
            Tok::Or => "or",
            Tok::Not => "not",
            Tok::True => "true",
            Tok::False => "false",
            Tok::Na => "na",
            Tok::LParen => "(",
            Tok::RParen => ")",
            Tok::LBracket => "[",
            Tok::RBracket => "]",
            Tok::Comma => ",",
            Tok::Assign => "=",
            Tok::Plus => "+",
            Tok::Minus => "-",
            Tok::Star => "*",
            Tok::Slash => "/",
            Tok::Percent => "%",
            Tok::Lt => "<",
            Tok::Le => "<=",
            Tok::Gt => ">",
            Tok::Ge => ">=",
            Tok::EqEq => "==",
            Tok::Ne => "!=",
            Tok::Question => "?",
            Tok::Colon => ":",
            Tok::DotDot => "..",
        };
        format!("`{sym}`")
    }
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub pos: Pos,
}

fn ch(chars: &[char], i: usize) -> char {
    chars.get(i).copied().unwrap_or('\0')
}

fn err(pos: Pos, msg: impl Into<String>) -> Diagnostic {
    Diagnostic::at(pos, msg)
}

struct Out {
    toks: Vec<Token>,
}

impl Out {
    fn push(&mut self, tok: Tok, pos: Pos) -> Result<(), Diagnostic> {
        if self.toks.len() >= MAX_TOKENS {
            return Err(err(
                pos,
                format!("script is too long (more than {MAX_TOKENS} tokens)"),
            ));
        }
        self.toks.push(Token { tok, pos });
        Ok(())
    }

    fn newline(&mut self, pos: Pos) -> Result<(), Diagnostic> {
        match self.toks.last() {
            None
            | Some(Token {
                tok: Tok::Newline, ..
            }) => Ok(()),
            _ => self.push(Tok::Newline, pos),
        }
    }
}

pub fn lex(src: &str) -> Result<Vec<Token>, Diagnostic> {
    let chars: Vec<char> = src.chars().collect();
    let len = chars.len();
    let mut out = Out { toks: Vec::new() };
    let (mut i, mut line, mut col) = (0usize, 1usize, 1usize);
    let mut depth = 0usize;

    while i < len {
        let c = chars[i];
        let pos = Pos { line, col };
        let next = ch(&chars, i + 1);
        // Single-line tokens: (token, width).
        let simple = match c {
            ' ' | '\t' | '\r' => {
                i += 1;
                col += 1;
                continue;
            }
            '\n' => {
                if depth == 0 {
                    out.newline(pos)?;
                }
                i += 1;
                line += 1;
                col = 1;
                continue;
            }
            ';' => {
                out.newline(pos)?;
                i += 1;
                col += 1;
                continue;
            }
            '/' if next == '/' => {
                while i < len && chars[i] != '\n' {
                    i += 1;
                    col += 1;
                }
                continue;
            }
            '0'..='9' => None,
            '.' if next.is_ascii_digit() => None,
            '.' if next == '.' => Some((Tok::DotDot, 2)),
            '"' => {
                let (s, width) = lex_string(&chars, i, pos)?;
                out.push(Tok::Str(s), pos)?;
                i += width;
                col += width;
                continue;
            }
            '#' => {
                let mut j = i + 1;
                while ch(&chars, j).is_ascii_alphanumeric() {
                    j += 1;
                }
                let hex: String = chars[i + 1..j].iter().collect();
                let color = parse_color(&hex).ok_or_else(|| {
                    err(
                        pos,
                        format!("invalid color `#{hex}`: expected #rrggbb or #rrggbbaa"),
                    )
                })?;
                out.push(Tok::Color(color), pos)?;
                col += j - i;
                i = j;
                continue;
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let mut j = i;
                while ch(&chars, j).is_ascii_alphanumeric() || ch(&chars, j) == '_' {
                    j += 1;
                }
                let word: String = chars[i..j].iter().collect();
                let tok = match word.as_str() {
                    "let" => Tok::Let,
                    "and" => Tok::And,
                    "or" => Tok::Or,
                    "not" => Tok::Not,
                    "true" => Tok::True,
                    "false" => Tok::False,
                    "na" | "nan" => Tok::Na,
                    _ => Tok::Ident(word),
                };
                out.push(tok, pos)?;
                col += j - i;
                i = j;
                continue;
            }
            '(' => {
                depth += 1;
                Some((Tok::LParen, 1))
            }
            '[' => {
                depth += 1;
                Some((Tok::LBracket, 1))
            }
            ')' => {
                depth = depth.saturating_sub(1);
                Some((Tok::RParen, 1))
            }
            ']' => {
                depth = depth.saturating_sub(1);
                Some((Tok::RBracket, 1))
            }
            ',' => Some((Tok::Comma, 1)),
            '+' => Some((Tok::Plus, 1)),
            '-' => Some((Tok::Minus, 1)),
            '*' => Some((Tok::Star, 1)),
            '/' => Some((Tok::Slash, 1)),
            '%' => Some((Tok::Percent, 1)),
            '?' => Some((Tok::Question, 1)),
            ':' => Some((Tok::Colon, 1)),
            '=' if next == '=' => Some((Tok::EqEq, 2)),
            '=' => Some((Tok::Assign, 1)),
            '!' if next == '=' => Some((Tok::Ne, 2)),
            '!' => return Err(err(pos, "unexpected `!`; use `not` for negation")),
            '<' if next == '=' => Some((Tok::Le, 2)),
            '<' => Some((Tok::Lt, 1)),
            '>' if next == '=' => Some((Tok::Ge, 2)),
            '>' => Some((Tok::Gt, 1)),
            '&' => return Err(err(pos, "unexpected `&`; use `and`")),
            '|' => return Err(err(pos, "unexpected `|`; use `or`")),
            '.' => return Err(err(pos, "unexpected `.`")),
            other => {
                return Err(err(pos, format!("unexpected character `{other}`")));
            }
        };
        match simple {
            Some((tok, width)) => {
                out.push(tok, pos)?;
                i += width;
                col += width;
            }
            None => {
                let start = i;
                let v = lex_number(&chars, &mut i, pos)?;
                out.push(Tok::Num(v), pos)?;
                col += i - start;
            }
        }
    }
    out.push(Tok::Eof, Pos { line, col })?;
    Ok(out.toks)
}

fn lex_number(chars: &[char], i: &mut usize, pos: Pos) -> Result<f64, Diagnostic> {
    let start = *i;
    let digits = |i: &mut usize| {
        while ch(chars, *i).is_ascii_digit() {
            *i += 1;
        }
    };
    digits(i);
    if ch(chars, *i) == '.' && ch(chars, *i + 1) != '.' {
        *i += 1;
        digits(i);
    }
    if matches!(ch(chars, *i), 'e' | 'E') {
        let j = if matches!(ch(chars, *i + 1), '+' | '-') {
            *i + 2
        } else {
            *i + 1
        };
        if ch(chars, j).is_ascii_digit() {
            *i = j;
            digits(i);
        }
    }
    let text: String = chars[start..*i].iter().collect();
    let tail = ch(chars, *i);
    if tail.is_ascii_alphanumeric() || tail == '_' || (tail == '.' && ch(chars, *i + 1) != '.') {
        return Err(err(
            pos,
            format!("invalid number literal starting `{text}`"),
        ));
    }
    text.parse::<f64>()
        .map_err(|_| err(pos, format!("invalid number literal `{text}`")))
}

fn lex_string(chars: &[char], start: usize, pos: Pos) -> Result<(String, usize), Diagnostic> {
    let mut s = String::new();
    let mut i = start + 1;
    loop {
        if i >= chars.len() || chars[i] == '\n' {
            return Err(err(pos, "unterminated string (missing closing `\"`)"));
        }
        match chars[i] {
            '"' => {
                i += 1;
                break;
            }
            '\\' => {
                let e = ch(chars, i + 1);
                s.push(match e {
                    'n' => '\n',
                    't' => '\t',
                    '"' => '"',
                    '\\' => '\\',
                    _ => {
                        let p = Pos {
                            line: pos.line,
                            col: pos.col + (i - start),
                        };
                        return Err(err(p, "unknown escape; use \\\" \\\\ \\n or \\t"));
                    }
                });
                i += 2;
            }
            c => {
                s.push(c);
                i += 1;
            }
        }
        if s.chars().count() > MAX_STRING_LEN {
            return Err(err(
                pos,
                format!("string is too long (limit {MAX_STRING_LEN} characters)"),
            ));
        }
    }
    Ok((s, i - start))
}

fn parse_color(hex: &str) -> Option<[u8; 4]> {
    if !(hex.len() == 6 || hex.len() == 8) || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |k: usize| u8::from_str_radix(&hex[k..k + 2], 16).ok();
    let a = if hex.len() == 8 { byte(6)? } else { 255 };
    Some([byte(0)?, byte(2)?, byte(4)?, a])
}
