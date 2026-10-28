//! Valve KeyValues text format, in the dialect Hammer uses for VMF files.
//!
//! A document is a list of nodes; each node is a key with either a string
//! value or a block of child nodes. Keys may repeat and order is preserved.
//! Strings are taken verbatim: VMF has no escape sequences, so `\n` inside an
//! entity `message` stays two characters.

use std::fmt;

pub const MAX_NESTING: usize = 256;

mod flat_blocks;
pub use flat_blocks::{
    FlatBlock, FlatBlockLimits, parse_flat_blocks_bounded, parse_flat_blocks_controlled,
};

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    String(String),
    Block(Vec<Node>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub key: String,
    pub value: Value,
}

impl Node {
    pub fn string(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: Value::String(value.into()),
        }
    }

    pub fn block(key: impl Into<String>, children: Vec<Node>) -> Self {
        Self {
            key: key.into(),
            value: Value::Block(children),
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match &self.value {
            Value::String(s) => Some(s),
            Value::Block(_) => None,
        }
    }

    pub fn children(&self) -> Option<&[Node]> {
        match &self.value {
            Value::String(_) => None,
            Value::Block(children) => Some(children),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseErrorKind {
    UnterminatedString,
    UnexpectedOpenBrace,
    UnexpectedCloseBrace,
    UnclosedBlock,
    MissingValue,
    NestingLimit,
    NodeLimit,
    NulByte,
    ByteLimit,
    BlockLimit,
    PairLimit,
    ExpectedOpenBrace,
    NestedBlock,
    UnsupportedBareToken,
    UnterminatedComment,
    ConditionalToken,
    EmbeddedCommentMarker,
}

impl fmt::Display for ParseErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnterminatedString => "unterminated quoted string",
            Self::UnexpectedOpenBrace => "'{' without a preceding key",
            Self::UnexpectedCloseBrace => "'}' without a matching '{'",
            Self::UnclosedBlock => "block opened here is never closed",
            Self::MissingValue => "key is missing a value or block",
            Self::NestingLimit => "block nesting exceeds the input limit",
            Self::NodeLimit => "node count exceeds the input limit",
            Self::NulByte => "NUL bytes are not accepted",
            Self::ByteLimit => "text exceeds the input byte limit",
            Self::BlockLimit => "block count exceeds the input limit",
            Self::PairLimit => "pair count exceeds the input limit",
            Self::ExpectedOpenBrace => "expected an anonymous block beginning with '{'",
            Self::NestedBlock => "nested blocks are not accepted in flat entity text",
            Self::UnsupportedBareToken => {
                "unquoted tokens cannot contain parentheses, apostrophes, non-ASCII bytes or control separators, or abut a double quote"
            }
            Self::UnterminatedComment => "unterminated block comment",
            Self::ConditionalToken => {
                "unquoted conditional selectors are unsupported in static material syntax"
            }
            Self::EmbeddedCommentMarker => {
                "comment markers inside unquoted tokens are unsupported in static material syntax"
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("line {line}, column {column}: {kind}")]
pub struct ParseError {
    pub line: usize,
    pub column: usize,
    pub kind: ParseErrorKind,
}

impl ParseError {
    fn at(src: &str, offset: usize, kind: ParseErrorKind) -> Self {
        let before = &src[..offset];
        let line = before.bytes().filter(|&b| b == b'\n').count() + 1;
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        let column = before[line_start..].chars().count() + 1;
        Self { line, column, kind }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token<'a> {
    Str(&'a str),
    Open,
    Close,
    Eof,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ParseMode {
    Strict,
    Lenient,
    StaticMaterial,
}

struct Lexer<'a> {
    src: &'a str,
    pos: usize,
    mode: ParseMode,
}

impl<'a> Lexer<'a> {
    fn next(&mut self) -> Result<(Token<'a>, usize), ParseError> {
        let bytes = self.src.as_bytes();
        loop {
            while self.pos < bytes.len() && bytes[self.pos].is_ascii_whitespace() {
                self.pos += 1;
            }
            let rest = &self.src[self.pos..];
            if rest.starts_with("//") {
                self.pos = rest.find('\n').map_or(bytes.len(), |i| self.pos + i + 1);
                continue;
            }
            if self.mode != ParseMode::Strict && rest.starts_with("/*") {
                let closing = if self.mode == ParseMode::StaticMaterial {
                    rest[2..].find("*/").map(|index| index + 2)
                } else {
                    rest.find("*/")
                };
                self.pos = match closing {
                    Some(index) => self.pos + index + 2,
                    None if self.mode == ParseMode::StaticMaterial => {
                        return Err(ParseError::at(
                            self.src,
                            self.pos,
                            ParseErrorKind::UnterminatedComment,
                        ));
                    }
                    None => bytes.len(),
                };
                continue;
            }
            break;
        }

        let start = self.pos;
        let Some(&b) = bytes.get(start) else {
            return Ok((Token::Eof, start));
        };
        let token = match b {
            b'{' => {
                self.pos += 1;
                Token::Open
            }
            b'}' => {
                self.pos += 1;
                Token::Close
            }
            b'"' => {
                let body = start + 1;
                let Some(len) = self.src[body..].find('"') else {
                    return Err(ParseError::at(
                        self.src,
                        start,
                        ParseErrorKind::UnterminatedString,
                    ));
                };
                self.pos = body + len + 1;
                Token::Str(&self.src[body..body + len])
            }
            _ => {
                let len = bytes[start..]
                    .iter()
                    .position(|&c| c.is_ascii_whitespace() || matches!(c, b'"' | b'{' | b'}'))
                    .unwrap_or(bytes.len() - start);
                self.pos = start + len;
                let token = &self.src[start..self.pos];
                if self.mode == ParseMode::StaticMaterial {
                    for (index, byte) in token.bytes().enumerate() {
                        let rest = &token.as_bytes()[index..];
                        let kind = if rest.starts_with(b"/*")
                            || rest.starts_with(b"*/")
                            || rest.starts_with(b"//")
                        {
                            ParseErrorKind::EmbeddedCommentMarker
                        } else if byte == b'[' {
                            ParseErrorKind::ConditionalToken
                        } else {
                            continue;
                        };
                        return Err(ParseError::at(self.src, start + index, kind));
                    }
                }
                Token::Str(token)
            }
        };
        Ok((token, start))
    }
}

/// Parses strictly: unclosed blocks are errors. Use for files we write back,
/// where silently accepting a truncated file would lose data on save.
pub fn parse(src: &str) -> Result<Vec<Node>, ParseError> {
    parse_impl(src, ParseMode::Strict, Some((MAX_NESTING, usize::MAX)))
}

/// Parses the way the engine reads game files: blocks left open at the end
/// of the file are closed, and `/* */` comments are skipped.
pub fn parse_lenient(src: &str) -> Result<Vec<Node>, ParseError> {
    parse_impl(src, ParseMode::Lenient, Some((MAX_NESTING, usize::MAX)))
}

pub fn parse_bounded(
    src: &str,
    max_depth: usize,
    max_nodes: usize,
) -> Result<Vec<Node>, ParseError> {
    if let Some(offset) = src.find('\0') {
        return Err(ParseError::at(src, offset, ParseErrorKind::NulByte));
    }
    parse_impl(
        src,
        ParseMode::Strict,
        Some((max_depth.min(MAX_NESTING), max_nodes)),
    )
}

pub fn parse_lenient_bounded(
    src: &str,
    max_depth: usize,
    max_nodes: usize,
) -> Result<Vec<Node>, ParseError> {
    if let Some(offset) = src.find('\0') {
        return Err(ParseError::at(src, offset, ParseErrorKind::NulByte));
    }
    parse_impl(
        src,
        ParseMode::Lenient,
        Some((max_depth.min(MAX_NESTING), max_nodes)),
    )
}

/// Parses complete material syntax with token-boundary block comments.
/// Unquoted selectors and embedded comment markers reject; quoted text is verbatim.
pub fn parse_static_material_bounded(
    src: &str,
    max_depth: usize,
    max_nodes: usize,
) -> Result<Vec<Node>, ParseError> {
    if let Some(offset) = src.find('\0') {
        return Err(ParseError::at(src, offset, ParseErrorKind::NulByte));
    }
    parse_impl(
        src,
        ParseMode::StaticMaterial,
        Some((max_depth.min(MAX_NESTING), max_nodes)),
    )
}

fn parse_impl(
    src: &str,
    mode: ParseMode,
    limits: Option<(usize, usize)>,
) -> Result<Vec<Node>, ParseError> {
    let mut count = 0_usize;
    let mut lexer = Lexer { src, pos: 0, mode };
    let mut current: Vec<Node> = Vec::new();
    // Enclosing blocks: (key, siblings collected so far, offset of the '{').
    let mut stack: Vec<(String, Vec<Node>, usize)> = Vec::new();

    loop {
        let (token, offset) = lexer.next()?;
        match token {
            Token::Eof => {
                if let Some(&(_, _, open)) = stack.last()
                    && mode != ParseMode::Lenient
                {
                    return Err(ParseError::at(src, open, ParseErrorKind::UnclosedBlock));
                }
                while let Some((key, siblings, _)) = stack.pop() {
                    let children = std::mem::replace(&mut current, siblings);
                    current.push(Node::block(key, children));
                }
                return Ok(current);
            }
            Token::Open => {
                return Err(ParseError::at(
                    src,
                    offset,
                    ParseErrorKind::UnexpectedOpenBrace,
                ));
            }
            Token::Close => {
                let Some((key, siblings, _)) = stack.pop() else {
                    return Err(ParseError::at(
                        src,
                        offset,
                        ParseErrorKind::UnexpectedCloseBrace,
                    ));
                };
                let children = std::mem::replace(&mut current, siblings);
                current.push(Node::block(key, children));
            }
            Token::Str(key) => {
                count = count.saturating_add(1);
                if limits.is_some_and(|(_, nodes)| count > nodes) {
                    return Err(ParseError::at(src, offset, ParseErrorKind::NodeLimit));
                }
                match lexer.next()? {
                    (Token::Str(value), _) => current.push(Node::string(key, value)),
                    (Token::Open, open) => {
                        if limits.is_some_and(|(depth, _)| stack.len() >= depth) {
                            return Err(ParseError::at(src, open, ParseErrorKind::NestingLimit));
                        }
                        stack.push((key.to_owned(), std::mem::take(&mut current), open));
                    }
                    (Token::Close | Token::Eof, _) => {
                        return Err(ParseError::at(src, offset, ParseErrorKind::MissingValue));
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    Lf,
    #[default]
    CrLf,
}

impl LineEnding {
    fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::CrLf => "\r\n",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WriteOptions {
    pub line_ending: LineEnding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WriteError {
    #[error("VMF strings cannot contain literal quotes")]
    Quote,
    #[error("KeyValues nesting exceeds {MAX_NESTING} levels")]
    NestingLimit,
}

pub fn validate_string(value: &str) -> Result<(), WriteError> {
    if value.contains('"') {
        return Err(WriteError::Quote);
    }
    Ok(())
}

pub fn validate_nodes(nodes: &[Node]) -> Result<(), WriteError> {
    let mut pending = vec![(nodes.iter(), 0)];
    while let Some((nodes, depth)) = pending.last_mut() {
        let Some(node) = nodes.next() else {
            pending.pop();
            continue;
        };
        validate_string(&node.key)?;
        match &node.value {
            Value::String(value) => validate_string(value)?,
            Value::Block(children) => {
                let depth = *depth + 1;
                if depth > MAX_NESTING {
                    return Err(WriteError::NestingLimit);
                }
                pending.push((children.iter(), depth));
            }
        }
    }
    Ok(())
}

/// Writes known-valid nodes. Use `try_write` for fallible publication.
/// Panics if strings contain literal quotes or nesting exceeds `MAX_NESTING`.
pub fn write(nodes: &[Node], options: &WriteOptions) -> String {
    try_write(nodes, options).expect("KeyValues must be representable")
}

pub fn try_write(nodes: &[Node], options: &WriteOptions) -> Result<String, WriteError> {
    validate_nodes(nodes)?;
    let mut out = String::new();
    write_nodes(&mut out, nodes, 0, options.line_ending.as_str());
    Ok(out)
}

fn write_nodes(out: &mut String, nodes: &[Node], depth: usize, newline: &str) {
    for node in nodes {
        indent(out, depth);
        match &node.value {
            Value::String(value) => {
                out.push('"');
                out.push_str(&node.key);
                out.push_str("\" \"");
                out.push_str(value);
                out.push('"');
                out.push_str(newline);
            }
            Value::Block(children) => {
                let quote = node.key.is_empty()
                    || node.key.starts_with("//")
                    || node.key.starts_with("/*")
                    || node
                        .key
                        .bytes()
                        .any(|byte| byte.is_ascii_whitespace() || matches!(byte, b'{' | b'}'));
                if quote {
                    out.push('"');
                }
                out.push_str(&node.key);
                if quote {
                    out.push('"');
                }
                out.push_str(newline);
                indent(out, depth);
                out.push('{');
                out.push_str(newline);
                write_nodes(out, children, depth + 1, newline);
                indent(out, depth);
                out.push('}');
                out.push_str(newline);
            }
        }
    }
}

fn indent(out: &mut String, depth: usize) {
    out.extend(std::iter::repeat_n('\t', depth));
}
