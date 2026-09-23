use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    Text(String),
    List(Vec<Value>),
    Map(BTreeMap<String, Value>),
}

impl Value {
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Self::Number(number) => Some(*number),
            Self::Text(text) => text.trim().parse().ok(),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Self::List(list) => Some(list),
            _ => None,
        }
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Map(map) => map.get(key),
            _ => None,
        }
    }
}

pub fn parse(text: &str) -> Result<Value, String> {
    let letters: Vec<char> = text.chars().collect();
    let mut reader = Reader { letters, at: 0 };
    reader.skip_space();
    let value = reader.value()?;
    reader.skip_space();
    if reader.at < reader.letters.len() {
        return Err(reader.complain("there is more here than one value"));
    }
    Ok(value)
}

struct Reader {
    letters: Vec<char>,
    at: usize,
}

impl Reader {
    fn peek(&self) -> Option<char> {
        self.letters.get(self.at).copied()
    }

    fn skip_space(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.at += 1;
        }
    }

    fn complain(&self, why: &str) -> String {
        format!("{why}, at character {}", self.at + 1)
    }

    fn take(&mut self, letter: char, why: &str) -> Result<(), String> {
        if self.peek() == Some(letter) {
            self.at += 1;
            return Ok(());
        }
        Err(self.complain(why))
    }

    fn value(&mut self) -> Result<Value, String> {
        match self.peek() {
            Some('{') => self.map(),
            Some('[') => self.list(),
            Some('"') => self.text().map(Value::Text),
            Some('\'') => Err(self.complain("a string is written in double quotes, not single")),
            Some(letter) if letter == '-' || letter.is_ascii_digit() => self.number(),
            Some(_) => self.word(),
            None => Err(self.complain("there is nothing here")),
        }
    }

    fn word(&mut self) -> Result<Value, String> {
        for (word, value) in [
            ("true", Value::Bool(true)),
            ("false", Value::Bool(false)),
            ("null", Value::Null),
        ] {
            let end = self.at + word.len();
            if self
                .letters
                .get(self.at..end)
                .is_some_and(|seen| seen.iter().collect::<String>() == word)
            {
                self.at = end;
                return Ok(value);
            }
        }
        Err(self.complain("this is not a value"))
    }

    fn number(&mut self) -> Result<Value, String> {
        let start = self.at;
        if self.peek() == Some('-') {
            self.at += 1;
        }
        while self.peek().is_some_and(|letter| {
            letter.is_ascii_digit() || matches!(letter, '.' | 'e' | 'E' | '+' | '-')
        }) {
            self.at += 1;
        }
        let written: String = self.letters[start..self.at].iter().collect();
        written
            .parse::<f64>()
            .map(Value::Number)
            .map_err(|_| format!("`{written}` is not a number, at character {}", start + 1))
    }

    fn text(&mut self) -> Result<String, String> {
        self.take('"', "a string starts with a double quote")?;
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Err(self.complain("this string is never closed")),
                Some('"') => {
                    self.at += 1;
                    return Ok(out);
                }
                Some('\\') => {
                    self.at += 1;
                    out.push(self.escaped()?);
                }
                Some(letter) => {
                    self.at += 1;
                    out.push(letter);
                }
            }
        }
    }

    fn escaped(&mut self) -> Result<char, String> {
        let letter = self
            .peek()
            .ok_or_else(|| self.complain("this string ends in a backslash"))?;
        self.at += 1;
        Ok(match letter {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            'b' => '\u{8}',
            'f' => '\u{c}',
            'u' => self.hex()?,
            '"' | '\\' | '/' => letter,
            _ => return Err(self.complain("this is not something a backslash may introduce")),
        })
    }

    fn hex(&mut self) -> Result<char, String> {
        let end = self.at + 4;
        let digits: String = self
            .letters
            .get(self.at..end)
            .ok_or_else(|| self.complain("a \\u needs four hexadecimal digits"))?
            .iter()
            .collect();
        let number = u32::from_str_radix(&digits, 16)
            .map_err(|_| self.complain("a \\u needs four hexadecimal digits"))?;
        self.at = end;
        char::from_u32(number).ok_or_else(|| self.complain("this is not a character"))
    }

    fn list(&mut self) -> Result<Value, String> {
        self.take('[', "a list starts with [")?;
        let mut out = Vec::new();
        self.skip_space();
        if self.peek() == Some(']') {
            self.at += 1;
            return Ok(Value::List(out));
        }
        loop {
            self.skip_space();
            out.push(self.value()?);
            self.skip_space();
            match self.peek() {
                Some(',') => self.at += 1,
                Some(']') => {
                    self.at += 1;
                    return Ok(Value::List(out));
                }
                _ => return Err(self.complain("a list wants a comma or a ]")),
            }
        }
    }

    fn map(&mut self) -> Result<Value, String> {
        self.take('{', "an object starts with {")?;
        let mut out = BTreeMap::new();
        self.skip_space();
        if self.peek() == Some('}') {
            self.at += 1;
            return Ok(Value::Map(out));
        }
        loop {
            self.skip_space();
            if self.peek() != Some('"') {
                return Err(self.complain("a key is written in double quotes"));
            }
            let key = self.text()?;
            self.skip_space();
            self.take(':', "a key is followed by a colon")?;
            self.skip_space();
            let value = self.value()?;
            out.insert(key, value);
            self.skip_space();
            match self.peek() {
                Some(',') => self.at += 1,
                Some('}') => {
                    self.at += 1;
                    return Ok(Value::Map(out));
                }
                _ => return Err(self.complain("an object wants a comma or a }")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Value, parse};

    #[test]
    fn a_whole_object_reads_back() {
        let value =
            parse(r#"{"a": "x\ny", "b": [1, -2.5, 1e3], "c": {"d": true}, "e": null, "f": "ไ"}"#)
                .expect("read");
        assert_eq!(value.get("a").and_then(Value::as_text), Some("x\ny"));
        let list = value.get("b").and_then(Value::as_list).expect("a list");
        let numbers: Vec<f64> = list.iter().filter_map(Value::as_number).collect();
        assert_eq!(numbers, vec![1.0, -2.5, 1000.0]);
        assert_eq!(
            value.get("c").and_then(|it| it.get("d")),
            Some(&Value::Bool(true))
        );
        assert_eq!(value.get("e"), Some(&Value::Null));
        assert_eq!(value.get("f").and_then(Value::as_text), Some("ไ"));
    }

    #[test]
    fn the_shapes_a_model_gets_wrong_are_refused() {
        for (bad, good) in [
            (r#"{"a": 1,}"#, r#"{"a": 1}"#),
            ("{'a': 1}", r#"{"a": 1}"#),
            ("{a: 1}", r#"{"a": 1}"#),
            (r#"{"a": [1 2]}"#, r#"{"a": [1, 2]}"#),
            (r#"{"a": "x}"#, r#"{"a": "x"}"#),
            (r#"{"a": 1} {"b": 2}"#, r#"{"a": 1}"#),
            (r#"{"a": yes}"#, r#"{"a": true}"#),
        ] {
            let why = parse(bad).expect_err(bad);
            assert!(
                why.contains("character") || why.contains("more here"),
                "{bad}: {why}"
            );
            parse(good).unwrap_or_else(|why| panic!("{good}: {why}"));
        }
    }

    #[test]
    fn a_quoted_number_is_a_number() {
        assert_eq!(Value::Text("12".to_owned()).as_number(), Some(12.0));
        assert_eq!(Value::Text("no".to_owned()).as_number(), None);
        assert_eq!(Value::Bool(true).as_number(), None);
    }
}
