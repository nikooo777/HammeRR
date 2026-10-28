use std::io;

use serde_json::{Map, Value};

use crate::util::invalid;

pub(crate) fn parse(source: &str) -> io::Result<Value> {
    let mut parser = Parser {
        bytes: source.as_bytes(),
        cursor: 0,
        nodes: 0,
    };
    parser.block(false, 0)
}

enum Token {
    Text(String),
    Open,
    Close,
}

struct Parser<'a> {
    bytes: &'a [u8],
    cursor: usize,
    nodes: usize,
}

impl Parser<'_> {
    fn skip(&mut self) {
        loop {
            while self
                .bytes
                .get(self.cursor)
                .is_some_and(u8::is_ascii_whitespace)
            {
                self.cursor += 1;
            }
            if self.bytes.get(self.cursor..self.cursor.saturating_add(2)) == Some(b"//") {
                while self.bytes.get(self.cursor).is_some_and(|c| *c != b'\n') {
                    self.cursor += 1;
                }
            } else {
                break;
            }
        }
    }

    fn token(&mut self) -> io::Result<Option<Token>> {
        self.skip();
        let Some(&first) = self.bytes.get(self.cursor) else {
            return Ok(None);
        };
        self.cursor += 1;
        if first == b'{' {
            return Ok(Some(Token::Open));
        }
        if first == b'}' {
            return Ok(Some(Token::Close));
        }
        let mut out = Vec::new();
        if first == b'"' {
            loop {
                let c = *self
                    .bytes
                    .get(self.cursor)
                    .ok_or_else(|| invalid("Unterminated VDF string"))?;
                self.cursor += 1;
                if c == b'"' {
                    break;
                }
                if c == b'\\'
                    && self
                        .bytes
                        .get(self.cursor)
                        .is_some_and(|c| matches!(*c, b'"' | b'\\'))
                {
                    out.push(self.bytes[self.cursor]);
                    self.cursor += 1;
                } else {
                    out.push(c);
                }
            }
        } else {
            out.push(first);
            while let Some(&c) = self.bytes.get(self.cursor) {
                if c.is_ascii_whitespace() || matches!(c, b'{' | b'}') {
                    break;
                }
                out.push(c);
                self.cursor += 1;
            }
        }
        if out.contains(&0) {
            return Err(invalid("NUL in VDF"));
        }
        String::from_utf8(out)
            .map(|text| Some(Token::Text(text)))
            .map_err(|e| invalid(e.to_string()))
    }

    fn block(&mut self, nested: bool, depth: usize) -> io::Result<Value> {
        if depth > 64 {
            return Err(invalid("VDF nesting budget exceeded"));
        }
        let mut map = Map::new();
        loop {
            let Some(key) = self.token()? else {
                return if nested {
                    Err(invalid("Unterminated VDF block"))
                } else {
                    Ok(Value::Object(map))
                };
            };
            let key = match key {
                Token::Close if nested => return Ok(Value::Object(map)),
                Token::Text(key) => key,
                _ => return Err(invalid("Unexpected VDF brace")),
            };
            self.nodes += 1;
            if self.nodes > 262_144 {
                return Err(invalid("VDF node budget exceeded"));
            }
            let value = match self.token()?.ok_or_else(|| invalid("Missing VDF value"))? {
                Token::Open => self.block(true, depth + 1)?,
                Token::Close => return Err(invalid("Missing VDF value")),
                Token::Text(value) => Value::String(value),
            };
            map.insert(key.to_ascii_lowercase(), value);
        }
    }
}
