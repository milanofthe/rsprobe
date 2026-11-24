//! JSON: a reader for records and traces, and the indented writer of
//! `report --json`. Numbers keep their text.

use crate::Result;

/// A JSON value; numbers as written, object members in document order.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(String),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    /// The value of a document.
    pub fn parse(text: &str) -> Result<Json> {
        let mut p = Parser { b: text.as_bytes(), i: 0 };
        let v = p.value()?;
        p.ws();
        if p.i < p.b.len() {
            return Err(p.err("trailing characters"));
        }
        Ok(v)
    }

    /// A number, integral values without a fraction (see [`crate::fmt::num`]).
    pub fn num(x: f64) -> Json {
        Json::Num(crate::fmt::num(x))
    }

    /// An object of the members.
    pub fn obj<K: Into<String>>(kv: impl IntoIterator<Item = (K, Json)>) -> Json {
        Json::Obj(kv.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    /// The member `key` of an object.
    pub fn get(&self, key: &str) -> Option<&Json> {
        self.members()?.iter().find(|m| m.0 == key).map(|m| &m.1)
    }

    /// A number.
    pub fn f64(&self) -> Option<f64> {
        let Json::Num(t) = self else { return None };
        t.parse().ok()
    }

    /// A non-negative integer.
    pub fn u64(&self) -> Option<u64> {
        let Json::Num(t) = self else { return None };
        let x = self.f64()?;
        t.parse().ok().or((x >= 0.0 && x.fract() == 0.0 && x < 1.8e19).then_some(x as u64))
    }

    /// A string.
    pub fn str(&self) -> Option<&str> {
        let Json::Str(s) = self else { return None };
        Some(s)
    }

    /// The elements of an array.
    pub fn arr(&self) -> Option<&[Json]> {
        let Json::Arr(a) = self else { return None };
        Some(a)
    }

    /// The members of an object.
    pub fn members(&self) -> Option<&[(String, Json)]> {
        let Json::Obj(o) = self else { return None };
        Some(o)
    }

    /// The document indented by one space per level, `": "` after a key,
    /// everything outside printable ASCII escaped.
    pub fn pretty(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0);
        out
    }

    fn write(&self, out: &mut String, depth: usize) {
        let (open, items): (char, Vec<(Option<&str>, &Json)>) = match self {
            Json::Null => return out.push_str("null"),
            Json::Bool(b) => return out.push_str(if *b { "true" } else { "false" }),
            Json::Num(t) => return out.push_str(t),
            Json::Str(s) => return quote(out, s),
            Json::Arr(a) => ('[', a.iter().map(|x| (None, x)).collect()),
            Json::Obj(o) => ('{', o.iter().map(|(k, x)| (Some(k.as_str()), x)).collect()),
        };
        let close = if open == '[' { ']' } else { '}' };
        out.push(open);
        let pad = |out: &mut String, d| {
            out.extend(std::iter::once('\n').chain(std::iter::repeat_n(' ', d)))
        };
        for (i, (k, x)) in items.iter().enumerate() {
            out.push_str(if i > 0 { "," } else { "" });
            pad(out, depth + 1);
            if let Some(k) = k {
                quote(out, k);
                out.push_str(": ");
            }
            x.write(out, depth + 1);
        }
        if !items.is_empty() {
            pad(out, depth);
        }
        out.push(close);
    }
}

/// A string literal, everything outside printable ASCII escaped.
fn quote(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' | '\\' => out.extend(['\\', c]),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            c => c
                .encode_utf16(&mut [0; 2])
                .iter()
                .for_each(|u| out.push_str(&format!("\\u{u:04x}"))),
        }
    }
    out.push('"');
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn err(&self, what: &str) -> String {
        format!("JSON: {what} at byte {}", self.i)
    }

    fn ws(&mut self) {
        while matches!(self.b.get(self.i), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }

    /// Skips blanks, then `s` if it follows.
    fn eat(&mut self, s: &str) -> bool {
        self.ws();
        let hit = self.b[self.i..].starts_with(s.as_bytes());
        self.i += if hit { s.len() } else { 0 };
        hit
    }

    /// Elements up to `close`, each read by `item`.
    fn list<T>(
        &mut self,
        close: &str,
        mut item: impl FnMut(&mut Self) -> Result<T>,
    ) -> Result<Vec<T>> {
        let mut out = Vec::new();
        if self.eat(close) {
            return Ok(out);
        }
        loop {
            out.push(item(self)?);
            if self.eat(close) {
                return Ok(out);
            }
            if !self.eat(",") {
                return Err(self.err(&format!("expected ',' or '{close}'")));
            }
        }
    }

    fn value(&mut self) -> Result<Json> {
        if self.eat("{") {
            let member = |p: &mut Self| {
                let k = p.string()?;
                if !p.eat(":") {
                    return Err(p.err("expected ':'"));
                }
                Ok((k, p.value()?))
            };
            return self.list("}", member).map(Json::Obj);
        }
        if self.eat("[") {
            return self.list("]", Self::value).map(Json::Arr);
        }
        for (word, v) in
            [("null", Json::Null), ("true", Json::Bool(true)), ("false", Json::Bool(false))]
        {
            if self.eat(word) {
                return Ok(v);
            }
        }
        if self.b.get(self.i) == Some(&b'"') {
            return self.string().map(Json::Str);
        }
        let start = self.i;
        while matches!(self.b.get(self.i), Some(b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')) {
            self.i += 1;
        }
        let t = std::str::from_utf8(&self.b[start..self.i]).unwrap_or("");
        t.parse::<f64>().map(|_| Json::Num(t.into())).map_err(|_| self.err("expected a value"))
    }

    fn string(&mut self) -> Result<String> {
        if !self.eat("\"") {
            return Err(self.err("expected a string"));
        }
        let mut out = Vec::new();
        loop {
            let c = *self.b.get(self.i).ok_or_else(|| self.err("unterminated string"))?;
            self.i += 1;
            let e = match c {
                b'"' => return String::from_utf8(out).map_err(|_| self.err("invalid UTF-8")),
                b'\\' => *self.b.get(self.i).ok_or_else(|| self.err("unterminated string"))?,
                c => {
                    out.push(c);
                    continue;
                }
            };
            self.i += 1;
            let c = match e {
                b'n' => '\n',
                b't' => '\t',
                b'r' => '\r',
                b'b' => '\u{8}',
                b'f' => '\u{c}',
                b'"' | b'\\' | b'/' => e as char,
                b'u' => self.unicode()?,
                _ => return Err(self.err("invalid escape")),
            };
            out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
        }
    }

    /// The four hex digits of `\u` at the cursor, a surrogate pair with the
    /// `\u` escape that follows as one character.
    fn unicode(&mut self) -> Result<char> {
        let hex = |p: &mut Self| {
            let u =
                p.b.get(p.i..p.i + 4)
                    .and_then(|h| u32::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok());
            p.i += 4;
            u.ok_or_else(|| p.err("invalid \\u escape"))
        };
        let u = hex(self)?;
        if (0xd800..0xdc00).contains(&u) && self.b[self.i..].starts_with(b"\\u") {
            self.i += 2;
            let low = hex(self)?;
            return Ok(char::from_u32(0x10000 + ((u - 0xd800) << 10) + (low & 0x3ff))
                .unwrap_or('\u{fffd}'));
        }
        Ok(char::from_u32(u).unwrap_or('\u{fffd}'))
    }
}
