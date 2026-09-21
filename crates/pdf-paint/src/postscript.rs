const MAX_STACK: usize = 100;
const MAX_STEPS: usize = 100_000;
const MAX_DEPTH: usize = 32;

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Number(f64),
    Operator(Operator),
    If(Vec<Node>),
    IfElse(Vec<Node>, Vec<Node>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operator {
    Abs,
    Add,
    Atan,
    Ceiling,
    Cos,
    Cvi,
    Cvr,
    Div,
    Exp,
    Floor,
    Idiv,
    Ln,
    Log,
    Mod,
    Mul,
    Neg,
    Round,
    Sin,
    Sqrt,
    Sub,
    Truncate,
    And,
    Bitshift,
    Eq,
    False,
    Ge,
    Gt,
    Le,
    Lt,
    Ne,
    Not,
    Or,
    True,
    Xor,
    Copy,
    Dup,
    Exch,
    Index,
    Pop,
    Roll,
}

pub fn parse(source: &[u8]) -> Result<Vec<Node>, PostScriptError> {
    let mut tokens = Tokens { source, cursor: 0 };
    tokens.skip_space();
    if tokens.take() != Some(b'{') {
        return Err(PostScriptError::MissingProgram);
    }
    let program = parse_procedure(&mut tokens, 0)?;
    tokens.skip_space();
    if tokens.cursor < source.len() {
        return Err(PostScriptError::TrailingData);
    }
    Ok(program)
}

fn parse_procedure(tokens: &mut Tokens<'_>, depth: usize) -> Result<Vec<Node>, PostScriptError> {
    if depth > MAX_DEPTH {
        return Err(PostScriptError::TooDeep);
    }
    let mut nodes = Vec::new();
    let mut pending: Vec<Vec<Node>> = Vec::new();
    loop {
        tokens.skip_space();
        match tokens.peek() {
            None => return Err(PostScriptError::Unbalanced),
            Some(b'}') => {
                tokens.take();
                if pending.is_empty() {
                    return Ok(nodes);
                }
                return Err(PostScriptError::DanglingProcedure);
            }
            Some(b'{') => {
                tokens.take();
                pending.push(parse_procedure(tokens, depth + 1)?);
                if pending.len() > 2 {
                    return Err(PostScriptError::DanglingProcedure);
                }
            }
            Some(_) => {
                let word = tokens.word();
                if word.is_empty() {
                    return Err(PostScriptError::UnknownToken);
                }
                match word.as_slice() {
                    b"if" => {
                        let procedure = pending.pop().ok_or(PostScriptError::DanglingProcedure)?;
                        if !pending.is_empty() {
                            return Err(PostScriptError::DanglingProcedure);
                        }
                        nodes.push(Node::If(procedure));
                    }
                    b"ifelse" => {
                        if pending.len() != 2 {
                            return Err(PostScriptError::DanglingProcedure);
                        }
                        let otherwise = pending.pop().expect("checked");
                        let then = pending.pop().expect("checked");
                        nodes.push(Node::IfElse(then, otherwise));
                    }
                    _ => {
                        if !pending.is_empty() {
                            return Err(PostScriptError::DanglingProcedure);
                        }
                        nodes.push(parse_word(&word)?);
                    }
                }
            }
        }
    }
}

fn parse_word(word: &[u8]) -> Result<Node, PostScriptError> {
    let operator = match word {
        b"abs" => Operator::Abs,
        b"add" => Operator::Add,
        b"atan" => Operator::Atan,
        b"ceiling" => Operator::Ceiling,
        b"cos" => Operator::Cos,
        b"cvi" => Operator::Cvi,
        b"cvr" => Operator::Cvr,
        b"div" => Operator::Div,
        b"exp" => Operator::Exp,
        b"floor" => Operator::Floor,
        b"idiv" => Operator::Idiv,
        b"ln" => Operator::Ln,
        b"log" => Operator::Log,
        b"mod" => Operator::Mod,
        b"mul" => Operator::Mul,
        b"neg" => Operator::Neg,
        b"round" => Operator::Round,
        b"sin" => Operator::Sin,
        b"sqrt" => Operator::Sqrt,
        b"sub" => Operator::Sub,
        b"truncate" => Operator::Truncate,
        b"and" => Operator::And,
        b"bitshift" => Operator::Bitshift,
        b"eq" => Operator::Eq,
        b"false" => Operator::False,
        b"ge" => Operator::Ge,
        b"gt" => Operator::Gt,
        b"le" => Operator::Le,
        b"lt" => Operator::Lt,
        b"ne" => Operator::Ne,
        b"not" => Operator::Not,
        b"or" => Operator::Or,
        b"true" => Operator::True,
        b"xor" => Operator::Xor,
        b"copy" => Operator::Copy,
        b"dup" => Operator::Dup,
        b"exch" => Operator::Exch,
        b"index" => Operator::Index,
        b"pop" => Operator::Pop,
        b"roll" => Operator::Roll,
        _ => {
            let text = std::str::from_utf8(word).map_err(|_| PostScriptError::UnknownToken)?;
            let value = text
                .parse::<f64>()
                .map_err(|_| PostScriptError::UnknownToken)?;
            if !value.is_finite() {
                return Err(PostScriptError::UnknownToken);
            }
            return Ok(Node::Number(value));
        }
    };
    Ok(Node::Operator(operator))
}

struct Tokens<'a> {
    source: &'a [u8],
    cursor: usize,
}

impl Tokens<'_> {
    fn peek(&self) -> Option<u8> {
        self.source.get(self.cursor).copied()
    }

    fn take(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.cursor += 1;
        Some(byte)
    }

    fn skip_space(&mut self) {
        while let Some(byte) = self.peek() {
            if byte == b'%' {
                while self
                    .peek()
                    .is_some_and(|byte| byte != b'\n' && byte != b'\r')
                {
                    self.cursor += 1;
                }
            } else if byte.is_ascii_whitespace() {
                self.cursor += 1;
            } else {
                break;
            }
        }
    }

    fn word(&mut self) -> Vec<u8> {
        let start = self.cursor;
        while self
            .peek()
            .is_some_and(|byte| !byte.is_ascii_whitespace() && byte != b'{' && byte != b'}')
        {
            self.cursor += 1;
        }
        self.source[start..self.cursor].to_vec()
    }
}

#[must_use]
pub fn evaluate(program: &[Node], inputs: &[f64], outputs: usize) -> Option<Vec<f64>> {
    let mut stack: Vec<f64> = inputs.to_vec();
    let mut steps = 0_usize;
    run(program, &mut stack, &mut steps)?;
    if stack.len() < outputs {
        return None;
    }
    Some(stack.split_off(stack.len() - outputs))
}

fn run(program: &[Node], stack: &mut Vec<f64>, steps: &mut usize) -> Option<()> {
    for node in program {
        *steps += 1;
        if *steps > MAX_STEPS || stack.len() > MAX_STACK {
            return None;
        }
        match node {
            Node::Number(value) => stack.push(*value),
            Node::If(body) => {
                let condition = stack.pop()?;
                if is_true(condition) {
                    run(body, stack, steps)?;
                }
            }
            Node::IfElse(then, otherwise) => {
                let condition = stack.pop()?;
                if is_true(condition) {
                    run(then, stack, steps)?;
                } else {
                    run(otherwise, stack, steps)?;
                }
            }
            Node::Operator(operator) => apply(*operator, stack)?,
        }
    }
    Some(())
}

fn is_true(value: f64) -> bool {
    value != 0.0 && !value.is_nan()
}

fn boolean(value: bool) -> f64 {
    if value { 1.0 } else { 0.0 }
}

#[allow(clippy::too_many_lines)]
fn apply(operator: Operator, stack: &mut Vec<f64>) -> Option<()> {
    match operator {
        Operator::True => stack.push(1.0),
        Operator::False => stack.push(0.0),
        Operator::Abs => unary(stack, f64::abs)?,
        Operator::Neg => unary(stack, |value| -value)?,
        Operator::Ceiling => unary(stack, f64::ceil)?,
        Operator::Floor => unary(stack, f64::floor)?,
        Operator::Round => unary(stack, f64::round)?,

        Operator::Sqrt => unary(stack, |value| value.max(0.0).sqrt())?,
        Operator::Sin => unary(stack, |value| value.to_radians().sin())?,
        Operator::Cos => unary(stack, |value| value.to_radians().cos())?,
        Operator::Ln => unary(stack, |value| if value > 0.0 { value.ln() } else { 0.0 })?,
        Operator::Log => unary(stack, |value| if value > 0.0 { value.log10() } else { 0.0 })?,
        Operator::Cvi | Operator::Truncate => unary(stack, f64::trunc)?,
        Operator::Cvr => unary(stack, |value| value)?,
        Operator::Not => {
            let value = stack.pop()?;
            let boolean_shaped = value.total_cmp(&0.0).is_eq() || value.total_cmp(&1.0).is_eq();
            stack.push(if is_integer(value) && !boolean_shaped {
                from_int(!to_int(value)?)
            } else {
                boolean(!is_true(value))
            });
        }
        Operator::Add => binary(stack, |left, right| Some(left + right))?,
        Operator::Sub => binary(stack, |left, right| Some(left - right))?,
        Operator::Mul => binary(stack, |left, right| Some(left * right))?,
        Operator::Div => binary(stack, |left, right| (right != 0.0).then_some(left / right))?,
        Operator::Idiv => binary(stack, |left, right| {
            let (left, right) = (to_int(left)?, to_int(right)?);
            (right != 0).then(|| from_int(left / right))
        })?,
        Operator::Mod => binary(stack, |left, right| {
            let (left, right) = (to_int(left)?, to_int(right)?);
            (right != 0).then(|| from_int(left % right))
        })?,
        Operator::Exp => binary(stack, |left, right| Some(left.powf(right)))?,
        Operator::Atan => binary(stack, |left, right| {
            let degrees = left.atan2(right).to_degrees();
            Some(if degrees < 0.0 {
                degrees + 360.0
            } else {
                degrees
            })
        })?,
        Operator::Eq => binary(stack, |left, right| {
            Some(boolean(left.total_cmp(&right).is_eq()))
        })?,
        Operator::Ne => binary(stack, |left, right| {
            Some(boolean(left.total_cmp(&right).is_ne()))
        })?,
        Operator::Gt => binary(stack, |left, right| Some(boolean(left > right)))?,
        Operator::Ge => binary(stack, |left, right| Some(boolean(left >= right)))?,
        Operator::Lt => binary(stack, |left, right| Some(boolean(left < right)))?,
        Operator::Le => binary(stack, |left, right| Some(boolean(left <= right)))?,
        Operator::And => bitwise(
            stack,
            |left, right| left & right,
            |left, right| left && right,
        )?,
        Operator::Or => bitwise(
            stack,
            |left, right| left | right,
            |left, right| left || right,
        )?,
        Operator::Xor => bitwise(
            stack,
            |left, right| left ^ right,
            |left, right| left != right,
        )?,
        Operator::Bitshift => binary(stack, |left, right| {
            let (value, shift) = (to_int(left)?, to_int(right)?);
            let shift = i32::try_from(shift).ok()?;
            Some(from_int(if shift >= 0 {
                value.checked_shl(u32::try_from(shift).ok()?).unwrap_or(0)
            } else {
                value.checked_shr(u32::try_from(-shift).ok()?).unwrap_or(0)
            }))
        })?,
        Operator::Pop => {
            stack.pop()?;
        }
        Operator::Dup => {
            let value = *stack.last()?;
            stack.push(value);
        }
        Operator::Exch => {
            let length = stack.len();
            if length < 2 {
                return None;
            }
            stack.swap(length - 1, length - 2);
        }
        Operator::Copy => {
            let count = to_index(stack.pop()?)?;
            if count > stack.len() || stack.len() + count > MAX_STACK {
                return None;
            }
            let start = stack.len() - count;
            for index in 0..count {
                stack.push(stack[start + index]);
            }
        }
        Operator::Index => {
            let depth = to_index(stack.pop()?)?;
            let value = *stack.get(stack.len().checked_sub(depth + 1)?)?;
            stack.push(value);
        }
        Operator::Roll => {
            let shift = to_int(stack.pop()?)?;
            let count = to_index(stack.pop()?)?;
            if count > stack.len() {
                return None;
            }
            if count > 0 {
                let start = stack.len() - count;
                let length = i64::try_from(count).ok()?;
                let shift = shift.rem_euclid(length);
                let shift = usize::try_from(shift).ok()?;
                stack[start..].rotate_right(shift);
            }
        }
    }
    Some(())
}

fn unary(stack: &mut Vec<f64>, operation: impl Fn(f64) -> f64) -> Option<()> {
    let value = stack.pop()?;
    stack.push(operation(value));
    Some(())
}

fn binary(stack: &mut Vec<f64>, operation: impl Fn(f64, f64) -> Option<f64>) -> Option<()> {
    let right = stack.pop()?;
    let left = stack.pop()?;
    stack.push(operation(left, right)?);
    Some(())
}

fn bitwise(
    stack: &mut Vec<f64>,
    integer: impl Fn(i64, i64) -> i64,
    logical: impl Fn(bool, bool) -> bool,
) -> Option<()> {
    binary(stack, |left, right| {
        let booleans = [left, right]
            .iter()
            .all(|value| value.total_cmp(&0.0).is_eq() || value.total_cmp(&1.0).is_eq());
        if booleans {
            Some(boolean(logical(is_true(left), is_true(right))))
        } else {
            Some(from_int(integer(to_int(left)?, to_int(right)?)))
        }
    })
}

fn is_integer(value: f64) -> bool {
    value.is_finite() && value.trunc().total_cmp(&value).is_eq()
}

fn to_int(value: f64) -> Option<i64> {
    if !value.is_finite() || value.abs() > 2_147_483_647.0 {
        return None;
    }
    let truncated = value.trunc();
    let mut low = -2_147_483_648_i64;
    let mut high = 2_147_483_647_i64;
    while low < high {
        let middle = low + (high - low + 1) / 2;
        #[allow(clippy::cast_precision_loss)]
        let as_float = middle as f64;
        if as_float <= truncated {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    Some(low)
}

fn from_int(value: i64) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let out = value as f64;
    out
}

fn to_index(value: f64) -> Option<usize> {
    let integer = to_int(value)?;
    usize::try_from(integer)
        .ok()
        .filter(|count| *count <= MAX_STACK)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PostScriptError {
    MissingProgram,
    Unbalanced,
    TrailingData,
    UnknownToken,
    DanglingProcedure,
    TooDeep,
}

impl std::fmt::Display for PostScriptError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::MissingProgram => "type 4 function has no outer procedure",
            Self::Unbalanced => "type 4 function has an unbalanced procedure",
            Self::TrailingData => "type 4 function has data after its program",
            Self::UnknownToken => "type 4 function uses an unknown token",
            Self::DanglingProcedure => "type 4 function has a procedure no conditional consumes",
            Self::TooDeep => "type 4 function nests procedures too deeply",
        })
    }
}

impl std::error::Error for PostScriptError {}

#[cfg(test)]
mod tests {
    use super::{PostScriptError, evaluate, parse};

    fn run(program: &str, inputs: &[f64], outputs: usize) -> Option<Vec<f64>> {
        evaluate(
            &parse(program.as_bytes()).expect("valid program"),
            inputs,
            outputs,
        )
    }

    #[test]
    fn evaluates_arithmetic_and_stack_operators() {
        assert_eq!(run("{ dup dup }", &[0.25], 3), Some(vec![0.25, 0.25, 0.25]));
        assert_eq!(run("{ 2 mul }", &[3.0], 1), Some(vec![6.0]));
        assert_eq!(run("{ sub }", &[10.0, 4.0], 1), Some(vec![6.0]));
        assert_eq!(run("{ exch }", &[1.0, 2.0], 2), Some(vec![2.0, 1.0]));
        assert_eq!(run("{ pop }", &[1.0, 2.0], 1), Some(vec![1.0]));
        assert_eq!(
            run("{ 1 index }", &[7.0, 8.0], 3),
            Some(vec![7.0, 8.0, 7.0])
        );
        assert_eq!(
            run("{ 3 1 roll }", &[1.0, 2.0, 3.0], 3),
            Some(vec![3.0, 1.0, 2.0])
        );
        assert_eq!(
            run("{ 2 copy }", &[1.0, 2.0], 4),
            Some(vec![1.0, 2.0, 1.0, 2.0])
        );
        assert_eq!(run("{ sin }", &[90.0], 1), Some(vec![1.0]));
        let cosine = run("{ 90 cos }", &[], 1).expect("cos");
        assert!(cosine[0].abs() < 1e-9);
        assert_eq!(run("{ 5 2 idiv }", &[], 1), Some(vec![2.0]));
        assert_eq!(run("{ 5 2 mod }", &[], 1), Some(vec![1.0]));
    }

    #[test]
    fn evaluates_conditionals() {
        assert_eq!(
            run("{ 0.5 gt { 1 } { 0 } ifelse }", &[0.75], 1),
            Some(vec![1.0])
        );
        assert_eq!(
            run("{ 0.5 gt { 1 } { 0 } ifelse }", &[0.25], 1),
            Some(vec![0.0])
        );
        assert_eq!(run("{ 0 gt { 42 } if }", &[1.0], 1), Some(vec![42.0]));
        assert_eq!(
            run("{ dup 0 gt { pop 42 } if }", &[-1.0], 1),
            Some(vec![-1.0])
        );
    }

    #[test]
    fn a_program_that_cannot_run_produces_nothing() {
        assert_eq!(run("{ add }", &[1.0], 1), None);
        assert_eq!(run("{ 1 0 div }", &[], 1), None);
        assert_eq!(run("{ pop }", &[1.0], 1), None);
    }

    #[test]
    fn malformed_programs_fail_closed() {
        assert_eq!(
            parse(b"1 2 add").unwrap_err(),
            PostScriptError::MissingProgram
        );
        assert_eq!(
            parse(b"{ 1 2 add").unwrap_err(),
            PostScriptError::Unbalanced
        );
        assert_eq!(
            parse(b"{ } trailing").unwrap_err(),
            PostScriptError::TrailingData
        );
        assert_eq!(
            parse(b"{ frobnicate }").unwrap_err(),
            PostScriptError::UnknownToken
        );
        assert_eq!(
            parse(b"{ { 1 } }").unwrap_err(),
            PostScriptError::DanglingProcedure
        );
    }
}
