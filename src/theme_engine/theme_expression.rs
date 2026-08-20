use super::*;

pub fn evaluate(source: &str, context: &DataContext) -> Result<f64, String> {
    let mut parser = Parser {
        source: source.as_bytes(),
        index: 0,
        context,
    };
    let value = parser.parse_expression()?;
    parser.skip_space();
    if parser.index != parser.source.len() {
        Err(format!(
            "Unexpected input at character {}",
            parser.index + 1
        ))
    } else {
        Ok(value)
    }
}

pub(super) struct Parser<'a> {
    source: &'a [u8],
    index: usize,
    context: &'a DataContext,
}

impl Parser<'_> {
    fn parse_expression(&mut self) -> Result<f64, String> {
        self.parse_or()
    }
    fn parse_or(&mut self) -> Result<f64, String> {
        let mut value = self.parse_and()?;
        while self.consume(b"||") {
            let rhs = self.parse_and()?;
            value = ((value != 0.0) || (rhs != 0.0)) as u8 as f64;
        }
        Ok(value)
    }
    fn parse_and(&mut self) -> Result<f64, String> {
        let mut value = self.parse_comparison()?;
        while self.consume(b"&&") {
            let rhs = self.parse_comparison()?;
            value = ((value != 0.0) && (rhs != 0.0)) as u8 as f64;
        }
        Ok(value)
    }
    fn parse_comparison(&mut self) -> Result<f64, String> {
        let mut value = self.parse_sum()?;
        loop {
            let operation = [
                b">=".as_slice(),
                b"<=".as_slice(),
                b"==".as_slice(),
                b"!=".as_slice(),
                b">".as_slice(),
                b"<".as_slice(),
            ]
            .into_iter()
            .find(|operator| self.consume(operator));
            let Some(operation) = operation else {
                return Ok(value);
            };
            let rhs = self.parse_sum()?;
            value = match operation {
                b">=" => value >= rhs,
                b"<=" => value <= rhs,
                b"==" => (value - rhs).abs() < f64::EPSILON,
                b"!=" => (value - rhs).abs() >= f64::EPSILON,
                b">" => value > rhs,
                b"<" => value < rhs,
                _ => false,
            } as u8 as f64;
        }
    }
    fn parse_sum(&mut self) -> Result<f64, String> {
        let mut value = self.parse_term()?;
        loop {
            self.skip_space();
            match self.peek() {
                Some(b'+') => {
                    self.index += 1;
                    value += self.parse_term()?;
                }
                Some(b'-') => {
                    self.index += 1;
                    value -= self.parse_term()?;
                }
                _ => return Ok(value),
            }
        }
    }
    fn parse_term(&mut self) -> Result<f64, String> {
        let mut value = self.parse_unary()?;
        loop {
            self.skip_space();
            match self.peek() {
                Some(b'*') => {
                    self.index += 1;
                    value *= self.parse_unary()?;
                }
                Some(b'/') => {
                    self.index += 1;
                    let rhs = self.parse_unary()?;
                    if rhs == 0.0 {
                        return Err("Division by zero".into());
                    }
                    value /= rhs;
                }
                Some(b'%') => {
                    self.index += 1;
                    let rhs = self.parse_unary()?;
                    if rhs == 0.0 {
                        return Err("Division by zero".into());
                    }
                    value %= rhs;
                }
                _ => return Ok(value),
            }
        }
    }
    fn parse_unary(&mut self) -> Result<f64, String> {
        self.skip_space();
        match self.peek() {
            Some(b'+') => {
                self.index += 1;
                self.parse_unary()
            }
            Some(b'-') => {
                self.index += 1;
                Ok(-self.parse_unary()?)
            }
            Some(b'!') => {
                self.index += 1;
                Ok((self.parse_unary()? == 0.0) as u8 as f64)
            }
            _ => self.parse_primary(),
        }
    }
    fn parse_primary(&mut self) -> Result<f64, String> {
        self.skip_space();
        if self.peek() == Some(b'(') {
            self.index += 1;
            let value = self.parse_expression()?;
            self.skip_space();
            if self.peek() != Some(b')') {
                return Err("Missing closing parenthesis".into());
            }
            self.index += 1;
            return Ok(value);
        }
        if self.peek().is_some_and(|c| c.is_ascii_digit() || c == b'.') {
            return self.parse_number();
        }
        let identifier = self.parse_identifier()?;
        self.skip_space();
        if self.peek() == Some(b'(') {
            self.index += 1;
            let mut arguments = Vec::new();
            self.skip_space();
            if self.peek() != Some(b')') {
                loop {
                    arguments.push(self.parse_expression()?);
                    self.skip_space();
                    if self.peek() == Some(b',') {
                        self.index += 1;
                    } else {
                        break;
                    }
                }
            }
            if self.peek() != Some(b')') {
                return Err(format!("Missing closing parenthesis after {identifier}"));
            }
            self.index += 1;
            return call_function(&identifier, &arguments);
        }
        self.context
            .get(&identifier)
            .ok_or_else(|| format!("Unknown value '{identifier}'"))
    }
    fn parse_number(&mut self) -> Result<f64, String> {
        let start = self.index;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-'))
        {
            if matches!(self.peek(), Some(b'+' | b'-'))
                && self.index > start
                && !matches!(self.source[self.index - 1], b'e' | b'E')
            {
                break;
            }
            self.index += 1;
        }
        std::str::from_utf8(&self.source[start..self.index])
            .ok()
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| "Invalid number".to_string())
    }
    fn parse_identifier(&mut self) -> Result<String, String> {
        let start = self.index;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.'))
        {
            self.index += 1;
        }
        if start == self.index {
            Err(format!("Expected a value at character {}", self.index + 1))
        } else {
            Ok(String::from_utf8_lossy(&self.source[start..self.index]).to_string())
        }
    }
    fn skip_space(&mut self) {
        while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
            self.index += 1;
        }
    }
    fn consume(&mut self, token: &[u8]) -> bool {
        self.skip_space();
        if self.source.get(self.index..self.index + token.len()) == Some(token) {
            self.index += token.len();
            true
        } else {
            false
        }
    }
    fn peek(&self) -> Option<u8> {
        self.source.get(self.index).copied()
    }
}

pub(super) fn call_function(name: &str, args: &[f64]) -> Result<f64, String> {
    let arity = |count: usize| {
        if args.len() == count {
            Ok(())
        } else {
            Err(format!("{name} expects {count} argument(s)"))
        }
    };
    match name.to_ascii_lowercase().as_str() {
        "min" => {
            arity(2)?;
            Ok(args[0].min(args[1]))
        }
        "max" => {
            arity(2)?;
            Ok(args[0].max(args[1]))
        }
        "clamp" => {
            arity(3)?;
            if args[1] > args[2] {
                return Err("clamp minimum cannot exceed maximum".into());
            }
            Ok(args[0].clamp(args[1], args[2]))
        }
        "round" => {
            arity(1)?;
            Ok(args[0].round())
        }
        "floor" => {
            arity(1)?;
            Ok(args[0].floor())
        }
        "ceil" => {
            arity(1)?;
            Ok(args[0].ceil())
        }
        "abs" => {
            arity(1)?;
            Ok(args[0].abs())
        }
        "sqrt" => {
            arity(1)?;
            Ok(args[0].sqrt())
        }
        "pow" => {
            arity(2)?;
            Ok(args[0].powf(args[1]))
        }
        "if" => {
            arity(3)?;
            Ok(if args[0] != 0.0 { args[1] } else { args[2] })
        }
        "lerp" => {
            arity(3)?;
            Ok(args[0] + (args[1] - args[0]) * args[2])
        }
        _ => Err(format!("Unknown function '{name}'")),
    }
}

pub fn parse_color(source: &str) -> Option<Rgba> {
    let hex = source.trim().strip_prefix('#')?;
    let pair = |start| u8::from_str_radix(&hex[start..start + 2], 16).ok();
    match hex.len() {
        6 => Some(Rgba {
            r: pair(0)?,
            g: pair(2)?,
            b: pair(4)?,
            a: 255,
        }),
        8 => Some(Rgba {
            r: pair(0)?,
            g: pair(2)?,
            b: pair(4)?,
            a: pair(6)?,
        }),
        _ => None,
    }
}

pub(super) fn format_usage_line(base: &str, context: &DataContext) -> Option<String> {
    let mut parts = base.split('.');
    let provider = parts.next()?;
    let window = parts.next()?;
    if parts.next().is_some()
        || !matches!(
            provider,
            "active" | "claude" | "codex" | "antigravity" | "opencode" | "cursor"
        )
        || !matches!(
            window,
            "session" | "five_hour" | "weekly" | "monthly" | "credits"
        )
    {
        return None;
    }
    if context.get("data.loading").unwrap_or(0.0) != 0.0 {
        return Some("--".into());
    }
    if context.get("data.has_error").unwrap_or(0.0) != 0.0
        || (context.get("data.poll_ok").unwrap_or(1.0) != 0.0
            && context.get(&format!("{provider}.available")).unwrap_or(0.0) == 0.0)
    {
        return Some("!".into());
    }
    let percentage = context
        .get(&format!("{provider}.{window}.percentage"))
        .unwrap_or(0.0);
    let percentage = format_value(percentage, "0", context);
    if context
        .get(&format!("{provider}.{window}.reset.unix"))
        .unwrap_or(0.0)
        <= 0.0
    {
        return Some(format!("{percentage}%"));
    }
    let seconds = context
        .get(&format!("{provider}.{window}.reset.seconds"))
        .unwrap_or(0.0);
    Some(format!(
        "{percentage}% · {}",
        format_value(seconds, "duration_short", context)
    ))
}

pub(super) fn format_usage_badge(base: &str, context: &DataContext) -> Option<String> {
    let line = format_usage_line(base, context)?;
    Some(
        line.split_once(" · ")
            .map(|(percentage, _)| percentage.to_string())
            .unwrap_or(line),
    )
}

pub(super) fn localized<'a>(context: &'a DataContext, name: &str, fallback: &'a str) -> &'a str {
    context.get_string(name).unwrap_or(fallback)
}

pub(super) fn format_value(value: f64, format: &str, context: &DataContext) -> String {
    if format.eq_ignore_ascii_case("duration_short") {
        let seconds = value.max(0.0).round() as u64;
        let days = seconds / 86_400;
        let hours = seconds / 3_600;
        let minutes = seconds / 60;
        return if days > 0 {
            format!("{days}{}", localized(context, "i18n.day_suffix", "d"))
        } else if hours > 0 {
            format!("{hours}{}", localized(context, "i18n.hour_suffix", "h"))
        } else if minutes > 0 {
            format!("{minutes}{}", localized(context, "i18n.minute_suffix", "m"))
        } else if seconds > 0 {
            format!("{seconds}{}", localized(context, "i18n.second_suffix", "s"))
        } else {
            localized(context, "i18n.now", "now").to_string()
        };
    }
    if format.eq_ignore_ascii_case("duration") {
        let seconds = value.max(0.0).round() as u64;
        let days = seconds / 86_400;
        let hours = seconds % 86_400 / 3_600;
        let minutes = seconds % 3_600 / 60;
        return if days > 0 {
            format!(
                "{days}{} {hours}{}",
                localized(context, "i18n.day_suffix", "d"),
                localized(context, "i18n.hour_suffix", "h")
            )
        } else if hours > 0 {
            format!(
                "{hours}{} {minutes}{}",
                localized(context, "i18n.hour_suffix", "h"),
                localized(context, "i18n.minute_suffix", "m")
            )
        } else if minutes > 0 {
            format!("{minutes}{}", localized(context, "i18n.minute_suffix", "m"))
        } else if seconds > 0 {
            format!("{seconds}{}", localized(context, "i18n.second_suffix", "s"))
        } else {
            localized(context, "i18n.now", "now").to_string()
        };
    }
    if format.eq_ignore_ascii_case("percent") {
        return format!("{value:.0}%");
    }
    let decimals = format
        .split('.')
        .nth(1)
        .map(|part| part.chars().filter(|c| matches!(c, '0' | '#')).count())
        .unwrap_or(0);
    let fixed = format.contains('.') && format.contains('0');
    let mut result = format!("{value:.decimals$}");
    if !fixed && decimals > 0 {
        while result.ends_with('0') {
            result.pop();
        }
        if result.ends_with('.') {
            result.pop();
        }
    }
    if format.contains(',') {
        let (sign, digits) = result
            .strip_prefix('-')
            .map(|v| ("-", v))
            .unwrap_or(("", result.as_str()));
        let (whole, fraction) = digits
            .split_once('.')
            .map(|(w, f)| (w, Some(f)))
            .unwrap_or((digits, None));
        let mut grouped = String::new();
        for (i, c) in whole.chars().rev().enumerate() {
            if i > 0 && i % 3 == 0 {
                grouped.push(',');
            }
            grouped.push(c);
        }
        result = format!(
            "{}{}{}",
            sign,
            grouped.chars().rev().collect::<String>(),
            fraction.map(|f| format!(".{f}")).unwrap_or_default()
        );
    }
    result
}
