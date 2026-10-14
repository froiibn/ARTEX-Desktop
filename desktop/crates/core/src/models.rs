//! Generated v1 entity DTOs. Wire JSON remains unchanged; entity IDs are distinct Rust types.
use crate::{Error, Result};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{fmt, marker::PhantomData};
pub trait Entity: Serialize + DeserializeOwned + Clone + Send + Sync + 'static {
    const COLLECTION: &'static str;
    const PREFIX: &'static str;
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Id<T: Entity> {
    value: String,
    kind: PhantomData<T>,
}
impl<T: Entity> Id<T> {
    pub fn parse(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if !crate::valid_id(&value, T::PREFIX) {
            return Err(Error::new("INVALID_PARAMS", "Wrong entity ID type"));
        }
        Ok(Self {
            value,
            kind: PhantomData,
        })
    }
    pub fn generate() -> Self {
        Self::parse(crate::new_id(T::PREFIX)).unwrap()
    }
    pub fn as_str(&self) -> &str {
        &self.value
    }
}
impl<T: Entity> Serialize for Id<T> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.value)
    }
}
impl<'de, T: Entity> Deserialize<'de> for Id<T> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Self::parse(String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}
impl<T: Entity> fmt::Display for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.value)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct UInt64(pub u64);
impl Serialize for UInt64 {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.0.to_string())
    }
}
impl<'de> Deserialize<'de> for UInt64 {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        crate::parse_u64(&String::deserialize(d)?)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timestamp(String);
impl Timestamp {
    pub fn parse(s: impl Into<String>) -> Result<Self> {
        let s = s.into();
        if !s.ends_with('Z') || chrono::DateTime::parse_from_rfc3339(&s).is_err() {
            return Err(Error::new("INVALID_PARAMS", "Invalid UTC timestamp"));
        }
        Ok(Self(s))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl Serialize for Timestamp {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}
impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Self::parse(String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Digest(String);
impl Digest {
    pub fn parse(s: impl Into<String>) -> Result<Self> {
        let s = s.into();
        if s.len() != 64
            || !s
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::new("INVALID_PARAMS", "Invalid content digest"));
        }
        Ok(Self(s))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl Serialize for Digest {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}
impl<'de> Deserialize<'de> for Digest {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Self::parse(String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlobRef {
    pub sha256: Digest,
    pub length: UInt64,
    pub media_type: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretRef {
    pub secret_ref: String,
    pub hint: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Body {
    pub text: Option<String>,
    pub blob: Option<BlobRef>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(bound = "T: Entity", deny_unknown_fields)]
pub struct TypedRecord<T: Entity> {
    pub header: TypedHeader<T>,
    pub data: T,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(bound = "T: Entity", deny_unknown_fields)]
pub struct TypedHeader<T: Entity> {
    pub id: Id<T>,
    pub collection: String,
    pub version: UInt64,
    pub commit_seq: UInt64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub legacy_key: Value,
}
impl<T: Entity> TypedRecord<T> {
    pub fn try_from_record(record: &crate::Record) -> Result<Self> {
        if record.header.collection != T::COLLECTION {
            return Err(Error::new("INVALID_PARAMS", "Wrong typed collection"));
        }
        crate::validate_record(record)?;
        Ok(serde_json::from_value(serde_json::to_value(record)?)?)
    }
    pub fn into_record(self) -> Result<crate::Record> {
        let r = serde_json::from_value(serde_json::to_value(self)?)?;
        crate::validate_record(&r)?;
        Ok(r)
    }
}
include!(concat!(env!("OUT_DIR"), "/entities.rs"));
