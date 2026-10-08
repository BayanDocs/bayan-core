//! A JSON Schema validator for exactly the part of draft 2020-12 that the protocol's schema uses.
//!
//! It refuses every keyword it does not know instead of ignoring it, so a constraint can never be skipped silently; extending the schema with a new kind of constraint therefore means extending this validator first. Annotations (`description`, `default`, `title`, `$schema`, `$id` and `x-…`) are ignored, as the standard says. Integers must be written as JSON integers, which is stricter than JSON Schema (it also accepts `1.0`) and matches what the engine accepts.

use serde_json::{Map, Value};

/// Validates values against one schema document.
pub struct Validator<'a> {
    root: &'a Value,
}

/// The keywords this validator checks.
const CHECKED: [&str; 14] = [
    "$ref",
    "type",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "const",
    "enum",
    "oneOf",
    "anyOf",
    "minimum",
    "maximum",
    "format",
    "$defs",
];

/// Keywords that annotate and constrain nothing.
const ANNOTATIONS: [&str; 6] = [
    "description",
    "default",
    "title",
    "examples",
    "$schema",
    "$id",
];

impl<'a> Validator<'a> {
    /// A validator for a schema document.
    pub fn new(root: &'a Value) -> Self {
        Self { root }
    }

    /// Validates a value against the document's root schema.
    pub fn validate(&self, value: &Value) -> Result<(), String> {
        self.check(self.root, value, "$")
    }

    /// Validates a value against the definition `name` of the document.
    pub fn validate_definition(&self, name: &str, value: &Value) -> Result<(), String> {
        let schema = self.definition(name)?;
        self.check(schema, value, "$")
    }

    fn definition(&self, name: &str) -> Result<&'a Value, String> {
        self.root
            .get("$defs")
            .and_then(|definitions| definitions.get(name))
            .ok_or_else(|| format!("no definition {name}"))
    }

    fn resolve(&self, reference: &str) -> Result<&'a Value, String> {
        let name = reference
            .strip_prefix("#/$defs/")
            .ok_or_else(|| format!("unsupported reference {reference}"))?;
        self.definition(name)
    }

    fn check(&self, schema: &Value, value: &Value, at: &str) -> Result<(), String> {
        let object = match schema {
            Value::Bool(true) => return Ok(()),
            Value::Bool(false) => return Err(format!("{at}: nothing is allowed here")),
            Value::Object(object) => object,
            _ => {
                return Err(format!(
                    "{at}: the schema {schema} is not an object or a boolean"
                ));
            }
        };
        for key in object.keys() {
            let known = CHECKED.contains(&key.as_str())
                || ANNOTATIONS.contains(&key.as_str())
                || key.starts_with("x-");
            if !known {
                return Err(format!(
                    "{at}: the validator does not know the keyword {key}"
                ));
            }
        }
        if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
            self.check(self.resolve(reference)?, value, at)?;
        }
        if let Some(kind) = object.get("type") {
            check_type(kind, value, at)?;
        }
        if let Some(constant) = object.get("const")
            && constant != value
        {
            return Err(format!("{at}: expected {constant}, found {value}"));
        }
        if let Some(values) = object.get("enum").and_then(Value::as_array)
            && !values.contains(value)
        {
            return Err(format!("{at}: {value} is not one of {values:?}"));
        }
        self.check_number(object, value, at)?;
        self.check_object(object, value, at)?;
        if let (Some(items), Value::Array(elements)) = (object.get("items"), value) {
            for (index, element) in elements.iter().enumerate() {
                self.check(items, element, &format!("{at}[{index}]"))?;
            }
        }
        if let Some(members) = object.get("oneOf").and_then(Value::as_array) {
            let matching = members
                .iter()
                .filter(|member| self.check(member, value, at).is_ok())
                .count();
            if matching != 1 {
                return Err(format!(
                    "{at}: {value} matches {matching} of the oneOf alternatives, not exactly one"
                ));
            }
        }
        if let Some(members) = object.get("anyOf").and_then(Value::as_array)
            && !members
                .iter()
                .any(|member| self.check(member, value, at).is_ok())
        {
            return Err(format!(
                "{at}: {value} matches none of the anyOf alternatives"
            ));
        }
        Ok(())
    }

    fn check_number(
        &self,
        object: &Map<String, Value>,
        value: &Value,
        at: &str,
    ) -> Result<(), String> {
        let Some(number) = value.as_f64() else {
            return Ok(());
        };
        if let Some(minimum) = object.get("minimum").and_then(Value::as_f64)
            && number < minimum
        {
            return Err(format!("{at}: {value} is below the minimum {minimum}"));
        }
        if let Some(maximum) = object.get("maximum").and_then(Value::as_f64)
            && number > maximum
        {
            return Err(format!("{at}: {value} is above the maximum {maximum}"));
        }
        if let Some(format) = object.get("format").and_then(Value::as_str) {
            let range: Option<(i128, i128)> = match format {
                "uint8" => Some((0, i128::from(u8::MAX))),
                "uint16" => Some((0, i128::from(u16::MAX))),
                "uint32" => Some((0, i128::from(u32::MAX))),
                "uint64" => Some((0, i128::from(u64::MAX))),
                "int8" => Some((i128::from(i8::MIN), i128::from(i8::MAX))),
                "int16" => Some((i128::from(i16::MIN), i128::from(i16::MAX))),
                "int32" => Some((i128::from(i32::MIN), i128::from(i32::MAX))),
                "int64" => Some((i128::from(i64::MIN), i128::from(i64::MAX))),
                "double" | "float" => None,
                other => {
                    return Err(format!(
                        "{at}: the validator does not know the format {other}"
                    ));
                }
            };
            if let Some((low, high)) = range {
                let integer = value
                    .as_i64()
                    .map(i128::from)
                    .or_else(|| value.as_u64().map(i128::from))
                    .ok_or_else(|| format!("{at}: {value} is not an integer"))?;
                if integer < low || integer > high {
                    return Err(format!("{at}: {value} does not fit the format {format}"));
                }
            }
        }
        Ok(())
    }

    fn check_object(
        &self,
        object: &Map<String, Value>,
        value: &Value,
        at: &str,
    ) -> Result<(), String> {
        let Value::Object(fields) = value else {
            return Ok(());
        };
        if let Some(required) = object.get("required").and_then(Value::as_array) {
            for name in required.iter().filter_map(Value::as_str) {
                if !fields.contains_key(name) {
                    return Err(format!("{at}: the required field {name} is missing"));
                }
            }
        }
        let properties = object.get("properties").and_then(Value::as_object);
        for (name, field) in fields {
            let path = format!("{at}.{name}");
            match properties.and_then(|properties| properties.get(name)) {
                Some(schema) => self.check(schema, field, &path)?,
                None => {
                    if let Some(additional) = object.get("additionalProperties") {
                        self.check(additional, field, &path)?;
                    }
                }
            }
        }
        Ok(())
    }
}

fn check_type(kind: &Value, value: &Value, at: &str) -> Result<(), String> {
    let kinds: Vec<&str> = match kind {
        Value::String(kind) => vec![kind.as_str()],
        Value::Array(kinds) => kinds.iter().filter_map(Value::as_str).collect(),
        _ => return Err(format!("{at}: the type {kind} is not a string or a list")),
    };
    let matches = kinds.iter().any(|kind| match *kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "number" => value.is_number(),
        "integer" => value.is_i64() || value.is_u64(),
        _ => false,
    });
    if matches {
        Ok(())
    } else {
        Err(format!("{at}: {value} is not of type {kind}"))
    }
}
