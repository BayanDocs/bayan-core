//! Values stored in property maps, identifier lists and marks.

use std::collections::BTreeMap;

use loro::{LoroListValue, LoroMapValue, LoroValue};

use crate::CrdtError;

/// The deepest nesting of lists and maps that a [`Value`] may have, counting a scalar as depth 1.
///
/// BayanDocs stores flat property values (now and then a short list or a map of scalars), so anything deeper is malformed or hostile. Values read from the CRDT are converted with this limit, so a deeply nested value received from another replica can never make conversion recurse deeply; deeper parts become [`Value::Unsupported`].
pub const MAX_VALUE_DEPTH: usize = 16;

/// A value in a property map, an identifier list or a mark: the BayanDocs-side mirror of the CRDT library's values.
///
/// Floating-point numbers are not supported (the model stores measurements as integers, ADR-0005), and neither are references to containers; both read as [`Value::Unsupported`], which consumers treat like a missing value.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Value {
    /// No value (also what an unmarked range carries).
    Null,
    /// A boolean.
    Bool(bool),
    /// A signed integer.
    Int(i64),
    /// A string.
    Str(String),
    /// Raw bytes.
    Bytes(Vec<u8>),
    /// A list of values.
    List(Vec<Self>),
    /// A map from keys to values, in key order.
    Map(BTreeMap<String, Self>),
    /// Something the adapter does not accept: a floating-point number, a container reference, or nesting deeper than [`MAX_VALUE_DEPTH`].
    Unsupported,
}

impl Value {
    /// The string, if this is one.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(text) => Some(text),
            _ => None,
        }
    }

    /// The boolean, if this is one.
    #[must_use]
    pub const fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// The integer, if this is one.
    #[must_use]
    pub const fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(value) => Some(*value),
            _ => None,
        }
    }

    /// Whether this is [`Value::Null`].
    #[must_use]
    pub const fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// The nesting depth: 1 for a scalar, one more than the deepest element for a list or map.
    #[must_use]
    pub fn depth(&self) -> usize {
        // Iterative, so that even a value built by mistake with very deep nesting cannot exhaust the stack here.
        let mut deepest = 0;
        let mut pending = vec![(self, 1_usize)];
        while let Some((value, depth)) = pending.pop() {
            deepest = deepest.max(depth);
            match value {
                Self::List(items) => pending.extend(items.iter().map(|item| (item, depth + 1))),
                Self::Map(entries) => {
                    pending.extend(entries.values().map(|item| (item, depth + 1)));
                }
                _ => {}
            }
        }
        deepest
    }

    /// Whether the adapter can store this value: it holds no [`Value::Unsupported`] anywhere and is nested no deeper than [`MAX_VALUE_DEPTH`]. A value read from the CRDT may not be, because another replica can store what this adapter refuses (a floating-point number, a container reference, a deeply nested value); code that copies stored values must skip those, or the copy fails halfway.
    #[must_use]
    pub fn is_storable(&self) -> bool {
        // Iterative, like `depth`, and stops at the first level beyond the limit.
        let mut pending = vec![(self, 1_usize)];
        while let Some((value, depth)) = pending.pop() {
            if depth > MAX_VALUE_DEPTH {
                return false;
            }
            match value {
                Self::Unsupported => return false,
                Self::List(items) => pending.extend(items.iter().map(|item| (item, depth + 1))),
                Self::Map(entries) => {
                    pending.extend(entries.values().map(|item| (item, depth + 1)));
                }
                _ => {}
            }
        }
        true
    }

    /// Converts a CRDT value, replacing anything unsupported or nested deeper than [`MAX_VALUE_DEPTH`] by [`Value::Unsupported`].
    pub(crate) fn from_loro(value: &LoroValue) -> Self {
        Self::from_loro_at(value, 1)
    }

    // The recursion is bounded by MAX_VALUE_DEPTH, whatever the input: a list or map whose elements would lie deeper than the limit becomes `Unsupported` itself, so the result is never deeper than MAX_VALUE_DEPTH.
    fn from_loro_at(value: &LoroValue, depth: usize) -> Self {
        match value {
            LoroValue::Null => Self::Null,
            LoroValue::Bool(value) => Self::Bool(*value),
            LoroValue::I64(value) => Self::Int(*value),
            LoroValue::String(text) => Self::Str(text.to_string()),
            LoroValue::Binary(bytes) => Self::Bytes(bytes.to_vec()),
            LoroValue::List(items) if depth < MAX_VALUE_DEPTH => Self::List(
                items
                    .iter()
                    .map(|item| Self::from_loro_at(item, depth + 1))
                    .collect(),
            ),
            LoroValue::Map(entries) if depth < MAX_VALUE_DEPTH => Self::Map(
                entries
                    .iter()
                    .map(|(key, item)| (key.clone(), Self::from_loro_at(item, depth + 1)))
                    .collect(),
            ),
            LoroValue::List(_)
            | LoroValue::Map(_)
            | LoroValue::Double(_)
            | LoroValue::Container(_) => Self::Unsupported,
        }
    }

    /// Converts to a CRDT value.
    ///
    /// # Errors
    ///
    /// [`CrdtError::InvalidValue`] when the value is [`Value::Unsupported`] or nested deeper than [`MAX_VALUE_DEPTH`].
    pub(crate) fn to_loro(&self) -> Result<LoroValue, CrdtError> {
        if self.depth() > MAX_VALUE_DEPTH {
            return Err(CrdtError::InvalidValue("nested too deeply"));
        }
        self.to_loro_unchecked()
    }

    // Only called after the depth check, so the recursion is bounded.
    fn to_loro_unchecked(&self) -> Result<LoroValue, CrdtError> {
        Ok(match self {
            Self::Null => LoroValue::Null,
            Self::Bool(value) => LoroValue::Bool(*value),
            Self::Int(value) => LoroValue::I64(*value),
            Self::Str(text) => LoroValue::from(text.as_str()),
            Self::Bytes(bytes) => LoroValue::from(bytes.clone()),
            Self::List(items) => LoroValue::List(
                items
                    .iter()
                    .map(Self::to_loro_unchecked)
                    .collect::<Result<LoroListValue, CrdtError>>()?,
            ),
            Self::Map(entries) => LoroValue::Map(
                entries
                    .iter()
                    .map(|(key, item)| Ok((key.clone(), item.to_loro_unchecked()?)))
                    .collect::<Result<LoroMapValue, CrdtError>>()?,
            ),
            Self::Unsupported => return Err(CrdtError::InvalidValue("unsupported value")),
        })
    }
}

impl From<&str> for Value {
    fn from(text: &str) -> Self {
        Self::Str(text.to_owned())
    }
}

impl From<String> for Value {
    fn from(text: String) -> Self {
        Self::Str(text)
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

/// Whether a CRDT value is nested deeper than `limit`, found without recursion and stopping at the first level beyond the limit. Used to inspect values received from other replicas, which may be nested arbitrarily deeply.
pub(crate) fn loro_depth_exceeds(value: &LoroValue, limit: usize) -> bool {
    let mut pending = vec![(value, 1_usize)];
    while let Some((value, depth)) = pending.pop() {
        if depth > limit {
            return true;
        }
        match value {
            LoroValue::List(items) => pending.extend(items.iter().map(|item| (item, depth + 1))),
            LoroValue::Map(entries) => {
                pending.extend(entries.values().map(|item| (item, depth + 1)))
            }
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nested(depth: usize) -> Value {
        let mut value = Value::Int(1);
        for _ in 1..depth {
            value = Value::List(vec![value]);
        }
        value
    }

    #[test]
    fn round_trips_every_supported_kind() {
        let value = Value::Map(BTreeMap::from([
            ("null".to_owned(), Value::Null),
            ("bool".to_owned(), Value::Bool(true)),
            ("int".to_owned(), Value::Int(-7)),
            ("str".to_owned(), Value::from("مرحبا")),
            ("bytes".to_owned(), Value::Bytes(vec![0, 255])),
            (
                "list".to_owned(),
                Value::List(vec![Value::Int(1), Value::from("x")]),
            ),
        ]));
        let loro = value.to_loro().unwrap();
        assert_eq!(Value::from_loro(&loro), value);
    }

    #[test]
    fn measures_depth_without_recursion() {
        assert_eq!(Value::Int(1).depth(), 1);
        assert_eq!(nested(MAX_VALUE_DEPTH).depth(), MAX_VALUE_DEPTH);
        assert!(nested(MAX_VALUE_DEPTH).to_loro().is_ok());
        assert!(nested(MAX_VALUE_DEPTH + 1).to_loro().is_err());
        assert!(Value::Unsupported.to_loro().is_err());
    }

    #[test]
    fn storable_values_are_exactly_those_that_convert() {
        let values = [
            Value::Null,
            Value::Int(3),
            Value::Unsupported,
            Value::List(vec![Value::Int(1), Value::Unsupported]),
            Value::Map(BTreeMap::from([("x".to_owned(), Value::Unsupported)])),
            Value::Map(BTreeMap::from([("x".to_owned(), Value::from("y"))])),
            nested(MAX_VALUE_DEPTH),
            nested(MAX_VALUE_DEPTH + 1),
        ];
        for value in values {
            assert_eq!(value.is_storable(), value.to_loro().is_ok(), "{value:?}");
        }
        assert!(!Value::List(vec![Value::Unsupported]).is_storable());
        assert!(nested(MAX_VALUE_DEPTH).is_storable());
    }

    #[test]
    fn reads_too_deep_and_unsupported_values_as_unsupported() {
        let mut loro = LoroValue::I64(1);
        for _ in 0..MAX_VALUE_DEPTH {
            loro = LoroValue::List(vec![loro].into());
        }
        let converted = Value::from_loro(&loro);
        let mut innermost = &converted;
        let mut levels = 1;
        while let Value::List(items) = innermost {
            innermost = &items[0];
            levels += 1;
        }
        assert_eq!(innermost, &Value::Unsupported);
        assert_eq!(levels, MAX_VALUE_DEPTH);
        assert_eq!(converted.depth(), MAX_VALUE_DEPTH);
        assert!(loro_depth_exceeds(&loro, MAX_VALUE_DEPTH));
        assert!(!loro_depth_exceeds(&loro, MAX_VALUE_DEPTH + 1));
        assert_eq!(
            Value::from_loro(&LoroValue::Double(0.5)),
            Value::Unsupported
        );
    }
}
