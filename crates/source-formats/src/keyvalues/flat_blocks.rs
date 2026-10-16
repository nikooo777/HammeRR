use std::io;

use super::{Lexer, ParseError, ParseErrorKind, Token};

/// An anonymous block's string pairs, preserving source order and duplicate keys.
pub type FlatBlock = Vec<(String, String)>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlatBlockLimits {
    pub max_bytes: usize,
    pub max_blocks: usize,
    /// Total pairs across every block, including duplicate keys.
    pub max_pairs: usize,
}

/// Parses complete anonymous flat blocks using the existing KeyValues dialect.
/// Empty or comment-only text yields no blocks; malformed text returns an error.
pub fn parse_flat_blocks_bounded(
    src: &str,
    limits: FlatBlockLimits,
) -> Result<Vec<FlatBlock>, ParseError> {
    parse_impl(src, limits, &mut || Ok(()), std::convert::identity)
}

/// Checks cancellation before validation and each token, retaining callback errors.
/// Syntax and limit failures use `InvalidData` and retain the original `ParseError`.
/// One lexer step may scan up to `max_bytes` before the next checkpoint.
pub fn parse_flat_blocks_controlled(
    src: &str,
    limits: FlatBlockLimits,
    check: &mut dyn FnMut() -> io::Result<()>,
) -> io::Result<Vec<FlatBlock>> {
    parse_impl(src, limits, check, |error| {
        io::Error::new(io::ErrorKind::InvalidData, error)
    })
}

fn parse_impl<E>(
    src: &str,
    limits: FlatBlockLimits,
    check: &mut dyn FnMut() -> Result<(), E>,
    syntax: impl Fn(ParseError) -> E,
) -> Result<Vec<FlatBlock>, E> {
    let fail = |offset, kind| syntax(ParseError::at(src, offset, kind));
    check()?;
    if src.len() > limits.max_bytes {
        return Err(fail(0, ParseErrorKind::ByteLimit));
    }
    if let Some(offset) = src.find('\0') {
        return Err(fail(offset, ParseErrorKind::NulByte));
    }
    let mut lexer = Lexer {
        src,
        pos: 0,
        lenient: false,
    };
    let mut blocks = Vec::new();
    let mut pairs = 0;
    loop {
        check()?;
        let (token, open) = lexer.next().map_err(&syntax)?;
        match token {
            Token::Eof => return Ok(blocks),
            Token::Open => {}
            Token::Close => return Err(fail(open, ParseErrorKind::UnexpectedCloseBrace)),
            Token::Str(_) => return Err(fail(open, ParseErrorKind::ExpectedOpenBrace)),
        }
        if blocks.len() >= limits.max_blocks {
            return Err(fail(open, ParseErrorKind::BlockLimit));
        }
        let mut block = Vec::new();
        loop {
            check()?;
            let (token, offset) = lexer.next().map_err(&syntax)?;
            let key = match token {
                Token::Str(key) => key,
                Token::Close => break,
                Token::Open => return Err(fail(offset, ParseErrorKind::NestedBlock)),
                Token::Eof => return Err(fail(open, ParseErrorKind::UnclosedBlock)),
            };
            if pairs >= limits.max_pairs {
                return Err(fail(offset, ParseErrorKind::PairLimit));
            }
            check()?;
            let (token, value_offset) = lexer.next().map_err(&syntax)?;
            let value = match token {
                Token::Str(value) => value,
                Token::Open => return Err(fail(value_offset, ParseErrorKind::NestedBlock)),
                Token::Close | Token::Eof => {
                    return Err(fail(offset, ParseErrorKind::MissingValue));
                }
            };
            block.push((key.to_owned(), value.to_owned()));
            pairs += 1;
        }
        blocks.push(block);
    }
}

#[cfg(test)]
mod tests;
