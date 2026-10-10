use std::collections::BTreeMap;

use serde_json::{Value, json, value::RawValue};

use super::{Mode, ToolError, failed};

type Fields = BTreeMap<String, Box<RawValue>>;

/// Opaque notebook values must never round-trip through a floating-point JSON number.
pub(super) struct Document {
    fields: Fields,
    cells: Vec<Box<RawValue>>,
}

impl Document {
    pub fn parse(bytes: &[u8]) -> Result<(Self, Value), ToolError> {
        let mut fields: Fields = serde_json::from_slice(bytes)
            .map_err(|_| failed("Notebook must contain a valid UTF-8 JSON object"))?;
        let cells = fields
            .remove("cells")
            .ok_or_else(|| failed("Notebook cells must be an array"))?;
        let cells: Vec<Box<RawValue>> = serde_json::from_str(cells.get())
            .map_err(|_| failed("Notebook cells must be an array"))?;
        let view = json!({
            "nbformat":typed(&fields, "nbformat")?,
            "nbformat_minor":typed(&fields, "nbformat_minor")?,
            "metadata":object_marker(&fields, "metadata"),
            "cells":cells.iter().map(|cell| cell_view(cell)).collect::<Result<Vec<_>, _>>()?,
        });
        Ok((Self { fields, cells }, view))
    }

    pub fn render(
        mut self,
        updated: &Value,
        mode: Mode,
        index: usize,
    ) -> Result<String, ToolError> {
        match mode {
            Mode::Delete => {
                self.cells.remove(index);
            }
            Mode::Insert => self.cells.insert(index, raw(&updated["cells"][index])?),
            Mode::Replace => {
                let mut cell: Fields = serde_json::from_str(self.cells[index].get())
                    .map_err(|_| failed("Invalid notebook cell"))?;
                let updated = &updated["cells"][index];
                for name in ["cell_type", "source", "outputs", "execution_count"] {
                    match updated.get(name) {
                        Some(value) => {
                            cell.insert(name.into(), raw(value)?);
                        }
                        None => {
                            cell.remove(name);
                        }
                    }
                }
                if updated["cell_type"] == "code" {
                    cell.remove("attachments");
                }
                self.cells[index] = raw(&cell)?;
            }
        }
        self.fields.insert("cells".into(), raw(&self.cells)?);
        serde_json::to_string_pretty(&self.fields).map_err(|_| failed("Could not encode notebook"))
    }
}

fn raw(value: &impl serde::Serialize) -> Result<Box<RawValue>, ToolError> {
    serde_json::value::to_raw_value(value).map_err(|_| failed("Could not encode notebook"))
}

fn typed(fields: &Fields, name: &str) -> Result<Value, ToolError> {
    fields
        .get(name)
        .map(|value| {
            serde_json::from_str(value.get()).map_err(|_| failed("Invalid notebook field"))
        })
        .transpose()
        .map(|value| value.unwrap_or(Value::Null))
}

fn object_marker(fields: &Fields, name: &str) -> Value {
    if fields
        .get(name)
        .is_some_and(|value| value.get().trim_start().starts_with('{'))
    {
        json!({})
    } else {
        Value::Null
    }
}

fn cell_view(cell: &RawValue) -> Result<Value, ToolError> {
    let fields: Fields =
        serde_json::from_str(cell.get()).map_err(|_| failed("Notebook cells must be objects"))?;
    let mut view = json!({"cell_type":typed(&fields, "cell_type")?,"source":typed(&fields, "source")?,"metadata":object_marker(&fields, "metadata")});
    if fields.contains_key("id") {
        view["id"] = typed(&fields, "id")?;
    }
    if let Some(outputs) = fields.get("outputs") {
        view["outputs"] = output_marker(outputs);
    }
    if let Some(count) = fields.get("execution_count") {
        view["execution_count"] = count_marker(count);
    }
    Ok(view)
}

fn output_marker(outputs: &RawValue) -> Value {
    let Some(contents) = outputs.get().trim().strip_prefix('[') else {
        return Value::Null;
    };
    if contents.trim_start().starts_with(']') {
        json!([])
    } else {
        json!([null])
    }
}

fn count_marker(count: &RawValue) -> Value {
    let count = count.get().trim();
    if count == "null" {
        return Value::Null;
    }
    let digits = count.strip_prefix('-').unwrap_or(count);
    if !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()) {
        json!(0)
    } else {
        json!(false)
    }
}
