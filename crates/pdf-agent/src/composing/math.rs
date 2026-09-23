use super::{Colour, Composer, Fitted, Style, shapes};
use pdf_edit::PenStep;

#[derive(Clone, Debug, PartialEq)]
enum Node {
    Row(Vec<Node>),
    Atom {
        text: String,
        class: Class,
    },
    Frac(Box<Node>, Box<Node>),
    Scripts {
        base: Box<Node>,
        sup: Option<Box<Node>>,
        sub: Option<Box<Node>>,
    },
    Sqrt {
        body: Box<Node>,
        index: Option<Box<Node>>,
    },
    Big {
        symbol: String,
        limits: bool,
    },
    Fenced {
        open: String,
        body: Box<Node>,
        close: String,
    },
    Matrix {
        rows: Vec<Vec<Node>>,
        open: String,
        close: String,
        left: bool,
    },
    Accent {
        mark: Accent,
        body: Box<Node>,
    },
    Space(f64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Letter,
    Number,
    Binary,
    Relation,
    Punct,
    Upright,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Accent {
    Vec,
    Hat,
    Bar,
    Dot,
    Ddot,
    Tilde,
}

impl Accent {
    fn combining(self) -> char {
        match self {
            Self::Vec => '\u{20D7}',
            Self::Hat => '\u{0302}',
            Self::Bar => '\u{0304}',
            Self::Dot => '\u{0307}',
            Self::Ddot => '\u{0308}',
            Self::Tilde => '\u{0303}',
        }
    }
}

#[expect(clippy::too_many_lines, reason = "a table")]
fn symbol(name: &str) -> Option<(&'static str, Class)> {
    use Class::{Binary, Letter, Punct, Relation, Upright};
    Some(match name {
        "alpha" => ("α", Letter),
        "beta" => ("β", Letter),
        "gamma" => ("γ", Letter),
        "delta" => ("δ", Letter),
        "epsilon" => ("ϵ", Letter),
        "varepsilon" => ("ε", Letter),
        "zeta" => ("ζ", Letter),
        "eta" => ("η", Letter),
        "theta" => ("θ", Letter),
        "vartheta" => ("ϑ", Letter),
        "iota" => ("ι", Letter),
        "kappa" => ("κ", Letter),
        "lambda" => ("λ", Letter),
        "mu" => ("μ", Letter),
        "nu" => ("ν", Letter),
        "xi" => ("ξ", Letter),
        "pi" => ("π", Letter),
        "varpi" => ("ϖ", Letter),
        "rho" => ("ρ", Letter),
        "varrho" => ("ϱ", Letter),
        "sigma" => ("σ", Letter),
        "varsigma" => ("ς", Letter),
        "tau" => ("τ", Letter),
        "upsilon" => ("υ", Letter),
        "phi" => ("ϕ", Letter),
        "varphi" => ("φ", Letter),
        "chi" => ("χ", Letter),
        "psi" => ("ψ", Letter),
        "omega" => ("ω", Letter),
        "Gamma" => ("Γ", Upright),
        "Delta" => ("Δ", Upright),
        "Theta" => ("Θ", Upright),
        "Lambda" => ("Λ", Upright),
        "Xi" => ("Ξ", Upright),
        "Pi" => ("Π", Upright),
        "Sigma" => ("Σ", Upright),
        "Upsilon" => ("Υ", Upright),
        "Phi" => ("Φ", Upright),
        "Psi" => ("Ψ", Upright),
        "Omega" => ("Ω", Upright),
        "infty" => ("∞", Upright),
        "partial" => ("∂", Upright),
        "nabla" => ("∇", Upright),
        "hbar" => ("ℏ", Letter),
        "ell" => ("ℓ", Letter),
        "Re" => ("ℜ", Upright),
        "Im" => ("ℑ", Upright),
        "aleph" => ("ℵ", Upright),
        "emptyset" | "varnothing" => ("∅", Upright),
        "forall" => ("∀", Upright),
        "exists" => ("∃", Upright),
        "neg" | "lnot" => ("¬", Upright),
        "angle" => ("∠", Upright),
        "triangle" => ("△", Upright),
        "degree" | "circ" => ("∘", Binary),
        "prime" => ("′", Upright),
        "ldots" | "dots" => ("…", Upright),
        "cdots" => ("⋯", Upright),
        "vdots" => ("⋮", Upright),
        "ddots" => ("⋱", Upright),
        "pm" => ("±", Binary),
        "mp" => ("∓", Binary),
        "times" => ("×", Binary),
        "div" => ("÷", Binary),
        "cdot" => ("⋅", Binary),
        "ast" => ("∗", Binary),
        "star" => ("⋆", Binary),
        "cup" => ("∪", Binary),
        "cap" => ("∩", Binary),
        "setminus" => ("∖", Binary),
        "wedge" | "land" => ("∧", Binary),
        "vee" | "lor" => ("∨", Binary),
        "oplus" => ("⊕", Binary),
        "otimes" => ("⊗", Binary),
        "leq" | "le" => ("≤", Relation),
        "geq" | "ge" => ("≥", Relation),
        "neq" | "ne" => ("≠", Relation),
        "approx" => ("≈", Relation),
        "equiv" => ("≡", Relation),
        "sim" => ("∼", Relation),
        "simeq" => ("≃", Relation),
        "cong" => ("≅", Relation),
        "propto" => ("∝", Relation),
        "ll" => ("≪", Relation),
        "gg" => ("≫", Relation),
        "in" => ("∈", Relation),
        "notin" => ("∉", Relation),
        "ni" => ("∋", Relation),
        "subset" => ("⊂", Relation),
        "subseteq" => ("⊆", Relation),
        "supset" => ("⊃", Relation),
        "supseteq" => ("⊇", Relation),
        "perp" => ("⊥", Relation),
        "parallel" => ("∥", Relation),
        "mid" => ("∣", Relation),
        "to" | "rightarrow" => ("→", Relation),
        "leftarrow" | "gets" => ("←", Relation),
        "leftrightarrow" => ("↔", Relation),
        "Rightarrow" | "implies" => ("⇒", Relation),
        "Leftarrow" => ("⇐", Relation),
        "Leftrightarrow" | "iff" => ("⇔", Relation),
        "mapsto" => ("↦", Relation),
        "uparrow" => ("↑", Relation),
        "downarrow" => ("↓", Relation),
        "colon" => (":", Punct),
        "lbrace" | "{" => ("{", Upright),
        "rbrace" | "}" => ("}", Upright),
        "langle" => ("⟨", Upright),
        "rangle" => ("⟩", Upright),
        "lfloor" => ("⌊", Upright),
        "rfloor" => ("⌋", Upright),
        "lceil" => ("⌈", Upright),
        "rceil" => ("⌉", Upright),
        "vert" | "|" => ("|", Upright),
        "Vert" => ("‖", Upright),
        "%" => ("%", Upright),
        "$" => ("$", Upright),
        "&" => ("&", Upright),
        "#" => ("#", Upright),
        "_" => ("_", Upright),
        _ => return None,
    })
}

const FUNCTIONS: [&str; 26] = [
    "sin", "cos", "tan", "cot", "sec", "csc", "arcsin", "arccos", "arctan", "sinh", "cosh", "tanh",
    "log", "ln", "lg", "exp", "det", "dim", "ker", "gcd", "deg", "arg", "Pr", "hom", "mod", "sgn",
];

fn big(name: &str) -> Option<(&'static str, bool)> {
    Some(match name {
        "sum" => ("∑", true),
        "prod" => ("∏", true),
        "coprod" => ("∐", true),
        "bigcup" => ("⋃", true),
        "bigcap" => ("⋂", true),
        "lim" => ("lim", true),
        "limsup" => ("lim sup", true),
        "liminf" => ("lim inf", true),
        "max" => ("max", true),
        "min" => ("min", true),
        "sup" => ("sup", true),
        "inf" => ("inf", true),
        "int" => ("∫", false),
        "iint" => ("∬", false),
        "iiint" => ("∭", false),
        "oint" => ("∮", false),
        _ => return None,
    })
}

fn double_struck(letter: char) -> char {
    match letter {
        'R' => 'ℝ',
        'N' => 'ℕ',
        'Z' => 'ℤ',
        'Q' => 'ℚ',
        'C' => 'ℂ',
        'P' => 'ℙ',
        'H' => 'ℍ',
        other => other,
    }
}

struct Reader {
    letters: Vec<char>,
    at: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stop {
    End,
    Brace,
    Right,
    Cell,
}

impl Reader {
    fn new(source: &str) -> Self {
        Self {
            letters: source.chars().collect(),
            at: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.letters.get(self.at).copied()
    }

    fn skip_blanks(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.at += 1;
        }
    }

    fn command(&mut self) -> String {
        let start = self.at;
        while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
            self.at += 1;
        }
        if self.at == start
            && let Some(one) = self.peek()
        {
            self.at += 1;
            return one.to_string();
        }
        self.letters[start..self.at].iter().collect()
    }

    fn looking_at(&self, name: &str) -> bool {
        if self.peek() != Some('\\') {
            return false;
        }
        let after: String = self.letters[self.at + 1..]
            .iter()
            .take(name.chars().count())
            .collect();
        after == name
            && !self
                .letters
                .get(self.at + 1 + name.chars().count())
                .is_some_and(|c| {
                    c.is_ascii_alphabetic() && name.chars().all(|n| n.is_ascii_alphabetic())
                })
    }

    fn row(&mut self, stop: Stop) -> Node {
        let mut items = Vec::new();
        loop {
            self.skip_blanks();
            let Some(letter) = self.peek() else { break };
            match letter {
                '}' if stop == Stop::Brace => {
                    self.at += 1;
                    break;
                }
                '}' => self.at += 1,
                '&' if stop == Stop::Cell => break,
                '\\' if stop == Stop::Cell && (self.looking_at("\\") || self.looking_at("end")) => {
                    break;
                }
                '\\' if stop == Stop::Right && self.looking_at("right") => break,
                '^' | '_' => {
                    self.at += 1;
                    let script = self.argument();
                    let base = items.pop().unwrap_or(Node::Row(Vec::new()));
                    items.push(attach(base, letter == '^', script));
                }
                '\'' => {
                    self.at += 1;
                    let base = items.pop().unwrap_or(Node::Row(Vec::new()));
                    items.push(attach(base, true, atom("′", Class::Upright)));
                }
                _ => {
                    if let Some(node) = self.one() {
                        items.push(node);
                    }
                }
            }
        }
        Node::Row(items)
    }

    fn argument(&mut self) -> Node {
        self.skip_blanks();
        match self.peek() {
            Some('{') => {
                self.at += 1;
                self.row(Stop::Brace)
            }
            Some(_) => self.one().unwrap_or(Node::Row(Vec::new())),
            None => Node::Row(Vec::new()),
        }
    }

    fn raw(&mut self) -> String {
        self.skip_blanks();
        if self.peek() != Some('{') {
            return self
                .peek()
                .map(|c| {
                    self.at += 1;
                    c.to_string()
                })
                .unwrap_or_default();
        }
        self.at += 1;
        let mut depth = 1;
        let mut out = String::new();
        while let Some(c) = self.peek() {
            self.at += 1;
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            out.push(c);
        }
        out
    }

    fn delimiter(&mut self) -> String {
        self.skip_blanks();
        match self.peek() {
            Some('\\') => {
                self.at += 1;
                let name = self.command();
                symbol(&name).map_or(name, |(text, _)| text.to_owned())
            }
            Some('.') => {
                self.at += 1;
                String::new()
            }
            Some(c) => {
                self.at += 1;
                c.to_string()
            }
            None => String::new(),
        }
    }

    fn one(&mut self) -> Option<Node> {
        let letter = self.peek()?;
        self.at += 1;
        Some(match letter {
            '{' => self.row(Stop::Brace),
            '\\' => self.after_backslash(),
            '-' => atom("−", Class::Binary),
            '+' | '*' => atom(if letter == '*' { "∗" } else { "+" }, Class::Binary),
            '=' | '<' | '>' => atom(&letter.to_string(), Class::Relation),
            ',' | ';' => atom(&letter.to_string(), Class::Punct),
            '!' | '?' | ':' | '(' | ')' | '[' | ']' | '|' | '/' | '.' => {
                atom(&letter.to_string(), Class::Upright)
            }
            c if c.is_ascii_digit() => {
                let mut text = c.to_string();
                while self
                    .peek()
                    .is_some_and(|c| c.is_ascii_digit() || (c == '.' && self.digit_after()))
                {
                    text.push(self.letters[self.at]);
                    self.at += 1;
                }
                atom(&text, Class::Number)
            }
            c if c.is_alphabetic() => atom(&c.to_string(), Class::Letter),
            c => atom(&c.to_string(), Class::Upright),
        })
    }

    fn digit_after(&self) -> bool {
        self.letters
            .get(self.at + 1)
            .is_some_and(char::is_ascii_digit)
    }

    #[expect(clippy::too_many_lines, reason = "a table of commands")]
    fn after_backslash(&mut self) -> Node {
        let name = self.command();
        match name.as_str() {
            "frac" | "dfrac" | "tfrac" | "cfrac" => {
                let over = self.argument();
                let under = self.argument();
                Node::Frac(Box::new(over), Box::new(under))
            }
            "binom" | "dbinom" | "tbinom" => {
                let over = self.argument();
                let under = self.argument();
                Node::Fenced {
                    open: "(".to_owned(),
                    body: Box::new(Node::Matrix {
                        rows: vec![vec![over], vec![under]],
                        open: String::new(),
                        close: String::new(),
                        left: false,
                    }),
                    close: ")".to_owned(),
                }
            }
            "sqrt" => {
                self.skip_blanks();
                let index = if self.peek() == Some('[') {
                    self.at += 1;
                    let mut inside = String::new();
                    while let Some(c) = self.peek() {
                        self.at += 1;
                        if c == ']' {
                            break;
                        }
                        inside.push(c);
                    }
                    Some(Box::new(Reader::new(&inside).row(Stop::End)))
                } else {
                    None
                };
                Node::Sqrt {
                    body: Box::new(self.argument()),
                    index,
                }
            }
            "left" => {
                let open = self.delimiter();
                let body = self.row(Stop::Right);
                let close = if self.looking_at("right") {
                    self.at += 1;
                    let _ = self.command();
                    self.delimiter()
                } else {
                    String::new()
                };
                Node::Fenced {
                    open,
                    body: Box::new(body),
                    close,
                }
            }
            "big" | "Big" | "bigg" | "Bigg" | "bigl" | "bigr" | "Bigl" | "Bigr" | "biggl"
            | "biggr" => {
                let delimiter = self.delimiter();
                atom(&delimiter, Class::Upright)
            }
            "begin" => self.environment(),
            "text" | "textrm" | "mathrm" | "operatorname" | "textit" | "textbf" | "mbox" => {
                let raw = self.raw();
                atom(&raw, Class::Upright)
            }
            "mathbf" | "boldsymbol" | "bm" | "mathit" | "mathsf" | "mathcal" | "mathscr"
            | "mathfrak" => self.argument(),
            "mathbb" => {
                let raw = self.raw();
                atom(
                    &raw.chars().map(double_struck).collect::<String>(),
                    Class::Upright,
                )
            }
            "vec" | "overrightarrow" => self.accented(Accent::Vec),
            "hat" | "widehat" => self.accented(Accent::Hat),
            "bar" | "overline" => self.accented(Accent::Bar),
            "dot" => self.accented(Accent::Dot),
            "ddot" => self.accented(Accent::Ddot),
            "tilde" | "widetilde" => self.accented(Accent::Tilde),
            "," | "thinspace" => Node::Space(0.17),
            ":" | ">" | "medspace" => Node::Space(0.22),
            ";" | "thickspace" => Node::Space(0.28),
            "!" => Node::Space(-0.1),
            "quad" => Node::Space(1.0),
            "qquad" => Node::Space(2.0),
            " " => Node::Space(0.25),
            "right" | "\\" | "displaystyle" | "textstyle" | "limits" | "nolimits" | "rm" | "it"
            | "bf" => Node::Row(Vec::new()),
            name if FUNCTIONS.contains(&name) => atom(name, Class::Upright),
            name => {
                if let Some((symbol, limits)) = big(name) {
                    Node::Big {
                        symbol: symbol.to_owned(),
                        limits,
                    }
                } else if let Some((text, class)) = symbol(name) {
                    atom(text, class)
                } else {
                    atom(&format!("\\{name}"), Class::Upright)
                }
            }
        }
    }

    fn accented(&mut self, mark: Accent) -> Node {
        Node::Accent {
            mark,
            body: Box::new(self.argument()),
        }
    }

    fn environment(&mut self) -> Node {
        let name = self.raw();
        let (open, close, left) = match name.trim_end_matches('*') {
            "pmatrix" => ("(", ")", false),
            "bmatrix" => ("[", "]", false),
            "Bmatrix" => ("{", "}", false),
            "vmatrix" => ("|", "|", false),
            "Vmatrix" => ("‖", "‖", false),
            "cases" => ("{", "", true),
            "aligned" | "align" | "split" | "gathered" | "array" | "eqnarray" => ("", "", true),
            _ => ("", "", false),
        };
        if name.starts_with("array") {
            let _ = self.raw();
        }
        let mut rows = vec![Vec::new()];
        loop {
            let cell = self.row(Stop::Cell);
            if let Some(row) = rows.last_mut() {
                row.push(cell);
            }
            match self.peek() {
                Some('&') => self.at += 1,
                Some('\\') if self.looking_at("\\") => {
                    self.at += 2;
                    rows.push(Vec::new());
                }
                Some('\\') if self.looking_at("end") => {
                    self.at += 1;
                    let _ = self.command();
                    let _ = self.raw();
                    break;
                }
                _ => break,
            }
        }
        rows.retain(|row| row.iter().any(|cell| !empty(cell)));
        Node::Matrix {
            rows,
            open: open.to_owned(),
            close: close.to_owned(),
            left,
        }
    }
}

fn atom(text: &str, class: Class) -> Node {
    Node::Atom {
        text: text.to_owned(),
        class,
    }
}

fn empty(node: &Node) -> bool {
    match node {
        Node::Row(items) => items.iter().all(empty),
        Node::Space(_) => true,
        _ => false,
    }
}

fn attach(base: Node, up: bool, script: Node) -> Node {
    let script = Some(Box::new(script));
    match base {
        Node::Scripts { base, sup, sub } => {
            if up {
                Node::Scripts {
                    base,
                    sup: script,
                    sub,
                }
            } else {
                Node::Scripts {
                    base,
                    sup,
                    sub: script,
                }
            }
        }
        base => {
            let (sup, sub) = if up { (script, None) } else { (None, script) };
            Node::Scripts {
                base: Box::new(base),
                sup,
                sub,
            }
        }
    }
}

fn read(latex: &str) -> Node {
    Reader::new(latex).row(Stop::End)
}

const SUPERSCRIPTS: [(char, char); 31] = [
    ('0', '⁰'),
    ('1', '¹'),
    ('2', '²'),
    ('3', '³'),
    ('4', '⁴'),
    ('5', '⁵'),
    ('6', '⁶'),
    ('7', '⁷'),
    ('8', '⁸'),
    ('9', '⁹'),
    ('+', '⁺'),
    ('−', '⁻'),
    ('=', '⁼'),
    ('(', '⁽'),
    (')', '⁾'),
    ('n', 'ⁿ'),
    ('i', 'ⁱ'),
    ('x', 'ˣ'),
    ('y', 'ʸ'),
    ('k', 'ᵏ'),
    ('m', 'ᵐ'),
    ('a', 'ᵃ'),
    ('b', 'ᵇ'),
    ('c', 'ᶜ'),
    ('d', 'ᵈ'),
    ('e', 'ᵉ'),
    ('t', 'ᵗ'),
    ('T', 'ᵀ'),
    ('′', '′'),
    ('*', '*'),
    ('∗', '*'),
];

const SUBSCRIPTS: [(char, char); 26] = [
    ('0', '₀'),
    ('1', '₁'),
    ('2', '₂'),
    ('3', '₃'),
    ('4', '₄'),
    ('5', '₅'),
    ('6', '₆'),
    ('7', '₇'),
    ('8', '₈'),
    ('9', '₉'),
    ('+', '₊'),
    ('−', '₋'),
    ('=', '₌'),
    ('(', '₍'),
    (')', '₎'),
    ('a', 'ₐ'),
    ('e', 'ₑ'),
    ('o', 'ₒ'),
    ('x', 'ₓ'),
    ('i', 'ᵢ'),
    ('j', 'ⱼ'),
    ('k', 'ₖ'),
    ('n', 'ₙ'),
    ('m', 'ₘ'),
    ('t', 'ₜ'),
    ('p', 'ₚ'),
];

fn scripted(text: &str, table: &[(char, char)]) -> Option<String> {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| {
            table
                .iter()
                .find(|(plain, _)| *plain == c)
                .map(|(_, up)| *up)
        })
        .collect()
}

#[must_use]
pub fn linear(latex: &str) -> String {
    let mut out = String::new();
    line(&read(latex), &mut out);
    out.trim().to_owned()
}

fn simple(node: &Node) -> bool {
    match node {
        Node::Atom { text, class } => {
            text.chars().count() <= 3 && !matches!(class, Class::Binary | Class::Relation)
        }
        Node::Row(items) => items.len() == 1 && simple(&items[0]),
        Node::Scripts { base, .. } => simple(base),
        Node::Accent { .. } | Node::Fenced { .. } => true,
        _ => false,
    }
}

#[expect(clippy::too_many_lines, reason = "every kind of piece, as a line")]
fn line(node: &Node, out: &mut String) {
    match node {
        Node::Row(items) => {
            let mut integral = false;
            for (at, item) in items.iter().enumerate() {
                let differential = integral
                    && matches!(item, Node::Atom { text, .. } if text == "d")
                    && matches!(
                        items.get(at + 1),
                        Some(Node::Atom {
                            class: Class::Letter,
                            ..
                        })
                    );
                if differential && !out.ends_with(' ') {
                    out.push(' ');
                }
                integral |= is_integral(item);
                let spaced = matches!(
                    item,
                    Node::Atom {
                        class: Class::Relation | Class::Binary,
                        ..
                    }
                ) && at > 0;
                if spaced {
                    out.push(' ');
                }
                line(item, out);
                let big = matches!(item, Node::Big { .. })
                    || matches!(item, Node::Scripts { base, .. } if matches!(**base, Node::Big { .. }));
                if spaced
                    || big
                    || matches!(
                        item,
                        Node::Atom {
                            class: Class::Punct,
                            ..
                        }
                    )
                {
                    out.push(' ');
                }
            }
        }
        Node::Atom { text, .. } => {
            out.push_str(text);
            if text.starts_with('\\') {
                out.push(' ');
            }
        }
        Node::Frac(over, under) => {
            bracketed(over, out);
            out.push('/');
            bracketed(under, out);
        }
        Node::Scripts { base, sup, sub } => {
            line(base, out);
            if let Some(sub) = sub {
                script(sub, &SUBSCRIPTS, '_', out);
            }
            if let Some(sup) = sup {
                script(sup, &SUPERSCRIPTS, '^', out);
            }
        }
        Node::Sqrt { body, index } => {
            match index.as_deref().map(linear_of) {
                Some(three) if three == "3" => out.push('∛'),
                Some(four) if four == "4" => out.push('∜'),
                Some(other) => {
                    out.push_str(&scripted(&other, &SUPERSCRIPTS).unwrap_or(other));
                    out.push('√');
                }
                None => out.push('√'),
            }
            bracketed(body, out);
        }
        Node::Big { symbol, .. } => out.push_str(symbol),
        Node::Fenced { open, body, close } => {
            out.push_str(open);
            line(body, out);
            out.push_str(close);
        }
        Node::Matrix {
            rows, open, close, ..
        } => {
            out.push_str(if open.is_empty() { "[" } else { open });
            for (at, row) in rows.iter().enumerate() {
                if at > 0 {
                    out.push_str("; ");
                }
                for (column, cell) in row.iter().enumerate() {
                    if column > 0 {
                        out.push_str(", ");
                    }
                    line(cell, out);
                }
            }
            out.push_str(if close.is_empty() && open.is_empty() {
                "]"
            } else {
                close
            });
        }
        Node::Accent { mark, body } => {
            let inside = linear_of(body);
            if inside.chars().count() == 1 {
                out.push_str(&inside);
                out.push(mark.combining());
            } else {
                out.push('(');
                out.push_str(&inside);
                out.push(')');
                out.push(mark.combining());
            }
        }
        Node::Space(ems) => {
            if *ems > 0.5 {
                out.push_str("  ");
            } else if *ems > 0.0 {
                out.push(' ');
            }
        }
    }
}

fn is_integral(node: &Node) -> bool {
    match node {
        Node::Big { limits: false, .. } => true,
        Node::Scripts { base, .. } => is_integral(base),
        _ => false,
    }
}

fn linear_of(node: &Node) -> String {
    let mut out = String::new();
    line(node, &mut out);
    out.trim().to_owned()
}

fn bracketed(node: &Node, out: &mut String) {
    if simple(node) {
        line(node, out);
    } else {
        out.push('(');
        out.push_str(&linear_of(node));
        out.push(')');
    }
}

fn script(node: &Node, table: &[(char, char)], mark: char, out: &mut String) {
    let text = linear_of(node);
    if let Some(small) = scripted(&text, table) {
        out.push_str(&small);
    } else if text.chars().count() == 1 {
        out.push(mark);
        out.push_str(&text);
    } else {
        out.push(mark);
        out.push('(');
        out.push_str(&text);
        out.push(')');
    }
}

#[derive(Clone, Debug, Default)]
struct Laid {
    width: f64,
    ascent: f64,
    descent: f64,
    items: Vec<Item>,
}

#[derive(Clone, Debug)]
enum Item {
    Text { x: f64, y: f64, fitted: Fitted },
    Stroke { steps: Vec<PenStep>, width: f64 },
}

impl Laid {
    fn shifted(mut self, dx: f64, dy: f64) -> Vec<Item> {
        self.shift(dx, dy);
        self.items
    }

    fn shift(&mut self, dx: f64, dy: f64) {
        for item in &mut self.items {
            match item {
                Item::Text { x, y, .. } => {
                    *x += dx;
                    *y += dy;
                }
                Item::Stroke { steps, .. } => {
                    for step in steps.iter_mut() {
                        *step = moved(*step, dx, dy);
                    }
                }
            }
        }
    }
}

fn moved(step: PenStep, dx: f64, dy: f64) -> PenStep {
    let m = |(x, y): (f64, f64)| (x + dx, y + dy);
    match step {
        PenStep::Move(p) => PenStep::Move(m(p)),
        PenStep::Line(p) => PenStep::Line(m(p)),
        PenStep::Curve(a, b, c) => PenStep::Curve(m(a), m(b), m(c)),
    }
}

const SMALLEST: f64 = 6.0;

struct Setter<'a, 'b> {
    composer: &'a Composer<'b>,
    style: &'a Style,
    width: f64,
}

impl Setter<'_, '_> {
    fn text(&self, text: &str, size: f64) -> Result<Laid, String> {
        let style = Style {
            size,
            italic: false,
            ..self.style.clone()
        };
        let fitted = self.composer.fit(text, &style, self.width)?;
        Ok(Laid {
            width: fitted.room.widest,
            ascent: size * 0.78,
            descent: size * 0.24,
            items: vec![Item::Text {
                x: 0.0,
                y: 0.0,
                fitted,
            }],
        })
    }

    fn lay(&self, node: &Node, size: f64) -> Result<Laid, String> {
        match node {
            Node::Row(items) => self.row(items, size),
            Node::Atom { text, .. } => self.text(text, size),
            Node::Space(ems) => Ok(Laid {
                width: ems * size,
                ..Laid::default()
            }),
            Node::Frac(over, under) => self.fraction(over, under, size),
            Node::Scripts { base, sup, sub } => {
                self.scripts(base, sup.as_deref(), sub.as_deref(), size)
            }
            Node::Sqrt { body, index } => self.root(body, index.as_deref(), size),
            Node::Big { symbol, .. } => {
                let bigger = if symbol.chars().all(char::is_alphabetic) || symbol.contains(' ') {
                    size
                } else {
                    size * 1.5
                };
                let mut laid = self.text(symbol, bigger)?;
                let lift = (bigger - size) * 0.3;
                laid.shift(0.0, lift);
                laid.ascent -= lift;
                laid.descent += lift;
                Ok(laid)
            }
            Node::Fenced { open, body, close } => self.fenced(open, body, close, size),
            Node::Matrix {
                rows,
                open,
                close,
                left,
            } => self.matrix(rows, (open, close), *left, size),
            Node::Accent { mark, body } => self.accent(*mark, body, size),
        }
    }

    fn row(&self, items: &[Node], size: f64) -> Result<Laid, String> {
        let mut out = Laid::default();
        let mut x = 0.0;
        let mut run = String::new();
        let flush = |run: &mut String, x: &mut f64, out: &mut Laid| -> Result<(), String> {
            if !run.trim().is_empty() {
                let laid = self.text(run.trim_end(), size)?;
                join(out, laid, x);
            }
            run.clear();
            Ok(())
        };
        for (at, item) in items.iter().enumerate() {
            match item {
                Node::Atom { text, class } => {
                    let spaced = matches!(class, Class::Binary | Class::Relation) && at > 0;
                    let room = if *class == Class::Relation {
                        ' '
                    } else {
                        '\u{2009}'
                    };
                    if spaced {
                        run.push(room);
                    }
                    run.push_str(text);
                    if spaced {
                        run.push(room);
                    } else if *class == Class::Punct {
                        run.push(' ');
                    }
                }
                other => {
                    let trailing = run.ends_with(' ') || run.ends_with('\u{2009}');
                    flush(&mut run, &mut x, &mut out)?;
                    if trailing {
                        x += size * 0.22;
                    }
                    let laid = self.lay(other, size)?;
                    join(&mut out, laid, &mut x);
                    if matches!(other, Node::Big { .. } | Node::Scripts { .. }) {
                        x += size * 0.1;
                    }
                }
            }
        }
        flush(&mut run, &mut x, &mut out)?;
        out.width = x;
        Ok(out)
    }

    fn fraction(&self, over: &Node, under: &Node, size: f64) -> Result<Laid, String> {
        let inner = (size * 0.85).max(SMALLEST);
        let (top, bottom) = (self.lay(over, inner)?, self.lay(under, inner)?);
        let axis = size * 0.27;
        let thick = size * 0.06;
        let gap = size * 0.18;
        let width = top.width.max(bottom.width) + size * 0.35;
        let over_y = -(axis + gap + thick / 2.0 + top.descent);
        let under_y = -axis + gap + thick / 2.0 + bottom.ascent;
        let ascent = axis + gap + thick / 2.0 + top.descent + top.ascent;
        let descent = under_y + bottom.descent;
        let mut items = Vec::new();
        let (top_width, bottom_width) = (top.width, bottom.width);
        items.extend(top.shifted((width - top_width) / 2.0, over_y));
        items.extend(bottom.shifted((width - bottom_width) / 2.0, under_y));
        items.push(Item::Stroke {
            steps: shapes::line(size * 0.08, -axis, width - size * 0.08, -axis),
            width: thick,
        });
        Ok(Laid {
            width,
            ascent,
            descent,
            items,
        })
    }

    fn scripts(
        &self,
        base: &Node,
        sup: Option<&Node>,
        sub: Option<&Node>,
        size: f64,
    ) -> Result<Laid, String> {
        let small = (size * 0.7).max(SMALLEST);
        let limits = matches!(base, Node::Big { limits: true, .. });
        let laid_base = self.lay(base, size)?;
        let up = sup.map(|node| self.lay(node, small)).transpose()?;
        let down = sub.map(|node| self.lay(node, small)).transpose()?;
        if limits {
            let width = laid_base
                .width
                .max(up.as_ref().map_or(0.0, |laid| laid.width))
                .max(down.as_ref().map_or(0.0, |laid| laid.width));
            let gap = size * 0.12;
            let mut ascent = laid_base.ascent;
            let mut descent = laid_base.descent;
            let mut items = Vec::new();
            if let Some(up) = up {
                let y = -(laid_base.ascent + gap + up.descent);
                ascent = laid_base.ascent + gap + up.descent + up.ascent;
                let w = up.width;
                items.extend(up.shifted((width - w) / 2.0, y));
            }
            if let Some(down) = down {
                let y = laid_base.descent + gap + down.ascent;
                descent = y + down.descent;
                let w = down.width;
                items.extend(down.shifted((width - w) / 2.0, y));
            }
            let w = laid_base.width;
            items.extend(laid_base.shifted((width - w) / 2.0, 0.0));
            return Ok(Laid {
                width,
                ascent,
                descent,
                items,
            });
        }
        let integral = matches!(base, Node::Big { limits: false, .. });
        let x = laid_base.width + if integral { size * 0.05 } else { size * 0.04 };
        let raise = if integral {
            laid_base.ascent - small * 0.55
        } else {
            (size * 0.42).max(laid_base.ascent - small * 0.6)
        };
        let lower = if integral {
            laid_base.descent + small * 0.1
        } else {
            (size * 0.2).max(laid_base.descent - small * 0.2)
        };
        let mut width = x;
        let mut ascent = laid_base.ascent;
        let mut descent = laid_base.descent;
        let mut items = laid_base.shifted(0.0, 0.0);
        if let Some(up) = up {
            ascent = ascent.max(raise + up.ascent);
            width = width.max(x + up.width);
            items.extend(up.shifted(x, -raise));
        }
        if let Some(down) = down {
            descent = descent.max(lower + down.descent);
            let at = if integral { x - size * 0.2 } else { x };
            width = width.max(at + down.width);
            items.extend(down.shifted(at, lower));
        }
        Ok(Laid {
            width,
            ascent,
            descent,
            items,
        })
    }

    fn root(&self, body: &Node, index: Option<&Node>, size: f64) -> Result<Laid, String> {
        let laid = self.lay(body, size)?;
        let clear = size * 0.14;
        let thick = size * 0.055;
        let sign = size * 0.6;
        let top = -(laid.ascent + clear);
        let bottom = laid.descent;
        let height = bottom - top;
        let width = sign + laid.width + size * 0.15;
        let mut items = Vec::new();
        let mut shift = 0.0;
        let mut ascent = laid.ascent + clear + thick;
        if let Some(index) = index {
            let small = self.lay(index, (size * 0.55).max(SMALLEST))?;
            shift = (small.width - sign * 0.45).max(0.0);
            let y = bottom - height * 0.55 - small.descent;
            ascent = ascent.max(-y + small.ascent);
            items.extend(small.shifted(0.0, y));
        }
        items.push(Item::Stroke {
            steps: vec![
                PenStep::Move((shift, bottom - height * 0.42)),
                PenStep::Line((shift + sign * 0.22, bottom - height * 0.5)),
                PenStep::Line((shift + sign * 0.5, bottom)),
                PenStep::Line((shift + sign * 0.92, top)),
                PenStep::Line((shift + width, top)),
            ],
            width: thick,
        });
        let descent = laid.descent;
        items.extend(laid.shifted(shift + sign, 0.0));
        Ok(Laid {
            width: width + shift,
            ascent,
            descent,
            items,
        })
    }

    fn fenced(&self, open: &str, body: &Node, close: &str, size: f64) -> Result<Laid, String> {
        let inside = self.lay(body, size)?;
        self.around(inside, (open, close), size)
    }

    fn around(&self, inside: Laid, (open, close): (&str, &str), size: f64) -> Result<Laid, String> {
        let height = inside.ascent + inside.descent;
        let tall = height > size * 1.25;
        let mut items = Vec::new();
        let mut x = 0.0;
        let (ascent, descent) = if tall {
            (inside.ascent + size * 0.08, inside.descent + size * 0.08)
        } else {
            (
                inside.ascent.max(size * 0.78),
                inside.descent.max(size * 0.24),
            )
        };
        let place =
            |which: &str, x: &mut f64, items: &mut Vec<Item>, left: bool| -> Result<(), String> {
                if which.is_empty() {
                    return Ok(());
                }
                if tall {
                    let wide = size * 0.42;
                    items.push(Item::Stroke {
                        steps: delimiter(which, *x, (-ascent, descent), wide, left),
                        width: size * 0.065,
                    });
                    *x += wide;
                } else {
                    let laid = self.text(which, size)?;
                    let w = laid.width;
                    items.extend(laid.shifted(*x, 0.0));
                    *x += w;
                }
                Ok(())
            };
        place(open, &mut x, &mut items, true)?;
        let w = inside.width;
        items.extend(inside.shifted(x + size * 0.06, 0.0));
        x += w + size * 0.12;
        place(close, &mut x, &mut items, false)?;
        Ok(Laid {
            width: x,
            ascent,
            descent,
            items,
        })
    }

    fn matrix(
        &self,
        rows: &[Vec<Node>],
        (open, close): (&str, &str),
        left: bool,
        size: f64,
    ) -> Result<Laid, String> {
        let mut cells = Vec::new();
        let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
        let mut widths = vec![0.0_f64; columns];
        let mut heights = Vec::new();
        for row in rows {
            let mut laid_row = Vec::new();
            let (mut up, mut down) = (size * 0.78, size * 0.24);
            for (at, cell) in row.iter().enumerate() {
                let laid = self.lay(cell, size)?;
                widths[at] = widths[at].max(laid.width);
                up = up.max(laid.ascent);
                down = down.max(laid.descent);
                laid_row.push(laid);
            }
            heights.push((up, down));
            cells.push(laid_row);
        }
        let column_gap = if left { size * 1.0 } else { size * 0.9 };
        let row_gap = size * 0.3;
        let total: f64 = heights.iter().map(|(up, down)| up + down).sum::<f64>()
            + row_gap * count(heights.len().saturating_sub(1));
        let axis = size * 0.27;
        let mut y = -(total / 2.0) - axis;
        let mut items = Vec::new();
        let width: f64 = widths.iter().sum::<f64>() + column_gap * count(columns.saturating_sub(1));
        for (row, (up, down)) in cells.into_iter().zip(&heights) {
            y += up;
            let mut x = 0.0;
            for (at, laid) in row.into_iter().enumerate() {
                let w = laid.width;
                let offset = if left { 0.0 } else { (widths[at] - w) / 2.0 };
                items.extend(laid.shifted(x + offset, y));
                x += widths[at] + column_gap;
            }
            y += down + row_gap;
        }
        let inside = Laid {
            width,
            ascent: total / 2.0 + axis,
            descent: total / 2.0 - axis,
            items,
        };
        self.around(inside, (open, close), size)
    }

    fn accent(&self, mark: Accent, body: &Node, size: f64) -> Result<Laid, String> {
        let laid = self.lay(body, size)?;
        let lift = laid.ascent + size * 0.08;
        let w = laid.width;
        let (x0, x1) = (w * 0.1, w * 0.9 + size * 0.05);
        let y = -lift - size * 0.08;
        let (steps, extra) = match mark {
            Accent::Vec => (
                vec![
                    PenStep::Move((x0, y)),
                    PenStep::Line((x1, y)),
                    PenStep::Move((x1 - size * 0.15, y - size * 0.1)),
                    PenStep::Line((x1, y)),
                    PenStep::Line((x1 - size * 0.15, y + size * 0.1)),
                ],
                size * 0.25,
            ),
            Accent::Bar => (shapes::line(x0, y, x1, y), size * 0.15),
            Accent::Hat => {
                let mid = w / 2.0;
                (
                    vec![
                        PenStep::Move((mid - size * 0.2, y + size * 0.05)),
                        PenStep::Line((mid, y - size * 0.1)),
                        PenStep::Line((mid + size * 0.2, y + size * 0.05)),
                    ],
                    size * 0.22,
                )
            }
            Accent::Tilde => {
                let mid = w / 2.0;
                (
                    vec![
                        PenStep::Move((mid - size * 0.22, y + size * 0.03)),
                        PenStep::Curve(
                            (mid - size * 0.12, y - size * 0.1),
                            (mid - size * 0.05, y + size * 0.1),
                            (mid + size * 0.05, y),
                        ),
                        PenStep::Curve(
                            (mid + size * 0.12, y - size * 0.1),
                            (mid + size * 0.18, y - size * 0.06),
                            (mid + size * 0.22, y - size * 0.03),
                        ),
                    ],
                    size * 0.2,
                )
            }
            Accent::Dot | Accent::Ddot => {
                let mid = w / 2.0;
                let r = size * 0.05;
                let mut steps = Vec::new();
                if mark == Accent::Dot {
                    steps.extend(shapes::circle(mid, y, r));
                } else {
                    steps.extend(shapes::circle(mid - size * 0.12, y, r));
                    steps.extend(shapes::circle(mid + size * 0.12, y, r));
                }
                (steps, size * 0.15)
            }
        };
        let ascent = laid.ascent + extra + size * 0.08;
        let descent = laid.descent;
        let mut items = laid.shifted(0.0, 0.0);
        let width = if matches!(mark, Accent::Dot | Accent::Ddot) {
            size * 0.12
        } else {
            size * 0.06
        };
        items.push(Item::Stroke { steps, width });
        Ok(Laid {
            width: w.max(x1),
            ascent,
            descent,
            items,
        })
    }
}

fn count(n: usize) -> f64 {
    f64::from(u32::try_from(n).unwrap_or(u32::MAX))
}

fn join(out: &mut Laid, laid: Laid, x: &mut f64) {
    out.ascent = out.ascent.max(laid.ascent);
    out.descent = out.descent.max(laid.descent);
    let width = laid.width;
    out.items.extend(laid.shifted(*x, 0.0));
    *x += width;
}

fn delimiter(
    which: &str,
    x: f64,
    (top, bottom): (f64, f64),
    wide: f64,
    left: bool,
) -> Vec<PenStep> {
    let height = bottom - top;
    let mid = f64::midpoint(top, bottom);
    let (near, far) = if left {
        (x + wide * 0.75, x + wide * 0.25)
    } else {
        (x + wide * 0.25, x + wide * 0.75)
    };
    match which {
        "(" | ")" => vec![
            PenStep::Move((near, top)),
            PenStep::Curve(
                (far - (near - far) * 0.15, top + height * 0.25),
                (far - (near - far) * 0.15, bottom - height * 0.25),
                (near, bottom),
            ),
        ],
        "[" | "]" | "⌈" | "⌉" | "⌊" | "⌋" => vec![
            PenStep::Move((near, top)),
            PenStep::Line((far, top)),
            PenStep::Line((far, bottom)),
            PenStep::Line((near, bottom)),
        ],
        "{" | "}" => {
            let beak = if left { x } else { x + wide };
            let spine = f64::midpoint(near, far);
            vec![
                PenStep::Move((near, top)),
                PenStep::Curve((spine, top), (spine, top), (spine, top + height * 0.12)),
                PenStep::Line((spine, mid - height * 0.1)),
                PenStep::Curve((spine, mid - height * 0.02), (spine, mid), (beak, mid)),
                PenStep::Curve(
                    (spine, mid),
                    (spine, mid + height * 0.02),
                    (spine, mid + height * 0.1),
                ),
                PenStep::Line((spine, bottom - height * 0.12)),
                PenStep::Curve((spine, bottom), (spine, bottom), (near, bottom)),
            ]
        }
        "‖" => {
            let one = x + wide * 0.35;
            let two = x + wide * 0.65;
            let mut steps = shapes::line(one, top, one, bottom);
            steps.extend(shapes::line(two, top, two, bottom));
            steps
        }
        "⟨" | "⟩" => vec![
            PenStep::Move((near, top)),
            PenStep::Line((far, mid)),
            PenStep::Line((near, bottom)),
        ],
        _ => {
            let middle = x + wide / 2.0;
            shapes::line(middle, top, middle, bottom)
        }
    }
}

pub(crate) fn set_out(
    composer: &mut Composer<'_>,
    latex: &str,
    style: &Style,
    (left, right): (f64, f64),
    space_before: f64,
) -> Result<(), String> {
    let size = style.size;
    let node = read(latex);
    let laid = {
        let setter = Setter {
            composer,
            style,
            width: right - left,
        };
        setter.lay(&node, size)?
    };
    let pad = size * 0.3;
    let height = laid.ascent + laid.descent + 2.0 * pad;
    let top = composer.place(space_before, height);
    let x = left + ((right - left) - laid.width).max(0.0) / 2.0;
    let baseline = top + pad + laid.ascent;
    let colour: Colour = style.colour.unwrap_or([0.0; 3]);
    for item in laid.shifted(x, baseline) {
        match item {
            Item::Text { x, y, fitted } => {
                let size = fitted.style.size;
                let widest = fitted.room.widest;
                composer.text((x, y - size), widest, fitted);
            }
            Item::Stroke { steps, width } => {
                composer.shape(steps, Some((colour, width)), None);
            }
        }
    }
    composer.top = top + height;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mathematics_in_a_line_reads_as_it_is_meant() {
        assert_eq!(linear(r"x^2 + y^2 = r^2"), "x² + y² = r²");
        assert_eq!(linear(r"\alpha \leq \beta"), "α ≤ β");
        assert_eq!(linear(r"\frac{a}{b}"), "a/b");
        assert_eq!(linear(r"\frac{a+b}{2}"), "(a + b)/2");
        assert_eq!(linear(r"\sqrt{x}"), "√x");
        assert_eq!(linear(r"\sum_{i=1}^{n} i"), "∑ᵢ₌₁ⁿ i");
        assert_eq!(linear(r"e^{-x^2}"), "e^(−x²)");
        assert_eq!(linear(r"\nabla f"), "∇f");
        assert_eq!(linear(r"\lim_{x \to 0}"), "lim_(x → 0)");
        assert_eq!(linear(r"\mathbb{R}^n"), "ℝⁿ");
        assert_eq!(linear(r"\det(A - \lambda I) = 0"), "det(A − λI) = 0");
        assert_eq!(
            linear(r"\begin{pmatrix} 1 & 2 \\ 3 & 4 \end{pmatrix}"),
            "(1, 2; 3, 4)"
        );
        assert_eq!(linear(r"\unknown x"), r"\unknown x");
        assert!(!linear(r"\frac{1}{2}").contains("frac"));
    }

    #[test]
    fn a_fraction_is_stacked_on_its_bar() {
        let node = read(r"\frac{1}{2}");
        let Node::Row(items) = node else { panic!() };
        assert!(matches!(items[0], Node::Frac(..)));
    }
}
