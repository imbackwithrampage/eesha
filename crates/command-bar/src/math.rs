// The bar's calculator. Strict on purpose: the whole input has to be
// mathematical and contain an operation, so a search is never answered as a sum.
//
// Ported from reference/vorssaint-utils/Sources/Vorssaint/Services/CommandBar/CommandBarMath.swift.

/// How this PC writes numbers (Settings › Time & language › Region).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Separators {
    pub decimal: char,
    pub grouping: char,
}

impl Default for Separators {
    fn default() -> Self {
        Self { decimal: '.', grouping: ',' }
    }
}

impl Separators {
    /// Whichever of "." and "," is not the decimal point: both work everywhere.
    fn alternate(&self) -> char {
        if matches!(self.grouping, '.' | ',') && self.grouping != self.decimal {
            self.grouping
        } else if self.decimal == ',' {
            '.'
        } else {
            ','
        }
    }

    /// A space or an apostrophe between thousands only ever groups.
    fn grouping_only(&self) -> Option<char> {
        (!matches!(self.grouping, '.' | ',') && self.grouping != self.decimal).then_some(self.grouping)
    }

    fn part_of_number(&self, c: char) -> bool {
        c.is_numeric() || c == self.decimal || c == self.alternate() || Some(c) == self.grouping_only()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    /// The value, written the way this PC writes numbers.
    pub text: String,
    pub value: f64,
    /// Closing brackets the expression was missing, supplied to work it out.
    pub closing: String,
}

/// Works out a complete expression. Trigonometry is in radians.
pub fn evaluate(input: &str, sep: Separators) -> Option<Answer> {
    let expression = without_trailing_equals(input)?;
    if expression.chars().count() > 120 || looks_like_date_or_time(&expression) {
        return None;
    }
    let mut tokens = tokenize(&expression, sep)?;
    if !is_calculation(&tokens) {
        return None;
    }
    let closings = virtual_closings(&tokens)?;
    tokens.extend(closings.iter().map(|c| Token::Right(*c)));
    let mut parser = Parser { tokens: &tokens, at: 0, depth: 0 };
    let result = parser.expression()?;
    if parser.at < tokens.len() {
        return None;
    }
    let value = significant(result.value);
    Some(Answer { text: format(value, sep)?, value, closing: closings.iter().map(|c| if *c == '(' { ')' } else { ']' }).collect() })
}

/// The answer as something to go on calculating with: no grouping, and a
/// negative value in brackets so a power after it means what it says.
pub fn reusable(answer: &Answer, sep: Separators) -> String {
    let plain = format!("{}", if answer.value == 0.0 { 0.0 } else { answer.value }).replace('.', &sep.decimal.to_string());
    if answer.value < 0.0 { format!("({plain})") } else { plain }
}

/// Ten significant digits: 0.1 + 0.2 reads as 0.3.
fn significant(value: f64) -> f64 {
    if value == 0.0 || !value.is_finite() {
        return value;
    }
    let factor = 10f64.powf(9.0 - value.abs().log10().floor());
    if !factor.is_finite() || factor == 0.0 {
        return value;
    }
    let scaled = (value * factor).round();
    if scaled.is_finite() { scaled / factor } else { value }
}

/// Grouped thousands and at most `digits` decimals, without trailing zeros.
pub fn grouped(value: f64, digits: usize, sep: Separators) -> String {
    let fixed = format!("{:.*}", digits, value.abs());
    let (whole, fraction) = fixed.split_once('.').unwrap_or((&fixed, ""));
    let fraction = fraction.trim_end_matches('0');
    let mut out = String::new();
    for (i, digit) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i) % 3 == 0 {
            out.push(sep.grouping);
        }
        out.push(digit);
    }
    if !fraction.is_empty() {
        out.push(sep.decimal);
        out.push_str(fraction);
    }
    // -0 is an answer no one asked for.
    if value < 0.0 && out.chars().any(|c| c.is_ascii_digit() && c != '0') { format!("-{out}") } else { out }
}

/// A number as this PC writes it. Plain digits would print a billionth as "0"
/// and a huge product as a wall of zeros, so those are written with an exponent.
pub fn format(value: f64, sep: Separators) -> Option<String> {
    let value = significant(value);
    if !value.is_finite() {
        return None;
    }
    let size = value.abs();
    if value != 0.0 && !(1e-6..1e12).contains(&size) {
        let written = format!("{:.7e}", value);
        let (mantissa, exponent) = written.split_once('e')?;
        let mantissa = mantissa.trim_end_matches('0').trim_end_matches('.');
        return Some(format!("{}e{exponent}", mantissa.replace('.', &sep.decimal.to_string())));
    }
    Some(grouped(value, 8, sep))
}

/// "2026-07-27", "27/07/2026", "10:30": groups of digits held together by one
/// kind of separator, with nothing else around them.
fn looks_like_date_or_time(input: &str) -> bool {
    for separator in ['-', '/', ':'] {
        let parts: Vec<&str> = input.split(separator).collect();
        if !(2..=3).contains(&parts.len()) {
            continue;
        }
        // A date is written closed up: "100 - 20 - 30" is a sum, not a day.
        let digits = |part: &&str| {
            let part = if separator == ':' { part.trim() } else { *part };
            !part.is_empty() && part.chars().count() <= 4 && part.chars().all(|c| c.is_numeric())
        };
        // Two plain numbers around a minus or a slash really can be a sum.
        if parts.iter().all(digits) && (separator == ':' || parts.len() == 3) {
            return true;
        }
    }
    false
}

/// One trailing "=" is fine; an equation is not a calculation.
fn without_trailing_equals(input: &str) -> Option<String> {
    let trimmed = input.trim();
    let expression = match trimmed.find('=') {
        None => trimmed,
        Some(at) if at == trimmed.len() - 1 => trimmed[..at].trim(),
        Some(_) => return None,
    };
    (!expression.is_empty()).then(|| expression.to_string())
}

// ── Tokens ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
enum Token {
    Number(f64),
    Constant(f64),
    Function(Function),
    Plus,
    Minus,
    Times,
    Divide,
    Power,
    Percent,
    Of,
    /// '(' or '['.
    Left(char),
    Right(char),
}

impl Token {
    fn ends_product(&self) -> bool {
        matches!(self, Token::Number(_) | Token::Constant(_) | Token::Right(_))
    }
    fn starts_product(&self) -> bool {
        matches!(self, Token::Constant(_) | Token::Function(_) | Token::Left(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Function {
    Sqrt,
    Abs,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Ln,
    Log,
    Exp,
    Floor,
    Ceil,
    Round,
}

impl Function {
    fn named(word: &str) -> Option<Self> {
        Some(match word {
            "sqrt" => Self::Sqrt,
            "abs" => Self::Abs,
            "sin" => Self::Sin,
            "cos" => Self::Cos,
            "tan" => Self::Tan,
            "asin" => Self::Asin,
            "acos" => Self::Acos,
            "atan" => Self::Atan,
            "ln" => Self::Ln,
            "log" | "log10" => Self::Log,
            "exp" => Self::Exp,
            "floor" => Self::Floor,
            "ceil" => Self::Ceil,
            "round" => Self::Round,
            _ => return None,
        })
    }

    /// None outside the function's domain: no answer is better than a wrong one.
    fn apply(self, x: f64) -> Option<f64> {
        let y = match self {
            Self::Sqrt if x >= 0.0 => x.sqrt(),
            Self::Abs => x.abs(),
            Self::Sin => x.sin(),
            Self::Cos => x.cos(),
            Self::Tan => x.tan(),
            Self::Asin if (-1.0..=1.0).contains(&x) => x.asin(),
            Self::Acos if (-1.0..=1.0).contains(&x) => x.acos(),
            Self::Atan => x.atan(),
            Self::Ln if x > 0.0 => x.ln(),
            Self::Log if x > 0.0 => x.log10(),
            Self::Exp => x.exp(),
            Self::Floor => x.floor(),
            Self::Ceil => x.ceil(),
            Self::Round => x.round(),
            _ => return None,
        };
        y.is_finite().then_some(y)
    }
}

/// "Percent OF a number", in the words people type whatever the language of the app.
const OF_WORDS: [&str; 8] = ["of", "de", "da", "do", "von", "di", "del", "dal"];

/// Mathematical intent: an operation, a function, or a product written without its sign (2π).
fn is_calculation(tokens: &[Token]) -> bool {
    tokens.iter().any(|t| matches!(t, Token::Plus | Token::Minus | Token::Times | Token::Divide | Token::Power | Token::Of | Token::Function(_)))
        || tokens.windows(2).any(|pair| pair[0].ends_product() && pair[1].starts_product())
}

/// The closers an unfinished expression still needs; None for a mismatch or one closer too many.
fn virtual_closings(tokens: &[Token]) -> Option<Vec<char>> {
    let mut open = Vec::new();
    for token in tokens {
        match token {
            Token::Left(bracket) => open.push(*bracket),
            Token::Right(bracket) if open.pop() != Some(*bracket) => return None,
            _ => {}
        }
    }
    open.reverse();
    Some(open)
}

fn tokenize(input: &str, sep: Separators) -> Option<Vec<Token>> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        let c = chars[at];
        if c.is_whitespace() {
            at += 1;
        } else if sep.part_of_number(c) {
            tokens.push(Token::Number(read_number(&chars, &mut at, sep)?));
        } else if c == 'π' {
            tokens.push(Token::Constant(std::f64::consts::PI));
            at += 1;
        } else if c.is_alphabetic() {
            let mut word = String::new();
            while at < chars.len() && chars[at].is_alphabetic() {
                word.extend(chars[at].to_lowercase());
                at += 1;
            }
            // Only log10 has digits in its name: 2x3 keeps its 3.
            if word == "log" {
                while at < chars.len() && chars[at].is_numeric() {
                    word.push(chars[at]);
                    at += 1;
                }
            }
            tokens.push(match word.as_str() {
                "x" => Token::Times,
                "pi" => Token::Constant(std::f64::consts::PI),
                "e" => Token::Constant(std::f64::consts::E),
                word if OF_WORDS.contains(&word) => Token::Of,
                word => Token::Function(Function::named(word)?),
            });
        } else {
            at += 1;
            tokens.push(match c {
                '+' => Token::Plus,
                '-' | '\u{2212}' => Token::Minus,
                '*' | '\u{00D7}' => Token::Times,
                '/' | '\u{00F7}' => Token::Divide,
                '^' => Token::Power,
                '%' => Token::Percent,
                '(' | '[' => Token::Left(c),
                ')' => Token::Right('('),
                ']' => Token::Right('['),
                _ => return None,
            });
        }
    }
    (!tokens.is_empty()).then_some(tokens)
}

/// Thousands never start at zero and come in threes: "1,234" groups, "0,125" and "1,5" have decimals.
fn looks_like_grouping(raw: &str, separator: char) -> bool {
    let parts: Vec<&str> = raw.split(separator).collect();
    parts.len() >= 2 && (1..=3).contains(&parts[0].len()) && !parts[0].starts_with('0') && parts[1..].iter().all(|p| p.len() == 3)
}

/// One number, deciding what each separator in it means. With the decimal and
/// the alternate both there, the last one is the decimal point; the alternate
/// alone groups only if it looks the part. An exponent may follow.
fn read_number(chars: &[char], at: &mut usize, sep: Separators) -> Option<f64> {
    let (decimal, alternate, grouping_only) = (sep.decimal, sep.alternate(), sep.grouping_only());
    let mut written = String::new();
    while *at < chars.len() && sep.part_of_number(chars[*at]) {
        // A grouping character only groups when a digit follows it.
        if Some(chars[*at]) == grouping_only && !chars.get(*at + 1).is_some_and(|c| c.is_numeric()) {
            break;
        }
        written.push(chars[*at]);
        *at += 1;
    }
    if written.is_empty() {
        return None;
    }
    let mut raw = written.clone();
    if let Some(space) = grouping_only.filter(|g| written.contains(*g)) {
        let whole: String = written.chars().take_while(|c| *c != decimal && *c != alternate).collect();
        if !looks_like_grouping(&whole, space) || written[whole.len()..].contains(space) {
            return None;
        }
        raw = written.replace(space, "");
    }

    let mut exponent = String::new();
    if matches!(chars.get(*at), Some('e' | 'E')) {
        let mut end = *at + 1;
        if matches!(chars.get(end), Some('+' | '-')) {
            end += 1;
        }
        let digits = end;
        while chars.get(end).is_some_and(|c| c.is_ascii_digit()) {
            end += 1;
        }
        if end > digits {
            exponent = chars[*at..end].iter().collect();
            *at = end;
        }
    }

    let (has_decimal, has_alternate) = (raw.contains(decimal), decimal != alternate && raw.contains(alternate));
    let plain = if has_decimal && has_alternate {
        let (point, group) = if raw.rfind(alternate) > raw.rfind(decimal) { (alternate, decimal) } else { (decimal, alternate) };
        raw.replace(group, "").replace(point, ".")
    } else if has_alternate {
        raw.replace(alternate, if looks_like_grouping(&raw, alternate) { "" } else { "." })
    } else {
        raw.replace(decimal, ".")
    };
    if plain.matches('.').count() > 1 {
        return None;
    }
    format!("{plain}{exponent}").parse::<f64>().ok().filter(|v| v.is_finite())
}

// ── Parser ────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct Operand {
    value: f64,
    /// The percentage as written, for "100 + 10%" to add ten percent of the hundred.
    percent: Option<f64>,
}

fn plain(value: f64) -> Option<Operand> {
    value.is_finite().then_some(Operand { value, percent: None })
}

struct Parser<'a> {
    tokens: &'a [Token],
    at: usize,
    depth: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<Token> {
        self.tokens.get(self.at).copied()
    }

    /// Sums and differences; a percentage on the right is taken of what is on the left.
    fn expression(&mut self) -> Option<Operand> {
        if self.depth >= 32 {
            return None;
        }
        let mut left = self.term()?;
        while let Some(sign @ (Token::Plus | Token::Minus)) = self.peek() {
            self.at += 1;
            let right = self.term()?;
            let delta = right.percent.map_or(right.value, |p| left.value * p / 100.0);
            left = plain(if sign == Token::Plus { left.value + delta } else { left.value - delta })?;
        }
        Some(left)
    }

    /// Products, written or not, divisions and "X% of Y".
    fn term(&mut self) -> Option<Operand> {
        let mut left = self.factor()?;
        while let Some(token) = self.peek() {
            let implicit = token.starts_product();
            if !(implicit || matches!(token, Token::Times | Token::Divide | Token::Of)) {
                break;
            }
            if !implicit {
                self.at += 1;
            }
            if token == Token::Of && left.percent.is_none() {
                return None;
            }
            let right = self.factor()?;
            left = plain(if token == Token::Divide {
                if right.value == 0.0 {
                    return None;
                }
                left.value / right.value
            } else {
                left.value * right.value
            })?;
        }
        Some(left)
    }

    /// A sign applies to the whole power: -2^2 is -4.
    fn factor(&mut self) -> Option<Operand> {
        match self.peek() {
            Some(Token::Minus) => {
                self.at += 1;
                let operand = self.factor()?;
                Some(Operand { value: -operand.value, percent: operand.percent.map(|p| -p) })
            }
            Some(Token::Plus) => {
                self.at += 1;
                self.factor()
            }
            _ => self.power(),
        }
    }

    /// Powers group to the right: 2^3^2 is 2^(3^2).
    fn power(&mut self) -> Option<Operand> {
        let base = self.primary()?;
        if self.peek() != Some(Token::Power) {
            return Some(base);
        }
        self.at += 1;
        let exponent = self.factor()?;
        plain(base.value.powf(exponent.value))
    }

    fn primary(&mut self) -> Option<Operand> {
        let value = match self.peek()? {
            Token::Number(value) | Token::Constant(value) => {
                self.at += 1;
                value
            }
            Token::Left(_) => self.bracketed()?,
            Token::Function(function) => {
                self.at += 1;
                function.apply(self.bracketed()?)?
            }
            _ => return None,
        };
        if self.peek() == Some(Token::Percent) {
            self.at += 1;
            return Some(Operand { value: value / 100.0, percent: Some(value) });
        }
        Some(Operand { value, percent: None })
    }

    fn bracketed(&mut self) -> Option<f64> {
        let Some(Token::Left(bracket)) = self.peek() else { return None };
        self.at += 1;
        self.depth += 1;
        let inner = self.expression();
        self.depth -= 1;
        if self.peek() != Some(Token::Right(bracket)) {
            return None;
        }
        self.at += 1;
        inner.map(|operand| operand.value)
    }
}
