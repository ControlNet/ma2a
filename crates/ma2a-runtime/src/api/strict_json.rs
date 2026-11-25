use std::fmt;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use super::ApiError;

pub(crate) struct RootObject {
    pub(crate) fields: Map<String, Value>,
    pub(crate) duplicate_member: bool,
    pub(crate) duplicate_version: bool,
}

pub(crate) fn decode_root_object(input: &[u8]) -> Result<RootObject, ApiError> {
    let mut deserializer = serde_json::Deserializer::from_slice(input);
    let object =
        RootObject::deserialize(&mut deserializer).map_err(|_| ApiError::invalid_input())?;
    deserializer.end().map_err(|_| ApiError::invalid_input())?;
    Ok(object)
}

impl<'de> Deserialize<'de> for RootObject {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(RootObjectVisitor)
    }
}

struct RootObjectVisitor;

impl<'de> Visitor<'de> for RootObjectVisitor {
    type Value = RootObject;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a local API request object")
    }

    fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut fields = Map::new();
        let mut duplicate_member = false;
        let mut duplicate_version = false;
        while let Some((key, value)) = access.next_entry::<String, StrictValue>()? {
            duplicate_member |= value.duplicate_member;
            if fields.contains_key(&key) {
                duplicate_member = true;
                duplicate_version |= key == "version";
            } else {
                fields.insert(key, value.value);
            }
        }
        Ok(RootObject {
            fields,
            duplicate_member,
            duplicate_version,
        })
    }
}

/// A JSON value paired with whether any nested object repeated a key.
struct StrictValue {
    value: Value,
    duplicate_member: bool,
}

impl StrictValue {
    const fn unique(value: Value) -> Self {
        Self {
            value,
            duplicate_member: false,
        }
    }
}

impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictValueVisitor)
    }
}

struct StrictValueVisitor;

impl<'de> Visitor<'de> for StrictValueVisitor {
    type Value = StrictValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("any valid JSON value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(StrictValue::unique(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(StrictValue::unique(Value::Number(value.into())))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(StrictValue::unique(Value::Number(value.into())))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        let number = Number::from_f64(value).ok_or_else(|| E::custom("non-finite JSON number"))?;
        Ok(StrictValue::unique(Value::Number(number)))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(StrictValue::unique(Value::String(value.to_owned())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(StrictValue::unique(Value::String(value)))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(StrictValue::unique(Value::Null))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(StrictValue::unique(Value::Null))
    }

    fn visit_seq<A>(self, mut access: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut items = Vec::new();
        let mut duplicate_member = false;
        while let Some(item) = access.next_element::<StrictValue>()? {
            duplicate_member |= item.duplicate_member;
            items.push(item.value);
        }
        Ok(StrictValue {
            value: Value::Array(items),
            duplicate_member,
        })
    }

    fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut fields = Map::new();
        let mut duplicate_member = false;
        while let Some((key, value)) = access.next_entry::<String, StrictValue>()? {
            duplicate_member |= value.duplicate_member;
            if fields.contains_key(&key) {
                duplicate_member = true;
            } else {
                fields.insert(key, value.value);
            }
        }
        Ok(StrictValue {
            value: Value::Object(fields),
            duplicate_member,
        })
    }
}
