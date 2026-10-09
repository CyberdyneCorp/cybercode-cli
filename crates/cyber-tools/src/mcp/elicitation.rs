//! Flat MCP forms use the existing durable, per-call question flow.
use cyber_server::runtime::{Asker, Question, QuestionOption, QuestionReply};
use serde_json::{Map, Value, json};

/// Transient call ownership and authorization; never retained by the connection.
pub struct ElicitationContext<'a> {
    asker: &'a Asker,
    authorize: &'a (dyn Fn() -> bool + Send + Sync),
}
impl<'a> ElicitationContext<'a> {
    pub fn new(asker: &'a Asker, authorize: &'a (dyn Fn() -> bool + Send + Sync)) -> Self {
        Self { asker, authorize }
    }
}

const OMIT: &str = "Leave this field unset";
const DEFAULT: &str = "Use the default value";

struct Field {
    name: String,
    kind: String,
    optional: bool,
    default: Option<Value>,
    choices: Vec<(String, Value)>,
    question: Question,
}
struct Form {
    fields: Vec<Field>,
    validator: jsonschema::Validator,
    questions: Vec<Question>,
}

fn choices(schema: &Value) -> Result<Vec<(String, Value)>, ()> {
    if let Some(values) = schema.get("enum") {
        let values = values.as_array().ok_or(())?;
        if values.is_empty() || values.len() > 64 {
            return Err(());
        }
        return values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let value = value.as_str().ok_or(())?;
                Ok((format!("{}. {value}", index + 1), json!(value)))
            })
            .collect();
    }
    if let Some(values) = schema.get("oneOf").or_else(|| schema.get("anyOf")) {
        let values = values.as_array().ok_or(())?;
        if values.is_empty() || values.len() > 64 {
            return Err(());
        }
        return values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let fields = value.as_object().ok_or(())?;
                if fields
                    .keys()
                    .any(|key| !matches!(key.as_str(), "const" | "title"))
                {
                    return Err(());
                }
                let raw = value["const"].as_str().ok_or(())?;
                let title = value
                    .get("title")
                    .map_or(Ok(raw), |title| title.as_str().ok_or(()))?;
                Ok((format!("{}. {title}", index + 1), json!(raw)))
            })
            .collect();
    }
    Ok(Vec::new())
}

fn field_kind(schema: &Value) -> Result<String, ()> {
    let props = schema.as_object().ok_or(())?;
    let kind = schema["type"].as_str().ok_or(())?;
    if props.keys().any(|key| !field_keyword(kind, key))
        || schema.get("format").is_some_and(|format| {
            !matches!(
                format.as_str(),
                Some("email" | "uri" | "date" | "date-time")
            )
        })
    {
        return Err(());
    }
    Ok(kind.into())
}

fn field_keyword(kind: &str, key: &str) -> bool {
    if matches!(key, "type" | "title" | "description" | "default") {
        return true;
    }
    match kind {
        "string" => matches!(
            key,
            "enum" | "oneOf" | "minLength" | "maxLength" | "pattern" | "format"
        ),
        "number" | "integer" => matches!(key, "minimum" | "maximum"),
        "array" => matches!(key, "items" | "minItems" | "maxItems" | "uniqueItems"),
        _ => false,
    }
}

fn field_choices(schema: &Value, kind: &str) -> Result<Vec<(String, Value)>, ()> {
    match kind {
        "string" => choices(schema),
        "boolean" => Ok(vec![
            ("true".into(), json!(true)),
            ("false".into(), json!(false)),
        ]),
        "number" | "integer" => Ok(Vec::new()),
        "array" => {
            let items = schema.get("items").ok_or(())?;
            let props = items.as_object().ok_or(())?;
            if props
                .keys()
                .any(|key| !matches!(key.as_str(), "type" | "enum" | "anyOf"))
                || items.get("type").is_some_and(|kind| kind != "string")
            {
                return Err(());
            }
            let choices = choices(items)?;
            if choices.is_empty() {
                return Err(());
            }
            Ok(choices)
        }
        _ => Err(()),
    }
}

impl Field {
    fn new(
        name: &str,
        schema: &Value,
        optional: bool,
        server: &str,
        message: &str,
    ) -> Result<Self, ()> {
        let kind = field_kind(schema)?;
        let selection = field_choices(schema, &kind)?;
        let default = schema.get("default").cloned();
        let mut options: Vec<_> = selection
            .iter()
            .map(|(label, _)| QuestionOption {
                label: label.clone(),
                description: String::new(),
            })
            .collect();
        if let Some(value) = &default {
            options.push(QuestionOption {
                label: DEFAULT.into(),
                description: format!("Use the server's declared default: {}", value),
            });
        }
        if optional {
            options.push(QuestionOption {
                label: OMIT.into(),
                description: "Do not send this field".into(),
            });
        }
        let title = schema
            .get("title")
            .map_or(Ok(name), |title| title.as_str().ok_or(()))?;
        let description = schema
            .get("description")
            .map_or(Ok(""), |description| description.as_str().ok_or(()))?;
        let question = Question {
            question: format!("MCP server {server}: {message}\n{title} ({kind})\n{description}"),
            header: name.chars().take(12).collect(),
            options,
            multi_select: kind == "array",
            allow_custom: matches!(kind.as_str(), "number" | "integer")
                || (kind == "string" && selection.is_empty()),
        };
        Ok(Self {
            name: name.into(),
            kind,
            optional,
            default,
            choices: selection,
            question,
        })
    }

    fn answer(&self, answers: &[String]) -> Result<Option<Value>, ()> {
        if answers.iter().any(|answer| answer == OMIT) {
            return if self.optional && answers.len() == 1 {
                Ok(None)
            } else {
                Err(())
            };
        }
        if answers.iter().any(|answer| answer == DEFAULT) {
            return if answers.len() == 1 {
                self.default.clone().map(Some).ok_or(())
            } else {
                Err(())
            };
        }
        if self.kind == "array" {
            return answers
                .iter()
                .map(|answer| self.choice(answer))
                .collect::<Result<Vec<_>, _>>()
                .map(|values| Some(json!(values)));
        }
        if answers.len() != 1 {
            return Err(());
        }
        if !self.choices.is_empty() {
            return self.choice(&answers[0]).map(Some);
        }
        if self.kind == "string" {
            return Ok(Some(json!(answers[0])));
        }
        let value: Value = serde_json::from_str(&answers[0]).map_err(|_| ())?;
        if !value.is_number() {
            return Err(());
        }
        Ok(Some(value))
    }

    fn choice(&self, label: &str) -> Result<Value, ()> {
        self.choices
            .iter()
            .find(|(choice, _)| choice == label)
            .map(|(_, value)| value.clone())
            .ok_or(())
    }
}

impl Form {
    fn new(params: &Value, server: &str) -> Result<Self, ()> {
        if params.get("mode").is_some_and(|mode| mode != "form") {
            return Err(());
        }
        let message = params["message"].as_str().ok_or(())?;
        if message.len() > 16_384 {
            return Err(());
        }
        let schema = params.get("requestedSchema").ok_or(())?;
        let root = schema.as_object().ok_or(())?;
        if schema["type"] != "object"
            || root.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "type"
                        | "properties"
                        | "required"
                        | "title"
                        | "description"
                        | "additionalProperties"
                )
            })
        {
            return Err(());
        }
        if schema
            .get("additionalProperties")
            .is_some_and(|value| !value.is_boolean())
        {
            return Err(());
        }
        let properties = schema["properties"].as_object().ok_or(())?;
        if properties.len() > 32 {
            return Err(());
        }
        let required: Vec<_> = match schema.get("required") {
            Some(value) => value
                .as_array()
                .ok_or(())?
                .iter()
                .map(|name| {
                    name.as_str()
                        .filter(|name| properties.contains_key(*name))
                        .ok_or(())
                })
                .collect::<Result<_, _>>()?,
            None => Vec::new(),
        };
        let fields = properties
            .iter()
            .map(|(name, schema)| {
                Field::new(
                    name,
                    schema,
                    !required.contains(&name.as_str()),
                    server,
                    message,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let validator = jsonschema::options()
            .should_validate_formats(true)
            .build(schema)
            .map_err(|_| ())?;
        let mut questions: Vec<_> = fields.iter().map(|field| field.question.clone()).collect();
        questions.push(Question {
            question:format!("Send these values to MCP server {server}? Review your answers before accepting.\n{message}"),
            header:"MCP consent".into(),
            options:vec![QuestionOption{label:"Accept".into(),description:"Send the completed form".into()},QuestionOption{label:"Decline".into(),description:"Send no form values".into()},QuestionOption{label:"Cancel".into(),description:"Cancel without sending values".into()}],
            multi_select:false,
            allow_custom:false,
        });
        if serde_json::to_vec(&questions).map_err(|_| ())?.len() > super::LIMIT {
            return Err(());
        }
        Ok(Self {
            fields,
            validator,
            questions,
        })
    }

    fn answer(&self, answers: &[Vec<String>]) -> Result<Value, ()> {
        if answers.len() != self.questions.len() {
            return Err(());
        }
        match answers.last().map(Vec::as_slice) {
            Some([choice]) if choice == "Cancel" => return Ok(json!({"action":"cancel"})),
            Some([choice]) if choice == "Decline" => return Ok(json!({"action":"decline"})),
            Some([choice]) if choice == "Accept" => {}
            _ => return Err(()),
        }
        let mut content = Map::new();
        for (field, answers) in self.fields.iter().zip(answers) {
            if let Some(value) = field.answer(answers)? {
                content.insert(field.name.clone(), value);
            }
        }
        let content = Value::Object(content);
        if !self.validator.is_valid(&content) {
            return Err(());
        }
        Ok(json!({"action":"accept","content":content}))
    }
}

/// The boolean marks actual user interaction, whose duration pauses call inactivity.
pub(super) async fn reply(
    server: &str,
    params: &Value,
    context: Option<&ElicitationContext<'_>>,
) -> (Value, bool) {
    let form = match Form::new(params, server) {
        Ok(form) => form,
        Err(()) => {
            return (
                json!({"error":{"code":-32602,"message":"Invalid or unsupported elicitation form"}}),
                false,
            );
        }
    };
    let Some(context) = context.filter(|context| context.asker.attended()) else {
        cyber_core::log::info(
            "mcp",
            "Elicitation declined without an interactive call owner",
            json!({"server":server,"method":"elicitation/create"}),
        );
        return (json!({"result":{"action":"decline"}}), false);
    };
    if !(context.authorize)() {
        return (json!({"result":{"action":"cancel"}}), false);
    }
    match context.asker.question(form.questions.clone()).await {
        QuestionReply::Answers { answers } => (
            json!({"result":if (context.authorize)() {form.answer(&answers).unwrap_or_else(|_|json!({"action":"cancel"}))} else {json!({"action":"cancel"})}}),
            true,
        ),
        QuestionReply::Dismissed => (json!({"result":{"action":"cancel"}}), true),
        QuestionReply::Unattended => (json!({"result":{"action":"decline"}}), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(properties: Value, required: Value) -> Value {
        json!({"message":"Review these values","requestedSchema":{"type":"object","properties":properties,"required":required}})
    }

    #[test]
    fn forms_preserve_typed_values_optional_omission_titles_and_defaults() {
        let form = Form::new(&params(json!({
            "color":{"type":"string","oneOf":[{"const":"#fff","title":"White"},{"const":"#000","title":"Black"}]},
            "count":{"type":"integer","minimum":1,"maximum":10},
            "enabled":{"type":"boolean","default":false},
            "flags":{"type":"array","items":{"type":"string","anyOf":[{"const":"a","title":"A"},{"const":"b","title":"B"}]},"minItems":2,"uniqueItems":true},
            "note":{"type":"string"}
        }),json!(["color","count","enabled","flags"])),"db").unwrap();
        let mut answers: Vec<Vec<String>> = form
            .fields
            .iter()
            .map(|field| match field.name.as_str() {
                "color" => vec!["1. White".into()],
                "count" => vec!["3".into()],
                "enabled" => vec![DEFAULT.into()],
                "flags" => vec!["1. A".into(), "2. B".into()],
                _ => vec![OMIT.into()],
            })
            .collect();
        answers.push(vec!["Accept".into()]);
        assert_eq!(
            form.answer(&answers).unwrap(),
            json!({"action":"accept","content":{"color":"#fff","count":3,"enabled":false,"flags":["a","b"]}})
        );
        answers[1] = vec!["2.5".into()];
        assert!(form.answer(&answers).is_err());
        answers[1] = vec!["11".into()];
        assert!(form.answer(&answers).is_err());
        *answers.last_mut().unwrap() = vec!["Decline".into()];
        assert_eq!(form.answer(&answers).unwrap(), json!({"action":"decline"}));
        *answers.last_mut().unwrap() = vec!["Cancel".into()];
        assert_eq!(form.answer(&answers).unwrap(), json!({"action":"cancel"}));
    }

    #[test]
    fn forms_validate_patterns_formats_and_refuse_nested_or_external_schemas() {
        let form = Form::new(&params(json!({"email":{"type":"string","format":"email","minLength":3,"pattern":"^[a-z]"}}),json!(["email"])),"db").unwrap();
        assert!(
            form.answer(&[vec!["invalid".into()], vec!["Accept".into()]])
                .is_err()
        );
        assert!(
            form.answer(&[vec!["a@example.test".into()], vec!["Accept".into()]])
                .is_ok()
        );
        for schema in [
            json!({"type":"object","$ref":"file:///private-secret"}),
            json!({"type":"number","items":{"$ref":"https://example.invalid/private-secret"}}),
            json!({"type":"string","oneOf":[{"$ref":"file:///private-secret"}]}),
            json!({"type":"array","items":{"type":"object"}}),
            json!({"type":"string","format":"unknown"}),
        ] {
            assert!(Form::new(&params(json!({"field":schema}), json!([])), "db").is_err());
        }
        let mut external = params(json!({}), json!([]));
        external["requestedSchema"]["additionalProperties"] =
            json!({"$ref":"file:///private-secret"});
        assert!(Form::new(&external, "db").is_err());
        external = params(json!({}), json!([]));
        external["mode"] = json!("url");
        external["url"] = json!("https://example.invalid/private-secret");
        assert!(Form::new(&external, "db").is_err());
    }

    #[tokio::test]
    async fn unattended_requests_decline_and_invalid_requests_echo_no_payloads() {
        let (result, interacted) = reply(
            "db",
            &params(json!({"field":{"type":"string"}}), json!([])),
            None,
        )
        .await;
        assert_eq!(result, json!({"result":{"action":"decline"}}));
        assert!(!interacted);
        let (result, interacted) =
            reply("db", &json!({"mode":"url","url":"private-secret"}), None).await;
        assert_eq!(result["error"]["code"], -32602);
        assert!(!result.to_string().contains("private-secret"));
        assert!(!interacted);
    }
}
