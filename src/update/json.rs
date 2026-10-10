//! Just enough JSON to read one GitHub release.
//!
//! A hand-rolled reader rather than `serde_json`, for the reason [`crate::config`] gives for its own
//! format: the one document read here is the answer to `releases/latest`, and four strings out of it
//! is all this program wants. A reader that builds the whole tree is sixty lines; a derive, a
//! dependency and a schema for a file whose shape GitHub owns would be larger and no more right.
//!
//! **Strict where it matters and nowhere else.** Strings are decoded properly — escapes, `\u`
//! surrogate pairs — because a URL is read out of one. Numbers are kept as their text, because
//! nothing here does arithmetic on any of them. Anything malformed is `None`, never a panic: this
//! is text off the network.

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// The literal, unparsed.
    Number(String),
    String(String),
    Array(Vec<Value>),
    /// In document order. A release has a few dozen keys, so a scan beats a map.
    Object(Vec<(String, Value)>),
}

impl Value {
    /// The member called `key`, if this is an object that has one.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(members) => members.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(text) => Some(text),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }
}

/// The whole document, or `None` if any of it is not JSON.
pub fn parse(text: &str) -> Option<Value> {
    let mut reader = Reader {
        bytes: text.as_bytes(),
        at: 0,
        depth: 0,
    };
    let value = reader.value()?;
    reader.space();
    (reader.at == reader.bytes.len()).then_some(value)
}

/// How deeply nested a document may be. A release is four levels deep; the bound is there so that
/// a hostile answer costs a bounded amount of stack rather than all of it.
const DEPTH: usize = 64;

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    depth: usize,
}

impl Reader<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn eat(&mut self, byte: u8) -> Option<()> {
        self.space();
        (self.peek() == Some(byte)).then(|| self.at += 1)
    }

    fn word(&mut self, word: &str, value: Value) -> Option<Value> {
        let end = self.at + word.len();
        (self.bytes.get(self.at..end) == Some(word.as_bytes())).then(|| {
            self.at = end;
            value
        })
    }

    fn value(&mut self) -> Option<Value> {
        self.space();
        match self.peek()? {
            b'{' => self.nested(Self::object),
            b'[' => self.nested(Self::array),
            b'"' => self.string().map(Value::String),
            b't' => self.word("true", Value::Bool(true)),
            b'f' => self.word("false", Value::Bool(false)),
            b'n' => self.word("null", Value::Null),
            b'-' | b'0'..=b'9' => self.number(),
            _ => None,
        }
    }

    fn nested(&mut self, read: fn(&mut Self) -> Option<Value>) -> Option<Value> {
        self.depth += 1;
        if self.depth > DEPTH {
            return None;
        }
        let value = read(self);
        self.depth -= 1;
        value
    }

    fn object(&mut self) -> Option<Value> {
        self.eat(b'{')?;
        let mut members = Vec::new();
        if self.eat(b'}').is_some() {
            return Some(Value::Object(members));
        }
        loop {
            self.space();
            let key = self.string()?;
            self.eat(b':')?;
            members.push((key, self.value()?));
            if self.eat(b',').is_some() {
                continue;
            }
            self.eat(b'}')?;
            return Some(Value::Object(members));
        }
    }

    fn array(&mut self) -> Option<Value> {
        self.eat(b'[')?;
        let mut items = Vec::new();
        if self.eat(b']').is_some() {
            return Some(Value::Array(items));
        }
        loop {
            items.push(self.value()?);
            if self.eat(b',').is_some() {
                continue;
            }
            self.eat(b']')?;
            return Some(Value::Array(items));
        }
    }

    fn number(&mut self) -> Option<Value> {
        let start = self.at;
        while matches!(self.peek(), Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')) {
            self.at += 1;
        }
        let text = std::str::from_utf8(&self.bytes[start..self.at]).ok()?;
        text.parse::<f64>().ok()?;
        Some(Value::Number(text.to_owned()))
    }

    fn string(&mut self) -> Option<String> {
        if self.peek()? != b'"' {
            return None;
        }
        self.at += 1;
        let mut out = String::new();
        loop {
            // A run with nothing to decode in it, copied whole. The input is a `&str`, so any run
            // that stops at an ASCII quote or backslash is still on a character boundary.
            let start = self.at;
            while !matches!(self.peek()?, b'"' | b'\\') {
                if self.peek()? < 0x20 {
                    return None;
                }
                self.at += 1;
            }
            out.push_str(std::str::from_utf8(&self.bytes[start..self.at]).ok()?);
            let byte = self.peek()?;
            self.at += 1;
            if byte == b'"' {
                return Some(out);
            }
            let escape = self.peek()?;
            self.at += 1;
            match escape {
                b'"' => out.push('"'),
                b'\\' => out.push('\\'),
                b'/' => out.push('/'),
                b'b' => out.push('\u{8}'),
                b'f' => out.push('\u{c}'),
                b'n' => out.push('\n'),
                b'r' => out.push('\r'),
                b't' => out.push('\t'),
                b'u' => {
                    let high = self.hex4()?;
                    let code = if (0xD800..0xDC00).contains(&high) {
                        // A surrogate pair, which is how JSON spells anything past the BMP — an
                        // emoji in a release's notes, say.
                        if self.bytes.get(self.at..self.at + 2) != Some(b"\\u") {
                            return None;
                        }
                        self.at += 2;
                        let low = self.hex4()?;
                        if !(0xDC00..0xE000).contains(&low) {
                            return None;
                        }
                        0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00)
                    } else {
                        high
                    };
                    out.push(char::from_u32(code)?);
                }
                _ => return None,
            }
        }
    }

    fn hex4(&mut self) -> Option<u32> {
        let digits = self.bytes.get(self.at..self.at + 4)?;
        // `from_str_radix` takes a leading `+`, which JSON does not.
        if !digits.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        let code = u32::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok()?;
        self.at += 4;
        Some(code)
    }
}
