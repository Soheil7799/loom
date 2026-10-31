//! A question form as filled in so far (Claude Code's AskUserQuestion, an MCP server asking
//! for input): its fields from the request's schema, what is picked and written, what is sent.
//! A UI draws it and keeps it up to date; the Telegram bot and the phone fill it too.

use serde_json::Value;

pub struct FormModel {
    pub fields: Vec<Field>,
    /// The fields that must be answered.
    required: Vec<String>,
}

pub struct Field {
    pub key: String,
    pub title: String,
    pub description: String,
    pub kind: Kind,
}

pub enum Kind {
    /// One of the options (`multi`: any of them).
    Choice { multi: bool, options: Vec<Choice>, picked: Vec<bool> },
    /// `number`: Some(true) a whole number, Some(false) any number.
    Text { number: Option<bool>, text: String },
    Flag { on: bool },
}

pub struct Choice {
    /// What it stands for in the answer.
    pub value: Value,
    pub name: String,
    pub about: String,
    /// What it would look like (Claude Code's option previews).
    pub preview: String,
}

impl FormModel {
    pub fn from_schema(schema: &Value) -> FormModel {
        let mut fields = Vec::new();
        for (key, field) in schema["properties"].as_object().into_iter().flatten() {
            let title = field["title"].as_str().unwrap_or(key).to_owned();
            let description = field["description"].as_str().unwrap_or("").to_owned();
            let options = choices(field);
            let kind = if !options.is_empty() {
                let defaults: Vec<Value> = match &field["default"] {
                    Value::Array(all) => all.clone(),
                    Value::Null => Vec::new(),
                    one => vec![one.clone()],
                };
                let picked = options.iter().map(|o| defaults.contains(&o.value)).collect();
                Kind::Choice { multi: field["type"].as_str() == Some("array"), options, picked }
            } else if field["type"].as_str() == Some("boolean") {
                Kind::Flag { on: field["default"].as_bool().unwrap_or(false) }
            } else {
                let number = match field["type"].as_str() {
                    Some("integer") => Some(true),
                    Some("number") => Some(false),
                    _ => None,
                };
                let text = match &field["default"] {
                    Value::String(text) => text.clone(),
                    Value::Number(n) => n.to_string(),
                    _ => String::new(),
                };
                Kind::Text { number, text }
            };
            fields.push(Field { key: key.clone(), title, description, kind });
        }
        let required = schema["required"].as_array().into_iter().flatten().filter_map(|k| k.as_str().map(str::to_owned)).collect();
        FormModel { fields, required }
    }

    /// What was filled in, as the schema's object, or why it can't be sent (a required field
    /// left empty, a number that isn't one).
    pub fn content(&self) -> Result<Value, String> {
        let mut content = serde_json::Map::new();
        for field in &self.fields {
            match &field.kind {
                Kind::Choice { multi, options, picked } => {
                    let chosen: Vec<Value> = options.iter().zip(picked).filter(|(_, on)| **on).map(|(o, _)| o.value.clone()).collect();
                    if *multi {
                        if !chosen.is_empty() {
                            content.insert(field.key.clone(), Value::Array(chosen));
                        }
                    } else if let Some(one) = chosen.into_iter().next() {
                        content.insert(field.key.clone(), one);
                    }
                }
                Kind::Text { number, text } => {
                    let text = text.trim();
                    if text.is_empty() {
                        continue;
                    }
                    let title = &field.title;
                    let value = match number {
                        Some(true) => Value::from(text.parse::<i64>().map_err(|_| format!("“{title}” needs a whole number."))?),
                        Some(false) => Value::from(text.parse::<f64>().map_err(|_| format!("“{title}” needs a number."))?),
                        None => Value::String(text.to_owned()),
                    };
                    content.insert(field.key.clone(), value);
                }
                Kind::Flag { on } => {
                    content.insert(field.key.clone(), Value::Bool(*on));
                }
            }
        }
        if let Some(missing) = self.required.iter().find(|k| !content.contains_key(k.as_str())) {
            let title = self.fields.iter().find(|f| f.key == *missing).map_or(missing.as_str(), |f| f.title.as_str());
            return Err(format!("“{title}” needs an answer."));
        }
        Ok(Value::Object(content))
    }

    /// "→ Approach: Small steps · Other: keep the API", for the card once answered.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        for field in &self.fields {
            let title = &field.title;
            match &field.kind {
                Kind::Choice { options, picked, .. } => {
                    let chosen: Vec<&str> = options.iter().zip(picked).filter(|(_, on)| **on).map(|(o, _)| o.name.as_str()).collect();
                    if !chosen.is_empty() {
                        parts.push(format!("{title}: {}", chosen.join(", ")));
                    }
                }
                Kind::Text { text, .. } if !text.trim().is_empty() => parts.push(format!("{title}: {}", text.trim())),
                Kind::Flag { on } => parts.push(format!("{title}: {}", if *on { "yes" } else { "no" })),
                _ => {}
            }
        }
        if parts.is_empty() { "→ (nothing picked)".to_owned() } else { format!("→ {}", parts.join(" · ")) }
    }

    /// Its choices: (question, option names, several allowed, which are picked).
    pub fn choices(&self) -> Vec<(String, Vec<String>, bool, Vec<bool>)> {
        self.fields
            .iter()
            .filter_map(|f| match &f.kind {
                Kind::Choice { multi, options, picked } => Some((f.title.clone(), options.iter().map(|o| o.name.clone()).collect(), *multi, picked.clone())),
                _ => None,
            })
            .collect()
    }

    /// Option `option` of field `field` (counted among all fields) set on or off, as the UI saw
    /// it change. One choice of a kind: the others go off.
    pub fn set_picked(&mut self, field: usize, option: usize, on: bool) {
        let Some(Field { kind: Kind::Choice { multi, picked, .. }, .. }) = self.fields.get_mut(field) else { return };
        if option >= picked.len() {
            return;
        }
        if on && !*multi {
            picked.iter_mut().for_each(|p| *p = false);
        }
        picked[option] = on;
    }

    /// Option `option` of the `choice`-th question with options: picked, or toggled in a
    /// multiple choice. The field it is in (counted among all fields), if there was one.
    pub fn pick_in(&mut self, choice: usize, option: usize) -> Option<usize> {
        let field = self.fields.iter().enumerate().filter(|(_, f)| matches!(f.kind, Kind::Choice { .. })).nth(choice).map(|(i, _)| i)?;
        let Kind::Choice { multi, picked, .. } = &self.fields[field].kind else { return None };
        let on = if *multi { !*picked.get(option)? } else { true };
        if option >= picked.len() {
            return None;
        }
        self.set_picked(field, option, on);
        Some(field)
    }

    /// The text of field `field`, as the UI saw it change.
    pub fn set_text(&mut self, field: usize, value: &str) {
        if let Some(Field { kind: Kind::Text { text, .. }, .. }) = self.fields.get_mut(field) {
            *text = value.to_owned();
        }
    }

    pub fn set_flag(&mut self, field: usize, value: bool) {
        if let Some(Field { kind: Kind::Flag { on }, .. }) = self.fields.get_mut(field) {
            *on = value;
        }
    }

    /// One text field (more, and a written answer could not say which it is for).
    pub fn writable(&self) -> bool {
        self.fields.iter().filter(|f| matches!(f.kind, Kind::Text { .. })).count() == 1
    }

    /// `text` in its one text field; the field (counted among all fields), unless it has not
    /// exactly one.
    pub fn write(&mut self, text: &str) -> Option<usize> {
        if !self.writable() {
            return None;
        }
        let field = self.fields.iter().position(|f| matches!(f.kind, Kind::Text { .. }))?;
        self.set_text(field, text);
        Some(field)
    }
}

/// A field's options: (value, name, what it means, its preview), from `oneOf`/`anyOf` with
/// titles, or `enum` with `enumNames`; an array's from its items.
fn choices(field: &Value) -> Vec<Choice> {
    let source = if field["type"].as_str() == Some("array") { &field["items"] } else { field };
    let name_of = |v: &Value| v.as_str().map_or_else(|| v.to_string(), str::to_owned);
    for key in ["oneOf", "anyOf"] {
        if let Some(options) = source[key].as_array() {
            return options
                .iter()
                .filter(|o| !o["const"].is_null())
                .map(|o| {
                    let value = o["const"].clone();
                    let name = o["title"].as_str().map_or_else(|| name_of(&value), str::to_owned);
                    let preview = o["_meta"]["_claude/askUserQuestionOption"]["preview"].as_str().unwrap_or("").to_owned();
                    Choice { value, name, about: o["description"].as_str().unwrap_or("").to_owned(), preview }
                })
                .collect();
        }
    }
    let names = source["enumNames"].as_array();
    source["enum"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(i, v)| {
            let name = names.and_then(|n| n.get(i)).and_then(Value::as_str).map_or_else(|| name_of(v), str::to_owned);
            Choice { value: v.clone(), name, about: String::new(), preview: String::new() }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn choices_of(field: &Value) -> Vec<(Value, String, String)> {
        choices(field).into_iter().map(|c| (c.value, c.name, c.about)).collect()
    }

    #[test]
    fn choices_come_from_one_of_and_enum() {
        let single = serde_json::json!({ "type": "string", "oneOf": [{ "const": "a", "title": "Apple", "description": "red" }] });
        assert_eq!(choices_of(&single), vec![("a".into(), "Apple".into(), "red".into())]);
        let multi = serde_json::json!({ "type": "array", "items": { "anyOf": [{ "const": "x", "title": "X" }] } });
        assert_eq!(choices_of(&multi), vec![("x".into(), "X".into(), String::new())]);
        let plain = serde_json::json!({ "type": "string", "enum": ["s", "m"], "enumNames": ["Small", "Medium"] });
        assert_eq!(choices_of(&plain)[1], ("m".into(), "Medium".into(), String::new()));
        let numbers = serde_json::json!({ "type": "integer", "enum": [1, 2] });
        assert_eq!(choices_of(&numbers)[1], (serde_json::json!(2), "2".into(), String::new()));
    }

    fn form() -> FormModel {
        FormModel::from_schema(&json!({
            "properties": {
                "approach": { "type": "string", "title": "Approach", "oneOf": [
                    { "const": "small", "title": "Small steps" }, { "const": "big", "title": "One go" }
                ] },
                "parts": { "type": "array", "title": "Parts", "items": { "enum": ["ui", "core"] }, "default": ["core"] },
                "Other": { "type": "string", "title": "Other" },
                "count": { "type": "integer", "title": "Count" },
                "sure": { "type": "boolean", "title": "Sure" }
            },
            "required": ["approach"]
        }))
    }

    #[test]
    fn defaults_and_what_is_missing() {
        let form = form();
        assert_eq!(form.content(), Err("“Approach” needs an answer.".to_owned()));
        assert_eq!(form.choices()[1], ("Parts".to_owned(), vec!["ui".to_owned(), "core".to_owned()], true, vec![false, true]));
        assert!(!form.writable(), "two text fields: an answer can't say which");
    }

    #[test]
    fn picks_writes_and_sends() {
        let mut form = form();
        assert_eq!(form.pick_in(0, 1), Some(0));
        assert_eq!(form.pick_in(0, 0), Some(0));
        // One choice: the other went off. A multiple one toggles.
        assert_eq!(form.choices()[0].3, vec![true, false]);
        form.pick_in(1, 0);
        form.pick_in(1, 1);
        assert_eq!(form.choices()[1].3, vec![true, false]);
        form.set_text(2, " keep the API ");
        form.set_text(3, "x");
        assert_eq!(form.content(), Err("“Count” needs a whole number.".to_owned()));
        form.set_text(3, "3");
        form.set_flag(4, true);
        assert_eq!(form.content().unwrap(), json!({ "approach": "small", "parts": ["ui"], "Other": "keep the API", "count": 3, "sure": true }));
        assert_eq!(form.summary(), "→ Approach: Small steps · Parts: ui · Other: keep the API · Count: 3 · Sure: yes");
        assert_eq!(form.pick_in(9, 0), None);
        assert_eq!(form.pick_in(0, 9), None);
    }
}
