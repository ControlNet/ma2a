use std::fmt;

use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
use serde_json::{Map, Value};

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
        while let Some((key, value)) = access.next_entry::<String, Value>()? {
            if fields.contains_key(&key) {
                duplicate_member = true;
                duplicate_version |= key == "version";
            } else {
                fields.insert(key, value);
            }
        }
        Ok(RootObject {
            fields,
            duplicate_member,
            duplicate_version,
        })
    }
}
