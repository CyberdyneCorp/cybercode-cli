use super::{ConversionError, PermissionRule, codex_prefix_rules};
use serde::Serialize;
use serde_json::{Map, Value};

#[derive(Debug, Serialize)]
pub struct CodexRules {
    pub rules: Vec<PermissionRule>,
    pub sources: Vec<CodexRuleSource>,
}

#[derive(Debug, Serialize)]
pub struct CodexRuleSource {
    pub pattern: Value,
    pub decision: Option<String>,
    pub justification: Option<String>,
    pub match_examples: Vec<Vec<String>>,
    pub not_match_examples: Vec<Vec<String>>,
}

fn refused(field: &str, reason: &'static str) -> ConversionError {
    ConversionError {
        field: field.into(),
        reason,
    }
}

/// Parse constant rules only. Dynamic Starlark is refused, never evaluated or partially imported.
pub fn codex_rules(text: &str) -> Result<CodexRules, ConversionError> {
    if text.len() > 1024 * 1024 {
        return Err(refused("rules", "source size limit exceeded"));
    }
    let mut parser = Reader {
        chars: text.chars().collect(),
        at: 0,
    };
    let mut records = Vec::new();
    let mut sources = Vec::new();
    while parser.peek().is_some() {
        if records.len() == 4096 {
            return Err(refused("rules", "source rule limit exceeded"));
        }
        let field = format!("rules[{}]", records.len());
        let mut record = parser.call()?;
        parser.statement_end()?;
        let source = source_record(&mut record, &field)?;
        records.push(Value::Object(record));
        sources.push(source);
    }
    let rules = codex_prefix_rules(&Value::Array(records))?;
    for (index, source) in sources.iter().enumerate() {
        validate_examples(
            &source.pattern,
            &source.match_examples,
            true,
            &format!("rules[{index}].match"),
        )?;
        validate_examples(
            &source.pattern,
            &source.not_match_examples,
            false,
            &format!("rules[{index}].not_match"),
        )?;
    }
    Ok(CodexRules { rules, sources })
}

fn source_record(
    record: &mut Map<String, Value>,
    field: &str,
) -> Result<CodexRuleSource, ConversionError> {
    let pattern = record
        .get("pattern")
        .cloned()
        .ok_or_else(|| refused(field, "missing prefix pattern"))?;
    let justification = optional_string(record.remove("justification"), field)?;
    let match_examples = examples(record.remove("match"), &format!("{field}.match"))?;
    let not_match_examples = examples(record.remove("not_match"), &format!("{field}.not_match"))?;
    Ok(CodexRuleSource {
        pattern,
        decision: record
            .get("decision")
            .and_then(Value::as_str)
            .map(str::to_owned),
        justification,
        match_examples,
        not_match_examples,
    })
}

fn optional_string(value: Option<Value>, field: &str) -> Result<Option<String>, ConversionError> {
    value
        .map(|value| {
            value
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .map(str::to_owned)
                .ok_or_else(|| refused(field, "expected a nonempty rationale string"))
        })
        .transpose()
}

fn examples(value: Option<Value>, field: &str) -> Result<Vec<Vec<String>>, ConversionError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let items = value
        .as_array()
        .filter(|items| items.len() <= 4096)
        .ok_or_else(|| refused(field, "expected a bounded example array"))?;
    items
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let field = format!("{field}[{index}]");
            let argv = match value {
                Value::String(command) => shell_words::split(command)
                    .map_err(|_| refused(&field, "invalid example quoting"))?,
                Value::Array(items) => items
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| refused(&field, "expected example argv strings"))
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                _ => return Err(refused(&field, "expected a command string or argv array")),
            };
            if argv.is_empty() {
                return Err(refused(&field, "expected a nonempty command example"));
            }
            Ok(argv)
        })
        .collect()
}

fn validate_examples(
    pattern: &Value,
    examples: &[Vec<String>],
    expected: bool,
    field: &str,
) -> Result<(), ConversionError> {
    let positions = pattern
        .as_array()
        .ok_or_else(|| refused(field, "invalid prefix pattern"))?;
    for (index, argv) in examples.iter().enumerate() {
        let matched = argv.len() >= positions.len()
            && positions
                .iter()
                .zip(argv)
                .all(|(position, token)| match position {
                    Value::String(literal) => literal == token,
                    Value::Array(choices) => {
                        choices.iter().any(|value| value.as_str() == Some(token))
                    }
                    _ => false,
                });
        if matched != expected {
            return Err(refused(
                &format!("{field}[{index}]"),
                "example contradicts prefix pattern",
            ));
        }
    }
    Ok(())
}

struct Reader {
    chars: Vec<char>,
    at: usize,
}

impl Reader {
    fn error(&self, reason: &'static str) -> ConversionError {
        refused(&format!("rules.offset[{}]", self.at), reason)
    }

    fn peek(&mut self) -> Option<char> {
        loop {
            while self.chars.get(self.at).is_some_and(|c| c.is_whitespace()) {
                self.at += 1;
            }
            if self.chars.get(self.at) != Some(&'#') {
                return self.chars.get(self.at).copied();
            }
            while self.chars.get(self.at).is_some_and(|c| *c != '\n') {
                self.at += 1;
            }
        }
    }

    fn statement_end(&mut self) -> Result<(), ConversionError> {
        let start = self.at;
        let next = self.peek();
        if next == Some(';') {
            self.at += 1;
            return Ok(());
        }
        if next.is_none() || self.chars[start..self.at].contains(&'\n') {
            return Ok(());
        }
        Err(self.error("expected a statement separator"))
    }

    fn take(&mut self, expected: char) -> Result<(), ConversionError> {
        if self.peek() != Some(expected) {
            return Err(self.error("unexpected rules syntax"));
        }
        self.at += 1;
        Ok(())
    }

    fn name(&mut self) -> Result<String, ConversionError> {
        self.peek();
        let start = self.at;
        while self
            .chars
            .get(self.at)
            .is_some_and(|c| c.is_ascii_alphabetic() || *c == '_')
        {
            self.at += 1;
        }
        if self.at == start {
            return Err(self.error("expected a constant rule field"));
        }
        Ok(self.chars[start..self.at].iter().collect())
    }

    fn call(&mut self) -> Result<Map<String, Value>, ConversionError> {
        if self.name()? != "prefix_rule" {
            return Err(self.error("unsupported rules statement"));
        }
        self.take('(')?;
        let mut fields = Map::new();
        while self.peek() != Some(')') {
            let name = self.name()?;
            if !["pattern", "decision", "justification", "match", "not_match"]
                .contains(&name.as_str())
            {
                return Err(self.error("unsupported prefix rule field"));
            }
            if fields.contains_key(&name) {
                return Err(self.error("duplicate prefix rule field"));
            }
            self.take('=')?;
            fields.insert(name, self.value(0)?);
            if self.peek() == Some(')') {
                break;
            }
            self.take(',')?;
        }
        self.take(')')?;
        Ok(fields)
    }

    fn value(&mut self, depth: usize) -> Result<Value, ConversionError> {
        match self.peek() {
            Some('\'' | '"') => Ok(Value::String(self.string()?)),
            Some('[') if depth < 2 => {
                self.take('[')?;
                let mut values = Vec::new();
                while self.peek() != Some(']') {
                    if values.len() == 4096 {
                        return Err(self.error("literal list limit exceeded"));
                    }
                    values.push(self.value(depth + 1)?);
                    if self.peek() == Some(']') {
                        break;
                    }
                    self.take(',')?;
                }
                self.take(']')?;
                Ok(Value::Array(values))
            }
            _ => Err(self.error("expected a bounded literal string or list")),
        }
    }

    fn string(&mut self) -> Result<String, ConversionError> {
        let quote = self
            .peek()
            .ok_or_else(|| self.error("missing quoted string"))?;
        self.at += 1;
        let mut value = String::new();
        loop {
            let ch = self
                .chars
                .get(self.at)
                .copied()
                .ok_or_else(|| self.error("unterminated quoted string"))?;
            self.at += 1;
            match ch {
                c if c == quote => return Ok(value),
                '\n' | '\r' => return Err(self.error("unsupported multiline string")),
                '\\' => value.push(self.escape()?),
                c => value.push(c),
            }
        }
    }

    fn escape(&mut self) -> Result<char, ConversionError> {
        let ch = self
            .chars
            .get(self.at)
            .copied()
            .ok_or_else(|| self.error("unterminated string escape"))?;
        self.at += 1;
        match ch {
            '\\' | '\'' | '"' => Ok(ch),
            'n' => Ok('\n'),
            'r' => Ok('\r'),
            't' => Ok('\t'),
            'b' => Ok('\u{8}'),
            'f' => Ok('\u{c}'),
            'x' => self.hex(2),
            'u' => self.hex(4),
            'U' => self.hex(8),
            _ => Err(self.error("unsupported string escape")),
        }
    }

    fn hex(&mut self, count: usize) -> Result<char, ConversionError> {
        let mut value = 0;
        for _ in 0..count {
            let digit = self
                .chars
                .get(self.at)
                .and_then(|c| c.to_digit(16))
                .ok_or_else(|| self.error("invalid string escape"))?;
            self.at += 1;
            value = value * 16 + digit;
        }
        char::from_u32(value).ok_or_else(|| self.error("invalid Unicode scalar"))
    }
}
