//! Valve KeyValues text format, in the dialect Hammer uses for VMF files.
//!
//! A document is a list of nodes; each node is a key with either a string
//! value or a block of child nodes. Keys may repeat and order is preserved.
//! Strings are taken verbatim: VMF has no escape sequences, so `\n` inside an
//! entity `message` stays two characters.

use std::fmt;

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

struct Lexer<'a> {
    src: &'a str,
    pos: usize,
    lenient: bool,
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
            if self.lenient && rest.starts_with("/*") {
                self.pos = rest.find("*/").map_or(bytes.len(), |i| self.pos + i + 2);
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
                Token::Str(&self.src[start..start + len])
            }
        };
        Ok((token, start))
    }
}

/// Parses strictly: unclosed blocks are errors. Use for files we write back,
/// where silently accepting a truncated file would lose data on save.
pub fn parse(src: &str) -> Result<Vec<Node>, ParseError> {
    parse_impl(src, false, None)
}

/// Parses the way the engine reads game files: blocks left open at the end
/// of the file are closed, and `/* */` comments are skipped.
pub fn parse_lenient(src: &str) -> Result<Vec<Node>, ParseError> {
    parse_impl(src, true, None)
}

pub fn parse_bounded(
    src: &str,
    max_depth: usize,
    max_nodes: usize,
) -> Result<Vec<Node>, ParseError> {
    if let Some(offset) = src.find('\0') {
        return Err(ParseError::at(src, offset, ParseErrorKind::NulByte));
    }
    parse_impl(src, false, Some((max_depth, max_nodes)))
}

pub fn parse_lenient_bounded(
    src: &str,
    max_depth: usize,
    max_nodes: usize,
) -> Result<Vec<Node>, ParseError> {
    if let Some(offset) = src.find('\0') {
        return Err(ParseError::at(src, offset, ParseErrorKind::NulByte));
    }
    parse_impl(src, true, Some((max_depth, max_nodes)))
}

fn parse_impl(
    src: &str,
    lenient: bool,
    limits: Option<(usize, usize)>,
) -> Result<Vec<Node>, ParseError> {
    let mut count = 0_usize;
    let mut lexer = Lexer {
        src,
        pos: 0,
        lenient,
    };
    let mut current: Vec<Node> = Vec::new();
    // Enclosing blocks: (key, siblings collected so far, offset of the '{').
    let mut stack: Vec<(String, Vec<Node>, usize)> = Vec::new();

    loop {
        let (token, offset) = lexer.next()?;
        match token {
            Token::Eof => {
                if let Some(&(_, _, open)) = stack.last()
                    && !lenient
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

/// Writes nodes in Hammer's layout: tab indentation, bare block names, quoted
/// key/value pairs. The format cannot represent a `"` inside a string, so
/// callers must keep quotes out of keys and values.
pub fn write(nodes: &[Node], options: &WriteOptions) -> String {
    let mut out = String::new();
    write_nodes(&mut out, nodes, 0, options.line_ending.as_str());
    out
}

fn write_nodes(out: &mut String, nodes: &[Node], depth: usize, newline: &str) {
    for node in nodes {
        debug_assert!(
            !node.key.contains('"'),
            "key contains a quote: {:?}",
            node.key
        );
        indent(out, depth);
        match &node.value {
            Value::String(value) => {
                debug_assert!(!value.contains('"'), "value contains a quote: {value:?}");
                out.push('"');
                out.push_str(&node.key);
                out.push_str("\" \"");
                out.push_str(value);
                out.push('"');
                out.push_str(newline);
            }
            Value::Block(children) => {
                out.push_str(&node.key);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_blocks_and_repeated_keys() {
        let nodes =
            parse("world\n{\n\t\"id\" \"1\"\n\tsolid\n\t{\n\t}\n\tsolid { \"id\" \"2\" }\n}\n")
                .unwrap();
        assert_eq!(
            nodes,
            vec![Node::block(
                "world",
                vec![
                    Node::string("id", "1"),
                    Node::block("solid", vec![]),
                    Node::block("solid", vec![Node::string("id", "2")]),
                ],
            )]
        );
    }

    #[test]
    fn keeps_backslashes_and_skips_comments() {
        let nodes = parse("// header\n\"message\" \"a\\nb\" // trailing\nbare value").unwrap();
        assert_eq!(
            nodes,
            vec![
                Node::string("message", "a\\nb"),
                Node::string("bare", "value")
            ]
        );
    }

    #[test]
    fn reports_error_positions() {
        let err = parse("a\n{\n\t\"b\" \"c\n").unwrap_err();
        assert_eq!(
            (err.line, err.column, err.kind),
            (3, 6, ParseErrorKind::UnterminatedString)
        );

        let err = parse("a\n{\n\t\"b\" \"c\"\n").unwrap_err();
        assert_eq!(
            (err.line, err.column, err.kind),
            (2, 1, ParseErrorKind::UnclosedBlock)
        );

        let err = parse("}").unwrap_err();
        assert_eq!(err.kind, ParseErrorKind::UnexpectedCloseBrace);

        let err = parse("a { b }").unwrap_err();
        assert_eq!(err.kind, ParseErrorKind::MissingValue);
    }

    #[test]
    fn lenient_mode_accepts_hand_edited_game_files() {
        let src = "Shader\n{\n\t\"$a\" 1\n\t/* \"Proxies\" { } */\n\tProxies\n\t{\n\t\tx 1\n";
        assert!(parse(src).is_err());
        assert_eq!(
            parse_lenient(src).unwrap(),
            vec![Node::block(
                "Shader",
                vec![
                    Node::string("$a", "1"),
                    Node::block("Proxies", vec![Node::string("x", "1")]),
                ],
            )]
        );
    }

    #[test]
    fn writes_hammer_layout() {
        let nodes = vec![Node::block(
            "cordon",
            vec![Node::string("active", "0"), Node::block("inner", vec![])],
        )];
        let lf = write(
            &nodes,
            &WriteOptions {
                line_ending: LineEnding::Lf,
            },
        );
        assert_eq!(lf, "cordon\n{\n\t\"active\" \"0\"\n\tinner\n\t{\n\t}\n}\n");
        assert_eq!(parse(&lf).unwrap(), nodes);
        let crlf = write(&nodes, &WriteOptions::default());
        assert_eq!(crlf, lf.replace('\n', "\r\n"));
    }
}

#[cfg(test)]
mod bounded_tests {
    use super::*;
    #[test]
    fn bounded_lenient_input_preserves_engine_syntax_and_enforces_limits() {
        let source = "Shader { /* hidden { ignored value } */ $a 1 Proxies { x 1";
        assert_eq!(
            parse_lenient_bounded(source, 2, 4).unwrap(),
            parse_lenient(source).unwrap()
        );
        assert_eq!(
            parse_lenient_bounded(source, 1, 4).unwrap_err().kind,
            ParseErrorKind::NestingLimit
        );
        assert_eq!(
            parse_lenient_bounded(source, 2, 3).unwrap_err().kind,
            ParseErrorKind::NodeLimit
        );
        assert_eq!(
            parse_lenient_bounded("root {}\0", 1, 1).unwrap_err().kind,
            ParseErrorKind::NulByte
        );
        assert_eq!(
            parse_lenient_bounded("root { incomplete", 1, 2)
                .unwrap_err()
                .kind,
            ParseErrorKind::MissingValue
        );
    }

    #[test]
    fn bounded_input_checks_structure_before_building_recursive_nodes() {
        assert!(parse_bounded("root { key value }", 1, 2).is_ok());
        assert_eq!(
            parse_bounded("root { nested {} }", 1, 3).unwrap_err().kind,
            ParseErrorKind::NestingLimit
        );
        assert_eq!(
            parse_bounded("root { key value }", 1, 1).unwrap_err().kind,
            ParseErrorKind::NodeLimit
        );
        assert_eq!(
            parse_bounded("root {", 1, 2).unwrap_err().kind,
            ParseErrorKind::UnclosedBlock
        );
        assert_eq!(
            parse_bounded("root {}\0", 1, 2).unwrap_err().kind,
            ParseErrorKind::NulByte
        );
    }
}
