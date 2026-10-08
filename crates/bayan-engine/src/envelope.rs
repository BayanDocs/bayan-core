//! The envelope of a protocol message ([engine protocol specification][spec] §4), read without building a tree of the whole message.
//!
//! A message can hold up to 16 MiB of JSON (spec §13), and an untrusted one can fill that with fields the engine does not know. Reading it into a generic JSON tree would cost 10 to 25 times its size, because every small value becomes an allocation of its own, and in WebAssembly that memory never shrinks again. So the envelope's four fields are read one by one, every other field is skipped without being stored, and the payload is kept as the text it was written as. [`Envelope::payload`] then reads that text straight into the message type's payload type, again skipping the fields the type does not know. A message thus costs about its own size once.
//!
//! [spec]: https://github.com/BayanDocs/docs/blob/HEAD/specs/engine-protocol.md

use std::fmt;

use serde::Deserialize;
use serde::de::{self, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::value::RawValue;

use crate::limits::{MAX_ID, MAX_MESSAGE_BYTES};
use crate::protocol::{ErrorCode, ErrorInfo, PROTOCOL_VERSION};

/// The payload of a message that has none.
const EMPTY_PAYLOAD: &str = "{}";

/// The longest message type an error repeats in its `args` (spec §12).
const MAX_TYPE_IN_ERROR: usize = 64;

/// Why a message is not a valid envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnvelopeError {
    /// Longer than the message limit (spec §13).
    TooLarge,
    /// Not JSON, not an object, the same field twice, or a field of the wrong type.
    Invalid,
    /// `v` is not the protocol version.
    UnsupportedVersion,
}

/// A message's envelope, checked (spec §4).
#[derive(Debug)]
pub(crate) struct Envelope<'a> {
    /// The request identifier, if the message has one.
    pub(crate) id: Option<u64>,
    /// The message type.
    pub(crate) kind: String,
    /// The payload as JSON text: an object.
    payload: &'a str,
}

impl<'a> Envelope<'a> {
    /// Reads an envelope. On failure, also returns the request identifier if one could be read, so the error can be the reply.
    pub(crate) fn parse(message: &'a [u8]) -> Result<Self, (Option<u64>, EnvelopeError)> {
        if message.len() > MAX_MESSAGE_BYTES {
            return Err((None, EnvelopeError::TooLarge));
        }
        let fields: Fields<'a> =
            serde_json::from_slice(message).map_err(|_| (None, EnvelopeError::Invalid))?;
        let id = match fields.id {
            None | Some(Field::Null) => None,
            Some(Field::Unsigned(id)) if (1..=MAX_ID).contains(&id) => Some(id),
            Some(_) => return Err((None, EnvelopeError::Invalid)),
        };
        if fields.v != Some(Field::Unsigned(u64::from(PROTOCOL_VERSION))) {
            return Err((id, EnvelopeError::UnsupportedVersion));
        }
        let Some(Field::Text(kind)) = fields.kind else {
            return Err((id, EnvelopeError::Invalid));
        };
        let payload = match fields.payload.map(RawValue::get) {
            None | Some("null") => EMPTY_PAYLOAD,
            Some(text) if text.starts_with('{') => text,
            Some(_) => return Err((id, EnvelopeError::Invalid)),
        };
        Ok(Self { id, kind, payload })
    }

    /// The payload as the message type's payload type `T`.
    ///
    /// # Errors
    ///
    /// `invalid_request` if the payload does not have `T`'s shape, including a field that appears twice.
    pub(crate) fn payload<T: Deserialize<'a>>(&self) -> Result<T, ErrorInfo> {
        serde_json::from_str(self.payload).map_err(|_| invalid_request(&self.kind))
    }
}

/// The error `invalid_request` for a message of type `kind`.
pub(crate) fn invalid_request(kind: &str) -> ErrorInfo {
    with_type(ErrorInfo::new(ErrorCode::InvalidRequest), kind)
}

/// Adds a message type to an error's `args` (spec §12), but only if it looks like a message type: 1 to 64 lower-case letters, digits, `_` and `.`. Any other text is left out, so an error never carries a string from the message back.
pub(crate) fn with_type(error: ErrorInfo, kind: &str) -> ErrorInfo {
    let looks_like_a_type = (1..=MAX_TYPE_IN_ERROR).contains(&kind.len())
        && kind.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_.".contains(&byte)
        });
    if looks_like_a_type {
        error.with_text("type", kind)
    } else {
        error
    }
}

/// The four fields of an envelope as they were written, each read only as far as the envelope's rules need.
struct Fields<'a> {
    v: Option<Field>,
    id: Option<Field>,
    kind: Option<Field>,
    payload: Option<&'a RawValue>,
}

impl<'de> Deserialize<'de> for Fields<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // A map, never a sequence: serde would otherwise also accept a JSON array as an envelope.
        deserializer.deserialize_map(FieldsVisitor)
    }
}

struct FieldsVisitor;

impl<'de> Visitor<'de> for FieldsVisitor {
    type Value = Fields<'de>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an envelope, a JSON object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Fields<'de>, A::Error> {
        let mut fields = Fields {
            v: None,
            id: None,
            kind: None,
            payload: None,
        };
        while let Some(key) = map.next_key::<Key>()? {
            match key {
                Key::V => first(&mut fields.v, map.next_value()?)?,
                Key::Id => first(&mut fields.id, map.next_value()?)?,
                Key::Type => first(&mut fields.kind, map.next_value()?)?,
                Key::Payload => first(&mut fields.payload, map.next_value()?)?,
                Key::Other => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(fields)
    }
}

/// Stores the value of a field seen for the first time. An envelope with the same field twice is invalid, because nobody can tell which of the two its sender meant.
fn first<T, E: de::Error>(slot: &mut Option<T>, value: T) -> Result<(), E> {
    if slot.is_some() {
        return Err(E::custom("a field appears twice"));
    }
    *slot = Some(value);
    Ok(())
}

/// A key of the envelope's object, compared after JSON escapes are undone.
enum Key {
    V,
    Id,
    Type,
    Payload,
    Other,
}

impl<'de> Deserialize<'de> for Key {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_identifier(KeyVisitor)
    }
}

struct KeyVisitor;

impl Visitor<'_> for KeyVisitor {
    type Value = Key;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a key")
    }

    fn visit_str<E: de::Error>(self, key: &str) -> Result<Key, E> {
        Ok(match key {
            "v" => Key::V,
            "id" => Key::Id,
            "type" => Key::Type,
            "payload" => Key::Payload,
            _ => Key::Other,
        })
    }
}

/// What a field of the envelope holds, as far as the envelope's rules need to know. Values of any other kind are skipped without being stored.
#[derive(Debug, PartialEq, Eq)]
enum Field {
    Null,
    /// A whole number from 0 to 2⁶⁴ − 1.
    Unsigned(u64),
    Text(String),
    /// Anything else: a negative or fractional number, a boolean, an array or an object.
    Other,
}

impl<'de> Deserialize<'de> for Field {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(FieldVisitor)
    }
}

struct FieldVisitor;

impl<'de> Visitor<'de> for FieldVisitor {
    type Value = Field;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Field, E> {
        Ok(Field::Null)
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Field, E> {
        Ok(Field::Unsigned(value))
    }

    fn visit_i64<E: de::Error>(self, _value: i64) -> Result<Field, E> {
        Ok(Field::Other)
    }

    fn visit_f64<E: de::Error>(self, _value: f64) -> Result<Field, E> {
        Ok(Field::Other)
    }

    fn visit_bool<E: de::Error>(self, _value: bool) -> Result<Field, E> {
        Ok(Field::Other)
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Field, E> {
        Ok(Field::Text(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Field, E> {
        Ok(Field::Text(value))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Field, A::Error> {
        while sequence.next_element::<IgnoredAny>()?.is_some() {}
        Ok(Field::Other)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Field, A::Error> {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(Field::Other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// What reading a message gives: its identifier, type and payload, or why it is not an envelope.
    type Outcome = Result<(Option<u64>, String, Value), (Option<u64>, EnvelopeError)>;

    /// The envelope reader that built a JSON tree of the whole message, before it was replaced (review of CORE-007, B1). The new reader must agree with it on every message without a repeated field and without nesting deeper than serde_json's limit of 128.
    fn reference(message: &[u8]) -> Outcome {
        if message.len() > MAX_MESSAGE_BYTES {
            return Err((None, EnvelopeError::TooLarge));
        }
        let value: Value =
            serde_json::from_slice(message).map_err(|_| (None, EnvelopeError::Invalid))?;
        let Value::Object(mut fields) = value else {
            return Err((None, EnvelopeError::Invalid));
        };
        let id = match fields.remove("id") {
            None | Some(Value::Null) => None,
            Some(value) => match value.as_u64() {
                Some(id) if (1..=MAX_ID).contains(&id) => Some(id),
                _ => return Err((None, EnvelopeError::Invalid)),
            },
        };
        if fields.get("v").and_then(Value::as_u64) != Some(u64::from(PROTOCOL_VERSION)) {
            return Err((id, EnvelopeError::UnsupportedVersion));
        }
        let Some(Value::String(kind)) = fields.remove("type") else {
            return Err((id, EnvelopeError::Invalid));
        };
        let payload = match fields.remove("payload") {
            None | Some(Value::Null) => Value::Object(serde_json::Map::new()),
            Some(payload @ Value::Object(_)) => payload,
            Some(_) => return Err((id, EnvelopeError::Invalid)),
        };
        Ok((id, kind, payload))
    }

    /// What the new reader gives, in the reference's terms.
    fn current(message: &[u8]) -> Outcome {
        let envelope = Envelope::parse(message)?;
        let payload = serde_json::from_str(envelope.payload).unwrap();
        Ok((envelope.id, envelope.kind, payload))
    }

    #[test]
    fn agrees_with_the_reader_it_replaced() {
        // Every combination of these values for the four fields and one unknown field; `None` leaves the field out.
        let values = [
            None,
            Some("null"),
            Some("0"),
            Some("1"),
            Some("-1"),
            Some("-0"),
            Some("0.0"),
            Some("1e3"),
            Some("9007199254740991"),
            Some("9007199254740992"),
            Some("18446744073709551616"),
            Some("true"),
            Some(r#""hello""#),
            Some(r#""ui.manifest""#),
            Some("[]"),
            Some("[1,[2,{\"a\":[]}]]"),
            Some("{}"),
            Some(r#"{"protocol_versions":[0],"junk":[[[]]]}"#),
        ];
        let mut checked = 0;
        for v in values {
            for id in values {
                for kind in values {
                    for payload in values {
                        for extra in [None, Some("[]"), Some(r#"{"a":"\u0000"}"#)] {
                            let mut members = Vec::new();
                            for (key, value) in [
                                ("v", v),
                                ("id", id),
                                ("type", kind),
                                ("payload", payload),
                                ("x", extra),
                            ] {
                                if let Some(value) = value {
                                    members.push(format!(r#""{key}":{value}"#));
                                }
                            }
                            let message = format!("{{{}}}", members.join(","));
                            assert_eq!(
                                current(message.as_bytes()),
                                reference(message.as_bytes()),
                                "{message}"
                            );
                            checked += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(checked, 18 * 18 * 18 * 18 * 3);
    }

    #[test]
    fn agrees_on_whole_messages_that_are_not_envelopes() {
        let messages: [&[u8]; 14] = [
            b"",
            b"   ",
            b"[]",
            b"[0,1,\"hello\",{}]",
            b"\"envelope\"",
            b"17",
            b"null",
            b"not json",
            b"{",
            b"{\"v\":0,\"type\":\"x\"} trailing",
            b"{\"v\":0,\"type\":\"x\",}",
            b" {\"v\" : 0 , \"type\" : \"x\" , \"payload\" : {} } ",
            b"{\"\\u0076\":0,\"\\u0074ype\":\"x\",\"payload\":{\"a\":1}}",
            b"{\"v\":0,\"type\":\"\\ud800\"}",
        ];
        for message in messages {
            assert_eq!(
                current(message),
                reference(message),
                "{}",
                String::from_utf8_lossy(message)
            );
        }
        // Not UTF-8.
        let latin1 = b"{\"v\":0,\"type\":\"caf\xe9\"}";
        assert_eq!(current(latin1), reference(latin1));
    }

    #[test]
    fn parses_envelopes_strictly() {
        let ok = Envelope::parse(br#"{"v":0,"id":3,"type":"hello","payload":{"a":1}}"#).unwrap();
        assert_eq!(
            (ok.id, ok.kind.as_str(), ok.payload),
            (Some(3), "hello", r#"{"a":1}"#)
        );
        let empty = Envelope::parse(br#"{"v":0,"type":"x","payload":null}"#).unwrap();
        assert_eq!((empty.id, empty.payload), (None, "{}"));
        let cases: [(&[u8], Option<u64>, EnvelopeError); 8] = [
            (b"[]", None, EnvelopeError::Invalid),
            (b"not json", None, EnvelopeError::Invalid),
            (
                br#"{"v":0,"id":0,"type":"x"}"#,
                None,
                EnvelopeError::Invalid,
            ),
            (
                br#"{"v":0,"id":9007199254740992,"type":"x"}"#,
                None,
                EnvelopeError::Invalid,
            ),
            (
                br#"{"v":1,"id":4,"type":"x"}"#,
                Some(4),
                EnvelopeError::UnsupportedVersion,
            ),
            (br#"{"v":0,"id":5}"#, Some(5), EnvelopeError::Invalid),
            (
                br#"{"v":0,"id":6,"type":"x","payload":[]}"#,
                Some(6),
                EnvelopeError::Invalid,
            ),
            (
                br#"{"v":0,"id":7,"type":7}"#,
                Some(7),
                EnvelopeError::Invalid,
            ),
        ];
        for (message, id, error) in cases {
            assert_eq!(
                Envelope::parse(message).unwrap_err(),
                (id, error),
                "{}",
                String::from_utf8_lossy(message)
            );
        }
        let too_long = vec![b' '; MAX_MESSAGE_BYTES + 1];
        assert_eq!(
            Envelope::parse(&too_long).unwrap_err(),
            (None, EnvelopeError::TooLarge)
        );
    }

    #[test]
    fn a_field_twice_makes_the_envelope_invalid() {
        for message in [
            r#"{"v":0,"v":0,"type":"x"}"#,
            r#"{"v":0,"id":1,"id":2,"type":"x"}"#,
            r#"{"v":0,"type":"x","type":"y"}"#,
            r#"{"v":0,"type":"x","payload":{},"payload":{}}"#,
            r#"{"v":0,"type":"x","\u0074ype":"y"}"#,
        ] {
            assert_eq!(
                Envelope::parse(message.as_bytes()).unwrap_err(),
                (None, EnvelopeError::Invalid),
                "{message}"
            );
        }
        // Unknown fields may repeat: they are never read.
        assert!(Envelope::parse(br#"{"v":0,"type":"x","a":1,"a":2}"#).is_ok());
    }

    #[test]
    fn unknown_fields_are_skipped_at_any_depth() {
        // The tree reader stopped at serde_json's nesting limit of 128; skipping needs no limit, because it does not recurse and stores nothing.
        let deep = format!("{}{}", "[".repeat(10_000), "]".repeat(10_000));
        let message = format!(r#"{{"v":0,"type":"x","junk":{deep},"payload":{{"junk":{deep}}}}}"#);
        let envelope = Envelope::parse(message.as_bytes()).unwrap();
        assert_eq!(envelope.kind, "x");
        let payload: crate::protocol::Empty = envelope.payload().unwrap();
        assert_eq!(payload, crate::protocol::Empty {});
    }

    #[test]
    fn a_payload_with_a_field_twice_is_an_invalid_request() {
        let message = br#"{"v":0,"type":"doc.close","payload":{"doc_id":1,"doc_id":2}}"#;
        let envelope = Envelope::parse(message).unwrap();
        let error = envelope.payload::<crate::protocol::DocClose>().unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn errors_repeat_only_types_that_look_like_message_types() {
        let type_of = |kind: &str| invalid_request(kind).args.get("type").cloned();
        let text = |kind: &str| Some(crate::protocol::ArgValue::Text(kind.to_owned()));
        assert_eq!(type_of("doc.open"), text("doc.open"));
        assert_eq!(type_of("x_1.y"), text("x_1.y"));
        assert_eq!(type_of(&"a".repeat(64)), text(&"a".repeat(64)));
        for refused in [
            "",
            "Doc.open",
            "doc open",
            "doc\"open",
            "doc-open",
            "dokumént",
            &"a".repeat(65),
        ] {
            assert_eq!(type_of(refused), None, "{refused}");
        }
    }
}
