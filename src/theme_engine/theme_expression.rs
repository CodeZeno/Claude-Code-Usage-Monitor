use super::*;

#[derive(Clone, Debug, PartialEq)]
pub(super) enum ExpressionValue {
    Number(f64),
    Text(String),
}

impl ExpressionValue {
    fn number(&self) -> Result<f64, String> {
        match self {
            Self::Number(value) => Ok(*value),
            Self::Text(_) => Err("Expected a number, found text".into()),
        }
    }
}

pub fn evaluate(source: &str, context: &DataContext) -> Result<f64, String> {
    evaluate_value(source, context)?.number()
}

pub(super) fn evaluate_value(
    source: &str,
    context: &DataContext,
) -> Result<ExpressionValue, String> {
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

pub(super) fn split_template_token(token: &str) -> (&str, &str) {
    let mut quote = None;
    let mut escaped = false;
    let mut separator = None;
    for (index, character) in token.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if quote.is_some() && character == '\\' {
            escaped = true;
        } else if quote == Some(character) {
            quote = None;
        } else if quote.is_none() && matches!(character, '"' | '\'') {
            quote = Some(character);
        } else if quote.is_none() && character == ':' {
            separator = Some(index);
        }
    }
    separator.map_or((token, "0.##"), |index| {
        (&token[..index], &token[index + 1..])
    })
}

pub(super) struct Parser<'a> {
    source: &'a [u8],
    index: usize,
    context: &'a DataContext,
}

impl Parser<'_> {
    fn parse_expression(&mut self) -> Result<ExpressionValue, String> {
        self.parse_or()
    }
    fn parse_or(&mut self) -> Result<ExpressionValue, String> {
        let mut value = self.parse_and()?;
        while self.consume(b"||") {
            let rhs = self.parse_and()?;
            value = ExpressionValue::Number(
                ((value.number()? != 0.0) || (rhs.number()? != 0.0)) as u8 as f64,
            );
        }
        Ok(value)
    }
    fn parse_and(&mut self) -> Result<ExpressionValue, String> {
        let mut value = self.parse_comparison()?;
        while self.consume(b"&&") {
            let rhs = self.parse_comparison()?;
            value = ExpressionValue::Number(
                ((value.number()? != 0.0) && (rhs.number()? != 0.0)) as u8 as f64,
            );
        }
        Ok(value)
    }
    fn parse_comparison(&mut self) -> Result<ExpressionValue, String> {
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
            let comparison = match operation {
                b"==" => match (&value, &rhs) {
                    (ExpressionValue::Number(lhs), ExpressionValue::Number(rhs)) => {
                        (lhs - rhs).abs() < f64::EPSILON
                    }
                    (ExpressionValue::Text(lhs), ExpressionValue::Text(rhs)) => lhs == rhs,
                    _ => false,
                },
                b"!=" => match (&value, &rhs) {
                    (ExpressionValue::Number(lhs), ExpressionValue::Number(rhs)) => {
                        (lhs - rhs).abs() >= f64::EPSILON
                    }
                    (ExpressionValue::Text(lhs), ExpressionValue::Text(rhs)) => lhs != rhs,
                    _ => true,
                },
                b">=" => value.number()? >= rhs.number()?,
                b"<=" => value.number()? <= rhs.number()?,
                b">" => value.number()? > rhs.number()?,
                b"<" => value.number()? < rhs.number()?,
                _ => false,
            };
            value = ExpressionValue::Number(comparison as u8 as f64);
        }
    }
    fn parse_sum(&mut self) -> Result<ExpressionValue, String> {
        let mut value = self.parse_term()?;
        loop {
            self.skip_space();
            match self.peek() {
                Some(b'+') => {
                    self.index += 1;
                    let rhs = self.parse_term()?;
                    value = ExpressionValue::Number(value.number()? + rhs.number()?);
                }
                Some(b'-') => {
                    self.index += 1;
                    let rhs = self.parse_term()?;
                    value = ExpressionValue::Number(value.number()? - rhs.number()?);
                }
                _ => return Ok(value),
            }
        }
    }
    fn parse_term(&mut self) -> Result<ExpressionValue, String> {
        let mut value = self.parse_unary()?;
        loop {
            self.skip_space();
            match self.peek() {
                Some(b'*') => {
                    self.index += 1;
                    let rhs = self.parse_unary()?;
                    value = ExpressionValue::Number(value.number()? * rhs.number()?);
                }
                Some(b'/') => {
                    self.index += 1;
                    let rhs = self.parse_unary()?.number()?;
                    if rhs == 0.0 {
                        return Err("Division by zero".into());
                    }
                    value = ExpressionValue::Number(value.number()? / rhs);
                }
                Some(b'%') => {
                    self.index += 1;
                    let rhs = self.parse_unary()?.number()?;
                    if rhs == 0.0 {
                        return Err("Division by zero".into());
                    }
                    value = ExpressionValue::Number(value.number()? % rhs);
                }
                _ => return Ok(value),
            }
        }
    }
    fn parse_unary(&mut self) -> Result<ExpressionValue, String> {
        self.skip_space();
        match self.peek() {
            Some(b'+') => {
                self.index += 1;
                self.parse_unary()
            }
            Some(b'-') => {
                self.index += 1;
                Ok(ExpressionValue::Number(-self.parse_unary()?.number()?))
            }
            Some(b'!') => {
                self.index += 1;
                Ok(ExpressionValue::Number(
                    (self.parse_unary()?.number()? == 0.0) as u8 as f64,
                ))
            }
            _ => self.parse_primary(),
        }
    }
    fn parse_primary(&mut self) -> Result<ExpressionValue, String> {
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
        if matches!(self.peek(), Some(b'"' | b'\'')) {
            return self.parse_string().map(ExpressionValue::Text);
        }
        if self.peek().is_some_and(|c| c.is_ascii_digit() || c == b'.') {
            return self.parse_number().map(ExpressionValue::Number);
        }
        let identifier = self.parse_identifier()?;
        self.skip_space();
        if self.peek() == Some(b'(') {
            self.index += 1;
            if identifier.eq_ignore_ascii_case("get") {
                return self.parse_get();
            }
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
        if let Some(value) = self.context.get(&identifier) {
            Ok(ExpressionValue::Number(value))
        } else if let Some(value) = self.context.get_string(&identifier) {
            Ok(ExpressionValue::Text(value.to_string()))
        } else {
            Err(format!("Unknown value '{identifier}'"))
        }
    }
    fn parse_get(&mut self) -> Result<ExpressionValue, String> {
        self.skip_space();
        if self.peek() == Some(b')') {
            return Err("get expects get(target, property) or get(target.property)".into());
        }

        let first = self.parse_get_name()?;
        self.skip_space();
        let mut key = if self.peek() == Some(b',') {
            self.index += 1;
            self.skip_space();
            let property = self.parse_get_name()?;
            format!("{first}.{property}")
        } else {
            first
        };
        self.skip_space();
        if self.peek() != Some(b')') {
            return Err("get expects get(target, property) or get(target.property)".into());
        }
        self.index += 1;

        if key.eq_ignore_ascii_case("self") {
            key = "this".into();
        } else if key
            .get(..5)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("self."))
        {
            key.replace_range(..4, "this");
        }
        if let Some(value) = self.context.get(&key) {
            Ok(ExpressionValue::Number(value))
        } else if let Some(value) = self.context.get_string(&key) {
            Ok(ExpressionValue::Text(value.to_string()))
        } else {
            Err(format!("get could not find '{key}'"))
        }
    }
    fn parse_get_name(&mut self) -> Result<String, String> {
        if matches!(self.peek(), Some(b'"' | b'\'')) {
            self.parse_string()
        } else {
            self.parse_identifier()
        }
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
    fn parse_string(&mut self) -> Result<String, String> {
        let quote = self.peek().expect("parse_string requires a quote");
        self.index += 1;
        let mut result = Vec::new();
        while let Some(character) = self.peek() {
            self.index += 1;
            if character == quote {
                return String::from_utf8(result).map_err(|_| "Invalid UTF-8 in text".into());
            }
            if character != b'\\' {
                result.push(character);
                continue;
            }
            let escaped = self
                .peek()
                .ok_or_else(|| "Missing closing quote in text".to_string())?;
            self.index += 1;
            result.push(match escaped {
                b'n' => b'\n',
                b'r' => b'\r',
                b't' => b'\t',
                b'\\' => b'\\',
                b'"' => b'"',
                b'\'' => b'\'',
                _ => {
                    return Err(format!(
                        "Unsupported escape sequence '\\{}'",
                        escaped as char
                    ))
                }
            });
        }
        Err("Missing closing quote in text".into())
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

pub(super) fn call_function(
    name: &str,
    args: &[ExpressionValue],
) -> Result<ExpressionValue, String> {
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
            Ok(ExpressionValue::Number(
                args[0].number()?.min(args[1].number()?),
            ))
        }
        "max" => {
            arity(2)?;
            Ok(ExpressionValue::Number(
                args[0].number()?.max(args[1].number()?),
            ))
        }
        "clamp" => {
            arity(3)?;
            let value = args[0].number()?;
            let minimum = args[1].number()?;
            let maximum = args[2].number()?;
            if minimum > maximum {
                return Err("clamp minimum cannot exceed maximum".into());
            }
            Ok(ExpressionValue::Number(value.clamp(minimum, maximum)))
        }
        "round" => {
            arity(1)?;
            Ok(ExpressionValue::Number(args[0].number()?.round()))
        }
        "floor" => {
            arity(1)?;
            Ok(ExpressionValue::Number(args[0].number()?.floor()))
        }
        "ceil" => {
            arity(1)?;
            Ok(ExpressionValue::Number(args[0].number()?.ceil()))
        }
        "abs" => {
            arity(1)?;
            Ok(ExpressionValue::Number(args[0].number()?.abs()))
        }
        "sqrt" => {
            arity(1)?;
            Ok(ExpressionValue::Number(args[0].number()?.sqrt()))
        }
        "pow" => {
            arity(2)?;
            Ok(ExpressionValue::Number(
                args[0].number()?.powf(args[1].number()?),
            ))
        }
        "if" => {
            arity(3)?;
            Ok(if args[0].number()? != 0.0 {
                args[1].clone()
            } else {
                args[2].clone()
            })
        }
        "lerp" => {
            arity(3)?;
            let start = args[0].number()?;
            let end = args[1].number()?;
            Ok(ExpressionValue::Number(
                start + (end - start) * args[2].number()?,
            ))
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
            "session" | "five_hour" | "weekly" | "monthly" | "fable" | "credits"
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
    if let Some(value) = format_timestamp(value, format, context) {
        return value;
    }
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
