// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use std::fmt;
use std::fmt::{Debug, Display, Formatter};
use std::marker::PhantomData;
use std::ops::Deref;
use std::str::FromStr;

use fastcrypto::encoding::Hex;
use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::{StructTag, TypeTag};
use schemars::JsonSchema;
use serde;
use serde::de::{Deserializer, Error};
use serde::ser::{Error as SerError, Serializer};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use serde_with::DisplayFromStr;
use serde_with::{Bytes, DeserializeAs, SerializeAs};

use sui_protocol_config::ProtocolVersion;

use crate::{parse_sui_struct_tag, parse_sui_type_tag};

#[inline]
fn to_custom_error<'de, D, E>(e: E) -> D::Error
where
    E: Debug,
    D: Deserializer<'de>,
{
    Error::custom(format!("byte deserialization failed, cause by: {:?}", e))
}

#[inline]
fn to_custom_ser_error<S, E>(e: E) -> S::Error
where
    E: Debug,
    S: Serializer,
{
    S::Error::custom(format!("byte serialization failed, cause by: {:?}", e))
}

/// Use with serde_as to control serde for human-readable serialization and deserialization
/// `H` : serde_as SerializeAs/DeserializeAs delegation for human readable in/output
/// `R` : serde_as SerializeAs/DeserializeAs delegation for non-human readable in/output
///
/// # Example:
///
/// ```text
/// #[serde_as]
/// #[derive(Deserialize, Serialize)]
/// struct Example(#[serde_as(as = "Readable<DisplayFromStr, _>")] [u8; 20]);
/// ```
///
/// The above example will delegate human-readable serde to `DisplayFromStr`
/// and array tuple (default) for non-human-readable serializer.
pub struct Readable<H, R> {
    human_readable: PhantomData<H>,
    non_human_readable: PhantomData<R>,
}

impl<T: ?Sized, H, R> SerializeAs<T> for Readable<H, R>
where
    H: SerializeAs<T>,
    R: SerializeAs<T>,
{
    fn serialize_as<S>(value: &T, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if serializer.is_human_readable() {
            H::serialize_as(value, serializer)
        } else {
            R::serialize_as(value, serializer)
        }
    }
}

impl<'de, R, H, T> DeserializeAs<'de, T> for Readable<H, R>
where
    H: DeserializeAs<'de, T>,
    R: DeserializeAs<'de, T>,
{
    fn deserialize_as<D>(deserializer: D) -> Result<T, D::Error>
    where
        D: Deserializer<'de>,
    {
        if deserializer.is_human_readable() {
            H::deserialize_as(deserializer)
        } else {
            R::deserialize_as(deserializer)
        }
    }
}

/// custom serde for AccountAddress
pub struct HexAccountAddress;

impl SerializeAs<AccountAddress> for HexAccountAddress {
    fn serialize_as<S>(value: &AccountAddress, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        Hex::serialize_as(value, serializer)
    }
}

impl<'de> DeserializeAs<'de, AccountAddress> for HexAccountAddress {
    fn deserialize_as<D>(deserializer: D) -> Result<AccountAddress, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        if s.starts_with("0x") {
            AccountAddress::from_hex_literal(&s)
        } else {
            AccountAddress::from_hex(&s)
        }
        .map_err(to_custom_error::<'de, D, _>)
    }
}

/// Serializes a bitmap according to the roaring bitmap on-disk standard.
/// <https://github.com/RoaringBitmap/RoaringFormatSpec>
pub struct SuiBitmap;

impl SerializeAs<roaring::RoaringBitmap> for SuiBitmap {
    fn serialize_as<S>(source: &roaring::RoaringBitmap, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut bytes = vec![];

        source
            .serialize_into(&mut bytes)
            .map_err(to_custom_ser_error::<S, _>)?;
        Bytes::serialize_as(&bytes, serializer)
    }
}

impl<'de> DeserializeAs<'de, roaring::RoaringBitmap> for SuiBitmap {
    fn deserialize_as<D>(deserializer: D) -> Result<roaring::RoaringBitmap, D::Error>
    where
        D: Deserializer<'de>,
    {
        let bytes: Vec<u8> = Bytes::deserialize_as(deserializer)?;
        roaring::RoaringBitmap::deserialize_from(&bytes[..]).map_err(to_custom_error::<'de, D, _>)
    }
}

/// Macro for implementing serde Serialize/Deserialize for a type that implements AsRef<[u8]>.
/// To be used only for non-fixed-size types (see `serialize_deserialize_with_to_from_bytes` in
/// FastCrypto for fixed-size types).
#[macro_export]
macro_rules! serde_to_from_bytes {
    ($type:ty) => {
        impl ::serde::Serialize for $type {
            fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                match serializer.is_human_readable() {
                    true => serializer.serialize_str(&self.encode_base64()),
                    false => self.as_ref().serialize(serializer),
                }
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $type {
            fn deserialize<D: ::serde::Deserializer<'de>>(
                deserializer: D,
            ) -> Result<Self, D::Error> {
                use serde::de::Error;
                match deserializer.is_human_readable() {
                    true => {
                        let s = <String as ::serde::Deserialize>::deserialize(deserializer)?;
                        Self::decode_base64(&s).map_err(::serde::de::Error::custom)
                    }
                    false => {
                        let data: Vec<u8> = Vec::deserialize(deserializer)?;
                        Self::from_bytes(&data).map_err(|e| Error::custom(e.to_string()))
                    }
                }
            }
        }
    };
}

pub struct SuiStructTag;

impl SerializeAs<StructTag> for SuiStructTag {
    fn serialize_as<S>(value: &StructTag, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = value.to_string();
        s.serialize(serializer)
    }
}

impl<'de> DeserializeAs<'de, StructTag> for SuiStructTag {
    fn deserialize_as<D>(deserializer: D) -> Result<StructTag, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        parse_sui_struct_tag(&s).map_err(D::Error::custom)
    }
}

pub struct SuiTypeTag;

impl SerializeAs<TypeTag> for SuiTypeTag {
    fn serialize_as<S>(value: &TypeTag, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = value.to_string();
        s.serialize(serializer)
    }
}

impl<'de> DeserializeAs<'de, TypeTag> for SuiTypeTag {
    fn deserialize_as<D>(deserializer: D) -> Result<TypeTag, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        parse_sui_type_tag(&s).map_err(D::Error::custom)
    }
}

#[serde_as]
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Copy, JsonSchema)]
pub struct BigInt<T>(
    #[schemars(with = "String")]
    #[serde_as(as = "DisplayFromStr")]
    T,
)
where
    T: Display + FromStr,
    <T as FromStr>::Err: Display;

impl<T> SerializeAs<T> for BigInt<T>
where
    T: Display + FromStr + Copy,
    <T as FromStr>::Err: Display,
{
    fn serialize_as<S>(value: &T, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        BigInt(*value).serialize(serializer)
    }
}

impl<'de, T> DeserializeAs<'de, T> for BigInt<T>
where
    T: Display + FromStr + Copy,
    <T as FromStr>::Err: Display,
{
    fn deserialize_as<D>(deserializer: D) -> Result<T, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(*BigInt::deserialize(deserializer)?)
    }
}

impl<T> From<T> for BigInt<T>
where
    T: Display + FromStr,
    <T as FromStr>::Err: Display,
{
    fn from(v: T) -> BigInt<T> {
        BigInt(v)
    }
}

impl<T> Deref for BigInt<T>
where
    T: Display + FromStr,
    <T as FromStr>::Err: Display,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> Display for BigInt<T>
where
    T: Display + FromStr,
    <T as FromStr>::Err: Display,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[serde_as]
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Copy, JsonSchema)]
pub struct SequenceNumber(#[schemars(with = "BigInt<u64>")] u64);

impl SerializeAs<crate::base_types::SequenceNumber> for SequenceNumber {
    fn serialize_as<S>(
        value: &crate::base_types::SequenceNumber,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = value.value().to_string();
        s.serialize(serializer)
    }
}

impl<'de> DeserializeAs<'de, crate::base_types::SequenceNumber> for SequenceNumber {
    fn deserialize_as<D>(deserializer: D) -> Result<crate::base_types::SequenceNumber, D::Error>
    where
        D: Deserializer<'de>,
    {
        let b = BigInt::deserialize(deserializer)?;
        Ok(crate::base_types::SequenceNumber::from_u64(*b))
    }
}

#[serde_as]
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Copy, JsonSchema)]
#[serde(rename = "ProtocolVersion")]
pub struct AsProtocolVersion(#[schemars(with = "BigInt<u64>")] u64);

impl SerializeAs<ProtocolVersion> for AsProtocolVersion {
    fn serialize_as<S>(value: &ProtocolVersion, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = value.as_u64().to_string();
        s.serialize(serializer)
    }
}

impl<'de> DeserializeAs<'de, ProtocolVersion> for AsProtocolVersion {
    fn deserialize_as<D>(deserializer: D) -> Result<ProtocolVersion, D::Error>
    where
        D: Deserializer<'de>,
    {
        let b = BigInt::<u64>::deserialize(deserializer)?;
        Ok(ProtocolVersion::from(*b))
    }
}
