#[derive(Clone, Debug)]
pub struct Formula(Node);

#[derive(Clone, Debug)]
enum Node {
    Number(f64),
    X,
    Negate(Box<Node>),
    Apply(Op, Box<Node>, Box<Node>),
    Call(fn(f64) -> f64, Box<Node>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Add,
    Subtract,
    Multiply,
    Divide,
    Power,
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Number(f64),
    Name(String),
    Symbol(char),
}

fn tokens(text: &str) -> Result<Vec<Token>, String> {
    let letters: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut at = 0;
    while at < letters.len() {
        let letter = letters[at];
        if letter.is_whitespace() {
            at += 1;
        } else if letter.is_ascii_digit() || letter == '.' {
            let start = at;
            while at < letters.len() && (letters[at].is_ascii_digit() || letters[at] == '.') {
                at += 1;
            }
            if at + 1 < letters.len()
                && (letters[at] == 'e' || letters[at] == 'E')
                && (letters[at + 1].is_ascii_digit()
                    || ((letters[at + 1] == '-' || letters[at + 1] == '+')
                        && letters.get(at + 2).is_some_and(char::is_ascii_digit)))
            {
                at += 2;
                while at < letters.len() && letters[at].is_ascii_digit() {
                    at += 1;
                }
            }
            let written: String = letters[start..at].iter().collect();
            let value = written
                .parse::<f64>()
                .map_err(|_| format!("`{written}` is not a number"))?;
            out.push(Token::Number(value));
        } else if letter.is_ascii_alphabetic() || letter == 'π' {
            let start = at;
            while at < letters.len()
                && (letters[at].is_ascii_alphanumeric() || letters[at] == '_' || letters[at] == 'π')
            {
                at += 1;
            }
            out.push(Token::Name(letters[start..at].iter().collect()));
        } else if letter == '*' && letters.get(at + 1) == Some(&'*') {
            out.push(Token::Symbol('^'));
            at += 2;
        } else {
            let symbol = match letter {
                '−' | '–' => '-',
                '×' | '·' | '⋅' => '*',
                '÷' => '/',
                other => other,
            };
            if !"+-*/^()|,".contains(symbol) {
                return Err(format!("`{letter}` means nothing in a formula"));
            }
            out.push(Token::Symbol(symbol));
            at += 1;
        }
    }
    Ok(out)
}

fn function(name: &str) -> Option<fn(f64) -> f64> {
    Some(match name {
        "sin" => f64::sin,
        "cos" => f64::cos,
        "tan" => f64::tan,
        "asin" | "arcsin" => f64::asin,
        "acos" | "arccos" => f64::acos,
        "atan" | "arctan" => f64::atan,
        "sinh" => f64::sinh,
        "cosh" => f64::cosh,
        "tanh" => f64::tanh,
        "exp" => f64::exp,
        "ln" => f64::ln,
        "log" | "log10" => f64::log10,
        "log2" => f64::log2,
        "sqrt" => f64::sqrt,
        "cbrt" => f64::cbrt,
        "abs" => f64::abs,
        "floor" => f64::floor,
        "ceil" => f64::ceil,
        "round" => f64::round,
        "sign" | "sgn" => f64::signum,
        _ => return None,
    })
}

struct Reader {
    tokens: Vec<Token>,
    at: usize,
}

impl Reader {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn symbol(&self, wanted: char) -> bool {
        self.peek() == Some(&Token::Symbol(wanted))
    }

    fn sum(&mut self) -> Result<Node, String> {
        let mut left = self.product()?;
        loop {
            let op = if self.symbol('+') {
                Op::Add
            } else if self.symbol('-') {
                Op::Subtract
            } else {
                return Ok(left);
            };
            self.at += 1;
            let right = self.product()?;
            left = Node::Apply(op, Box::new(left), Box::new(right));
        }
    }

    fn product(&mut self) -> Result<Node, String> {
        let mut left = self.sign()?;
        loop {
            let op = if self.symbol('*') {
                self.at += 1;
                Op::Multiply
            } else if self.symbol('/') {
                self.at += 1;
                Op::Divide
            } else if matches!(self.peek(), Some(Token::Number(_) | Token::Name(_)))
                || self.symbol('(')
            {
                Op::Multiply
            } else {
                return Ok(left);
            };
            let right = self.sign()?;
            left = Node::Apply(op, Box::new(left), Box::new(right));
        }
    }

    fn sign(&mut self) -> Result<Node, String> {
        if self.symbol('-') {
            self.at += 1;
            return Ok(Node::Negate(Box::new(self.sign()?)));
        }
        if self.symbol('+') {
            self.at += 1;
            return self.sign();
        }
        self.power()
    }

    fn power(&mut self) -> Result<Node, String> {
        let base = self.atom()?;
        if self.symbol('^') {
            self.at += 1;
            let exponent = self.sign()?;
            return Ok(Node::Apply(Op::Power, Box::new(base), Box::new(exponent)));
        }
        Ok(base)
    }

    fn atom(&mut self) -> Result<Node, String> {
        let token = self
            .peek()
            .cloned()
            .ok_or_else(|| "the formula ends too soon".to_owned())?;
        self.at += 1;
        match token {
            Token::Number(value) => Ok(Node::Number(value)),
            Token::Symbol('(') => {
                let inside = self.sum()?;
                if !self.symbol(')') {
                    return Err("a `(` is not closed".to_owned());
                }
                self.at += 1;
                Ok(inside)
            }
            Token::Symbol('|') => {
                let inside = self.sum()?;
                if !self.symbol('|') {
                    return Err("a `|` is not closed".to_owned());
                }
                self.at += 1;
                Ok(Node::Call(f64::abs, Box::new(inside)))
            }
            Token::Symbol(other) => Err(format!("`{other}` cannot start a term")),
            Token::Name(name) => match name.as_str() {
                "x" | "X" => Ok(Node::X),
                "pi" | "PI" | "π" => Ok(Node::Number(std::f64::consts::PI)),
                "e" => Ok(Node::Number(std::f64::consts::E)),
                "tau" => Ok(Node::Number(std::f64::consts::TAU)),
                _ => {
                    let call = function(&name).ok_or_else(|| {
                        format!(
                            "`{name}` is not a function or a name this knows; the variable is x"
                        )
                    })?;
                    let argument = if self.symbol('(') {
                        self.atom()?
                    } else {
                        self.power()?
                    };
                    let argument = if self.symbol('^') {
                        self.at += 1;
                        let exponent = self.sign()?;
                        return Ok(Node::Apply(
                            Op::Power,
                            Box::new(Node::Call(call, Box::new(argument))),
                            Box::new(exponent),
                        ));
                    } else {
                        argument
                    };
                    Ok(Node::Call(call, Box::new(argument)))
                }
            },
        }
    }
}

impl Formula {
    pub fn read(text: &str) -> Result<Self, String> {
        let text = text.rsplit('=').next().unwrap_or(text);
        let mut reader = Reader {
            tokens: tokens(text)?,
            at: 0,
        };
        if reader.tokens.is_empty() {
            return Err("the formula is empty".to_owned());
        }
        let node = reader.sum()?;
        if reader.at < reader.tokens.len() {
            return Err("the formula has something left over at its end".to_owned());
        }
        Ok(Self(node))
    }

    #[must_use]
    pub fn at(&self, x: f64) -> f64 {
        value(&self.0, x)
    }
}

fn value(node: &Node, x: f64) -> f64 {
    match node {
        Node::Number(number) => *number,
        Node::X => x,
        Node::Negate(inside) => -value(inside, x),
        Node::Call(call, inside) => call(value(inside, x)),
        Node::Apply(op, left, right) => {
            let (a, b) = (value(left, x), value(right, x));
            match op {
                Op::Add => a + b,
                Op::Subtract => a - b,
                Op::Multiply => a * b,
                Op::Divide => a / b,
                Op::Power => a.powf(b),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Formula;

    fn at(text: &str, x: f64) -> f64 {
        Formula::read(text).expect("it reads").at(x)
    }

    #[test]
    fn formulas_work_out_as_written() {
        assert!((at("exp(-x^2)", 0.0) - 1.0).abs() < 1e-12);
        assert!((at("-x^2", 3.0) + 9.0).abs() < 1e-12);
        assert!((at("2x + 1", 4.0) - 9.0).abs() < 1e-12);
        assert!((at("3(x+1)", 1.0) - 6.0).abs() < 1e-12);
        assert!((at("sin(pi/2)", 0.0) - 1.0).abs() < 1e-12);
        assert!((at("sin(x)^2 + cos(x)^2", 0.7) - 1.0).abs() < 1e-12);
        assert!((at("y = x^3 - 2x", 2.0) - 4.0).abs() < 1e-12);
        assert!((at("2^-x", 1.0) - 0.5).abs() < 1e-12);
        assert!((at("|x - 5|", 2.0) - 3.0).abs() < 1e-12);
        assert!((at("1e-3 * x", 1000.0) - 1.0).abs() < 1e-12);
        assert!((at("x**2", 5.0) - 25.0).abs() < 1e-12);
        assert!((at("ln(e)", 0.0) - 1.0).abs() < 1e-12);
        assert!(at("1/x", 0.0).is_infinite());
        assert!(Formula::read("foo(x)").is_err());
        assert!(Formula::read("(x + 1").is_err());
        assert!(Formula::read("x $ 2").is_err());
    }
}
