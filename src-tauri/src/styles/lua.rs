//! Minimal reader for Lightroom's `.lrtemplate` files: a Lua assignment `s = { ... }` whose
//! value is a table literal of strings, numbers, booleans and nested tables. Only the data
//! subset Lightroom writes is supported (no expressions, no function calls); comments
//! (`--` to end of line) are skipped.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Num(f64),
    Bool(bool),
    Nil,
    Table(Table),
}

/// A Lua table: named fields (`key = v`, `["key"] = v`) and positional items (`v, v, ...`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    pub fields: BTreeMap<String, Value>,
    pub items: Vec<Value>,
}

impl Table {
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.fields.get(key)
    }

    pub fn table(&self, key: &str) -> Option<&Table> {
        match self.get(key) {
            Some(Value::Table(t)) => Some(t),
            _ => None,
        }
    }

    pub fn str(&self, key: &str) -> Option<&str> {
        match self.get(key) {
            Some(Value::Str(s)) => Some(s),
            _ => None,
        }
    }
}

/// Parses `name = { ... }` (the first assignment) or a bare table literal.
pub fn parse_template(text: &str) -> Result<Table, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut p = Parser { s: text.as_bytes(), i: 0 };
    p.ws();
    if p.peek() != Some(b'{') {
        p.ident().ok_or("expected `s = {`")?;
        p.ws();
        if p.peek() != Some(b'=') {
            return Err("expected `=`".into());
        }
        p.i += 1;
        p.ws();
    }
    match p.value(0)? {
        Value::Table(t) => Ok(t),
        _ => Err("expected a table".into()),
    }
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    /// Skips whitespace and comments (`--...` line comments, `--[[ ... ]]` block comments).
    fn ws(&mut self) {
        loop {
            while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
                self.i += 1;
            }
            if self.s[self.i..].starts_with(b"--") {
                self.i += 2;
                if self.s[self.i..].starts_with(b"[[") {
                    match find(&self.s[self.i..], b"]]") {
                        Some(k) => self.i += k + 2,
                        None => self.i = self.s.len(),
                    }
                } else {
                    while self.peek().is_some_and(|c| c != b'\n') {
                        self.i += 1;
                    }
                }
                continue;
            }
            break;
        }
    }

    fn ident(&mut self) -> Option<String> {
        let start = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_') {
            self.i += 1;
        }
        if self.i > start && !self.s[start].is_ascii_digit() {
            Some(String::from_utf8_lossy(&self.s[start..self.i]).into_owned())
        } else {
            self.i = start;
            None
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, String> {
        if depth > 64 {
            return Err("tables nested too deeply".into());
        }
        self.ws();
        match self.peek() {
            Some(b'{') => self.table(depth).map(Value::Table),
            Some(b'"') | Some(b'\'') => self.string().map(Value::Str),
            Some(b'[') if matches!(self.s.get(self.i + 1), Some(b'[') | Some(b'=')) => {
                self.long_string().map(Value::Str)
            }
            Some(c) if c == b'-' || c == b'+' || c == b'.' || c.is_ascii_digit() => self.number().map(Value::Num),
            Some(_) => {
                let at = self.i;
                match self.ident().as_deref() {
                    Some("true") => Ok(Value::Bool(true)),
                    Some("false") => Ok(Value::Bool(false)),
                    Some("nil") => Ok(Value::Nil),
                    _ => Err(format!("unexpected input at byte {at}")),
                }
            }
            None => Err("unexpected end of file".into()),
        }
    }

    fn table(&mut self, depth: usize) -> Result<Table, String> {
        self.i += 1; // '{'
        let mut t = Table::default();
        loop {
            self.ws();
            match self.peek() {
                Some(b'}') => {
                    self.i += 1;
                    return Ok(t);
                }
                None => return Err("unterminated table".into()),
                _ => {}
            }
            // `[key] = v`, `name = v` or a positional `v`.
            let save = self.i;
            let mut key: Option<String> = None;
            if self.peek() == Some(b'[') && !matches!(self.s.get(self.i + 1), Some(b'[') | Some(b'=')) {
                self.i += 1;
                let k = self.value(depth + 1)?;
                self.ws();
                if self.peek() != Some(b']') {
                    return Err("expected `]`".into());
                }
                self.i += 1;
                key = Some(match k {
                    Value::Str(s) => s,
                    Value::Num(n) => format!("{n}"),
                    _ => return Err("unsupported table key".into()),
                });
            } else if let Some(name) = self.ident() {
                self.ws();
                if self.peek() == Some(b'=') && self.s.get(self.i + 1) != Some(&b'=') {
                    key = Some(name);
                } else {
                    self.i = save;
                }
            }
            if key.is_some() {
                self.ws();
                if self.peek() != Some(b'=') {
                    return Err("expected `=`".into());
                }
                self.i += 1;
            }
            let v = self.value(depth + 1)?;
            match key {
                Some(k) => {
                    t.fields.insert(k, v);
                }
                None => t.items.push(v),
            }
            self.ws();
            match self.peek() {
                Some(b',') | Some(b';') => self.i += 1,
                Some(b'}') => {}
                _ => return Err(format!("expected `,` or `}}` at byte {}", self.i)),
            }
        }
    }

    fn string(&mut self) -> Result<String, String> {
        let quote = self.s[self.i];
        self.i += 1;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let Some(c) = self.peek() else { return Err("unterminated string".into()) };
            self.i += 1;
            match c {
                c if c == quote => break,
                b'\\' => {
                    let Some(e) = self.peek() else { return Err("unterminated string".into()) };
                    self.i += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        b'\n' => out.push(b'\n'),
                        b'0'..=b'9' => {
                            let mut v = u32::from(e - b'0');
                            for _ in 0..2 {
                                match self.peek() {
                                    Some(d @ b'0'..=b'9') => {
                                        v = v * 10 + u32::from(d - b'0');
                                        self.i += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push(v.min(255) as u8);
                        }
                        other => out.push(other),
                    }
                }
                c => out.push(c),
            }
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    /// `[[...]]` / `[==[...]==]`.
    fn long_string(&mut self) -> Result<String, String> {
        self.i += 1;
        let mut level = 0;
        while self.peek() == Some(b'=') {
            level += 1;
            self.i += 1;
        }
        if self.peek() != Some(b'[') {
            return Err("bad long string".into());
        }
        self.i += 1;
        let close: Vec<u8> = std::iter::once(b']').chain(std::iter::repeat_n(b'=', level)).chain(*b"]").collect();
        let k = find(&self.s[self.i..], &close).ok_or("unterminated long string")?;
        let mut body = &self.s[self.i..self.i + k];
        if body.first() == Some(&b'\n') {
            body = &body[1..];
        }
        self.i += k + close.len();
        Ok(String::from_utf8_lossy(body).into_owned())
    }

    fn number(&mut self) -> Result<f64, String> {
        let start = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'+')) {
            // Stop a sign that is not the first character or an exponent sign.
            if matches!(self.peek(), Some(b'-') | Some(b'+'))
                && self.i > start
                && !matches!(self.s[self.i - 1], b'e' | b'E')
            {
                break;
            }
            self.i += 1;
        }
        let t = std::str::from_utf8(&self.s[start..self.i]).map_err(|e| e.to_string())?;
        let t = t.strip_prefix('+').unwrap_or(t);
        if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
            return i64::from_str_radix(hex, 16).map(|v| v as f64).map_err(|_| format!("bad number {t:?}"));
        }
        t.parse::<f64>().map_err(|_| format!("bad number {t:?}"))
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lightroom_template() {
        let t = parse_template(
            r#"s = {
	id = "0F2F1B3C-1C6E-4D35-A3B4-8A0C1A1A7C11",
	internalName = "Warm Film",
	title = "$$$/Presets/Warm=Warm Film",
	type = "Develop",
	value = {
		settings = {
			ConvertToGrayscale = false,
			Exposure2012 = -0.35,
			HueAdjustmentRed = 5,
			ToneCurvePV2012 = { 0, 0, 64, 58, 255, 255, },
			WhiteBalance = "Custom", -- comment
			["Look"] = { Name = "Adobe Color", Amount = 1, UUID = "B952C231111CD8E0ECCF14B86BAA7077", },
			Note = [[multi
line]],
		},
		uuid = "3B0C8C5E",
	},
	version = 0,
}
"#,
        )
        .unwrap();
        assert_eq!(t.str("type"), Some("Develop"));
        let s = t.table("value").unwrap().table("settings").unwrap();
        assert_eq!(s.get("Exposure2012"), Some(&Value::Num(-0.35)));
        assert_eq!(s.get("ConvertToGrayscale"), Some(&Value::Bool(false)));
        assert_eq!(s.str("WhiteBalance"), Some("Custom"));
        let curve = s.table("ToneCurvePV2012").unwrap();
        assert_eq!(curve.items.len(), 6);
        assert_eq!(s.table("Look").unwrap().str("UUID"), Some("B952C231111CD8E0ECCF14B86BAA7077"));
        assert_eq!(s.str("Note"), Some("multi\nline"));
        assert!(parse_template("s = { a = }").is_err());
        assert!(parse_template("s = { a = 1").is_err());
        assert_eq!(
            parse_template("{ 1e2, -3, 0x10 }").unwrap().items,
            vec![Value::Num(100.0), Value::Num(-3.0), Value::Num(16.0)]
        );
    }
}
