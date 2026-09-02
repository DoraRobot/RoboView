//! Byte-text helpers shared by the ASCII parsers (PLY and PCD ASCII bodies,
//! OBJ, XYZ/CSV, plan §3.5): physical line splitting (CRLF-tolerant, the
//! final line may lack a newline), whitespace or `[,\t ]` tokenization, and
//! numeric token parsing (decimal and scientific notation; the textual
//! `nan`/`inf`/`infinity` spellings parse for float types so spec G1 values
//! survive a round trip).
//!
//! Everything operates on `&[u8]`. Each parser owns its error type and
//! message text, so the helpers are error-free except for the structural
//! [`NumberTokenError`], which callers map onto their family-specific
//! messages (1-based physical line numbers are derived by enumerating a
//! [`LineIter`]).

use std::str::FromStr;

/// Iterator over the physical lines of a byte slice, without terminators.
///
/// A trailing `\r` is removed so CRLF files behave like LF files (plan §5);
/// the final line needs no newline. Empty lines (blank lines between
/// records) yield empty slices; iteration stops once the file's final
/// newline has been consumed. This is the historical private iterator of
/// the PLY and PCD parsers, shared so every ASCII parser splits and numbers
/// lines identically.
pub(crate) struct LineIter<'a> {
    rest: &'a [u8],
}

impl<'a> LineIter<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { rest: bytes }
    }
}

impl<'a> Iterator for LineIter<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        if self.rest.is_empty() {
            return None;
        }
        let (line, rest) = match self.rest.iter().position(|&b| b == b'\n') {
            Some(offset) => (&self.rest[..offset], &self.rest[offset + 1..]),
            None => (self.rest, &[][..]),
        };
        self.rest = rest;
        Some(if line.last() == Some(&b'\r') {
            &line[..line.len() - 1]
        } else {
            line
        })
    }
}

/// Split a line on runs of ASCII whitespace (the PLY/PCD/OBJ convention).
/// Empty tokens are dropped, so no token in the result is empty.
pub(crate) fn ws_tokens(line: &[u8]) -> Vec<&[u8]> {
    line.split(|byte| byte.is_ascii_whitespace())
        .filter(|token| !token.is_empty())
        .collect()
}

/// Split a line on runs of space, tab and comma — the XYZ/CSV delimiter
/// class `[,\t ]+` (spec §7 F2). Unlike [`ws_tokens`], other ASCII
/// whitespace bytes (form feed, vertical tab) are not delimiters; they end
/// up inside tokens, where the number parser rejects them with a
/// line-numbered error.
pub(crate) fn xyz_tokens(line: &[u8]) -> Vec<&[u8]> {
    line.split(|byte| matches!(byte, b' ' | b'\t' | b','))
        .filter(|token| !token.is_empty())
        .collect()
}

/// Why a numeric token failed to parse. Kept structural so each parser
/// formats its own message text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NumberTokenError {
    /// The token is not valid UTF-8.
    NotUtf8,
    /// The token is valid UTF-8 but outside the target type's grammar.
    Invalid,
}

/// Parse one token as `T` through the type's own grammar. Float targets
/// (`f32`, `f64`) accept decimal and scientific notation plus the textual
/// non-finite spellings `nan`/`inf`/`infinity` (with an optional sign),
/// which keeps spec G1 values intact; integer targets get exact range
/// checking from their `FromStr` implementation.
pub(crate) fn parse_number<T>(token: &[u8]) -> Result<T, NumberTokenError>
where
    T: FromStr,
{
    let text = std::str::from_utf8(token).map_err(|_| NumberTokenError::NotUtf8)?;
    text.parse::<T>().map_err(|_| NumberTokenError::Invalid)
}
