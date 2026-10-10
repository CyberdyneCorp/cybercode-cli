use super::{ConversionError, PermissionRule};
use serde_json::Value;

const MAX_RULES: usize = 4096;
const MAX_TEXT_BYTES: usize = 1024 * 1024;

fn refused(field: &str, reason: &'static str) -> ConversionError {
    ConversionError {
        field: field.into(),
        reason,
    }
}

/// Convert already-parsed constant prefix_rule records; never evaluate Starlark or grant trust.
pub fn codex_prefix_rules(source: &Value) -> Result<Vec<PermissionRule>, ConversionError> {
    let source = source
        .as_array()
        .ok_or_else(|| refused("rules", "expected a prefix rule array"))?;
    if source.len() > MAX_RULES {
        return Err(refused("rules", "rule expansion limit exceeded"));
    }
    let mut rules = Vec::new();
    let mut text_bytes = 0;
    for (index, item) in source.iter().enumerate() {
        let field = format!("rules[{index}]");
        let item = item
            .as_object()
            .ok_or_else(|| refused(&field, "expected a prefix rule object"))?;
        if item
            .keys()
            .any(|key| !["pattern", "decision"].contains(&key.as_str()))
        {
            return Err(refused(&field, "unsupported prefix rule field"));
        }
        let effect = effect(item.get("decision"), &field)?;
        let pattern = item
            .get("pattern")
            .and_then(Value::as_array)
            .filter(|p| !p.is_empty())
            .ok_or_else(|| refused(&field, "expected a nonempty argv prefix"))?;
        let prefixes = expand(
            pattern,
            MAX_RULES - rules.len(),
            MAX_TEXT_BYTES - text_bytes,
            &field,
        )?;
        text_bytes += prefixes
            .iter()
            .map(|prefix| prefix.iter().map(String::len).sum::<usize>() + prefix.len() + 1)
            .sum::<usize>();
        rules.extend(prefixes.into_iter().map(|prefix| PermissionRule {
            tool: None,
            action: "bash".into(),
            resource: format!("{} *", prefix.join(" ")),
            argv_prefix: Some(prefix),
            effect: effect.into(),
        }));
    }
    // Codex resolves all matches by strength; stable grouping preserves same-effect source order.
    rules.sort_by_key(|rule| match rule.effect.as_str() {
        "allow" => 0,
        "ask" => 1,
        _ => 2,
    });
    Ok(rules)
}

fn effect(value: Option<&Value>, field: &str) -> Result<&'static str, ConversionError> {
    match value {
        None => Ok("allow"),
        Some(Value::String(s)) => match s.as_str() {
            "allow" => Ok("allow"),
            "prompt" => Ok("ask"),
            "forbidden" => Ok("deny"),
            _ => Err(refused(field, "unsupported prefix decision")),
        },
        _ => Err(refused(field, "expected a string decision")),
    }
}

fn expand(
    pattern: &[Value],
    remaining: usize,
    remaining_bytes: usize,
    field: &str,
) -> Result<Vec<Vec<String>>, ConversionError> {
    if pattern.len() > 128 {
        return Err(refused(field, "argv prefix length limit exceeded"));
    }
    let mut prefixes = vec![Vec::new()];
    for (index, position) in pattern.iter().enumerate() {
        let field = format!("{field}.pattern[{index}]");
        let choices = choices(position, &field)?;
        if prefixes
            .len()
            .checked_mul(choices.len())
            .is_none_or(|count| count > remaining)
        {
            return Err(refused(&field, "rule expansion limit exceeded"));
        }
        let prefix_bytes: usize = prefixes
            .iter()
            .map(|prefix| prefix.iter().map(String::len).sum::<usize>() + prefix.len())
            .sum();
        let choice_bytes: usize = choices.iter().map(|choice| choice.len() + 1).sum();
        let expanded_bytes = prefix_bytes * choices.len() + choice_bytes * prefixes.len();
        if expanded_bytes + prefixes.len() * choices.len() > remaining_bytes {
            return Err(refused(&field, "expanded command text limit exceeded"));
        }
        prefixes = prefixes
            .into_iter()
            .flat_map(|prefix| {
                choices.iter().map(move |choice| {
                    let mut expanded = prefix.clone();
                    expanded.push((*choice).into());
                    expanded
                })
            })
            .collect();
    }
    Ok(prefixes)
}

fn choices<'a>(value: &'a Value, field: &str) -> Result<Vec<&'a str>, ConversionError> {
    let values: Vec<&Value> = match value {
        Value::String(_) => vec![value],
        Value::Array(values) if !values.is_empty() && values.len() <= MAX_RULES => {
            values.iter().collect()
        }
        _ => {
            return Err(refused(
                field,
                "expected a literal token or nonempty alternatives",
            ));
        }
    };
    values
        .into_iter()
        .map(|value| {
            let token = value
                .as_str()
                .ok_or_else(|| refused(field, "expected a literal token"))?;
            if token.is_empty()
                || token.len() > 4096
                || !token
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-./:=+@".contains(&b))
            {
                return Err(refused(
                    field,
                    "token cannot be represented as a literal command pattern",
                ));
            }
            Ok(token)
        })
        .collect()
}
