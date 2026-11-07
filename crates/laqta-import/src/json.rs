//! Readers for ShareX's JSON. ShareX writes PascalCase keys, and some numbers and flags as text, so
//! these accept both forms.

use serde_json::{Map, Value};

use crate::ImportError;

pub(crate) type Object = Map<String, Value>;

/// Drops the UTF-8 byte order that Windows tools put at the start of JSON files.
pub(crate) fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

pub(crate) fn parse_value(file: &str, text: &str) -> Result<Value, ImportError> {
    serde_json::from_str(strip_bom(text)).map_err(|source| ImportError::Json { file: file.to_owned(), source })
}

/// A file whose root must be an object.
pub(crate) fn root_object(file: &str, text: &str) -> Result<Object, ImportError> {
    match parse_value(file, text)? {
        Value::Object(object) => Ok(object),
        _ => Err(ImportError::Invalid(format!("{file} is not a JSON object"))),
    }
}

/// The non-blank text under `key`.
pub(crate) fn text<'a>(object: &'a Object, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str).filter(|text| !text.trim().is_empty())
}

/// A boolean under `key`, also when ShareX wrote it as `"true"` or `"false"`.
pub(crate) fn flag(object: &Object, key: &str) -> Option<bool> {
    match object.get(key)? {
        Value::Bool(flag) => Some(*flag),
        Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// A number under `key`, also when it is written as text.
pub(crate) fn number(object: &Object, key: &str) -> Option<f64> {
    match object.get(key)? {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

pub(crate) fn sub<'a>(object: &'a Object, key: &str) -> Option<&'a Object> {
    object.get(key).and_then(Value::as_object)
}

/// The array under `key`, empty when it is absent.
pub(crate) fn list<'a>(object: &'a Object, key: &str) -> &'a [Value] {
    match object.get(key).and_then(Value::as_array) {
        Some(items) => items.as_slice(),
        None => &[],
    }
}
