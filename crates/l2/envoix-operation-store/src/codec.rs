use std::fmt;

use serde::Serialize;
use serde::de::{
    self, DeserializeOwned, DeserializeSeed, EnumAccess, IntoDeserializer, MapAccess, SeqAccess,
    VariantAccess, Visitor,
};
use serde::ser::{
    self, SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple,
    SerializeTupleStruct, SerializeTupleVariant,
};

pub(crate) fn to_vec<T: Serialize>(value: &T) -> Result<Vec<u8>, CodecError> {
    let mut encoder = Encoder { bytes: Vec::new() };
    value.serialize(&mut encoder)?;
    Ok(encoder.bytes)
}

pub(crate) fn from_slice<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, CodecError> {
    let mut decoder = Decoder { remaining: bytes };
    let value = T::deserialize(&mut decoder)?;
    if !decoder.remaining.is_empty() {
        return Err(CodecError);
    }
    Ok(value)
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct CodecError;

impl fmt::Display for CodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid operation-store state encoding")
    }
}

impl std::error::Error for CodecError {}

impl ser::Error for CodecError {
    fn custom<T: fmt::Display>(_message: T) -> Self {
        Self
    }
}

impl de::Error for CodecError {
    fn custom<T: fmt::Display>(_message: T) -> Self {
        Self
    }
}

struct Encoder {
    bytes: Vec<u8>,
}

impl Encoder {
    fn length(&mut self, length: usize) -> Result<(), CodecError> {
        let length = u32::try_from(length).map_err(|_| CodecError)?;
        self.bytes.extend_from_slice(&length.to_be_bytes());
        Ok(())
    }
}

impl<'a> ser::Serializer for &'a mut Encoder {
    type Ok = ();
    type Error = CodecError;
    type SerializeSeq = Compound<'a>;
    type SerializeTuple = Compound<'a>;
    type SerializeTupleStruct = Compound<'a>;
    type SerializeTupleVariant = Compound<'a>;
    type SerializeMap = Compound<'a>;
    type SerializeStruct = Compound<'a>;
    type SerializeStructVariant = Compound<'a>;

    fn serialize_bool(self, value: bool) -> Result<Self::Ok, Self::Error> {
        self.bytes.push(u8::from(value));
        Ok(())
    }

    fn serialize_i8(self, value: i8) -> Result<Self::Ok, Self::Error> {
        self.bytes.push(value as u8);
        Ok(())
    }

    fn serialize_i16(self, value: i16) -> Result<Self::Ok, Self::Error> {
        self.bytes.extend_from_slice(&value.to_be_bytes());
        Ok(())
    }

    fn serialize_i32(self, value: i32) -> Result<Self::Ok, Self::Error> {
        self.bytes.extend_from_slice(&value.to_be_bytes());
        Ok(())
    }

    fn serialize_i64(self, value: i64) -> Result<Self::Ok, Self::Error> {
        self.bytes.extend_from_slice(&value.to_be_bytes());
        Ok(())
    }

    fn serialize_i128(self, value: i128) -> Result<Self::Ok, Self::Error> {
        self.bytes.extend_from_slice(&value.to_be_bytes());
        Ok(())
    }

    fn serialize_u8(self, value: u8) -> Result<Self::Ok, Self::Error> {
        self.bytes.push(value);
        Ok(())
    }

    fn serialize_u16(self, value: u16) -> Result<Self::Ok, Self::Error> {
        self.bytes.extend_from_slice(&value.to_be_bytes());
        Ok(())
    }

    fn serialize_u32(self, value: u32) -> Result<Self::Ok, Self::Error> {
        self.bytes.extend_from_slice(&value.to_be_bytes());
        Ok(())
    }

    fn serialize_u64(self, value: u64) -> Result<Self::Ok, Self::Error> {
        self.bytes.extend_from_slice(&value.to_be_bytes());
        Ok(())
    }

    fn serialize_u128(self, value: u128) -> Result<Self::Ok, Self::Error> {
        self.bytes.extend_from_slice(&value.to_be_bytes());
        Ok(())
    }

    fn serialize_f32(self, value: f32) -> Result<Self::Ok, Self::Error> {
        self.serialize_u32(value.to_bits())
    }

    fn serialize_f64(self, value: f64) -> Result<Self::Ok, Self::Error> {
        self.serialize_u64(value.to_bits())
    }

    fn serialize_char(self, value: char) -> Result<Self::Ok, Self::Error> {
        self.serialize_u32(value as u32)
    }

    fn serialize_str(self, value: &str) -> Result<Self::Ok, Self::Error> {
        self.serialize_bytes(value.as_bytes())
    }

    fn serialize_bytes(self, value: &[u8]) -> Result<Self::Ok, Self::Error> {
        self.length(value.len())?;
        self.bytes.extend_from_slice(value);
        Ok(())
    }

    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        self.bytes.push(0);
        Ok(())
    }

    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<Self::Ok, Self::Error> {
        self.bytes.push(1);
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        variant_index: u32,
        _variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        self.serialize_u32(variant_index)
    }

    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        variant_index: u32,
        _variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        self.serialize_u32(variant_index)?;
        value.serialize(self)
    }

    fn serialize_seq(self, length: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        self.length(length.ok_or(CodecError)?)?;
        Ok(Compound { encoder: self })
    }

    fn serialize_tuple(self, _length: usize) -> Result<Self::SerializeTuple, Self::Error> {
        Ok(Compound { encoder: self })
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _length: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        Ok(Compound { encoder: self })
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        variant_index: u32,
        _variant: &'static str,
        _length: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        self.serialize_u32(variant_index)?;
        Ok(Compound { encoder: self })
    }

    fn serialize_map(self, length: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        self.length(length.ok_or(CodecError)?)?;
        Ok(Compound { encoder: self })
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        _length: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        Ok(Compound { encoder: self })
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        variant_index: u32,
        _variant: &'static str,
        _length: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        self.serialize_u32(variant_index)?;
        Ok(Compound { encoder: self })
    }

    fn collect_str<T: ?Sized + fmt::Display>(self, value: &T) -> Result<Self::Ok, Self::Error> {
        self.serialize_str(&value.to_string())
    }
}

struct Compound<'a> {
    encoder: &'a mut Encoder,
}

macro_rules! compound_element {
    ($trait_name:ident, $method:ident) => {
        impl $trait_name for Compound<'_> {
            type Ok = ();
            type Error = CodecError;

            fn $method<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
                value.serialize(&mut *self.encoder)
            }

            fn end(self) -> Result<Self::Ok, Self::Error> {
                Ok(())
            }
        }
    };
}

compound_element!(SerializeSeq, serialize_element);
compound_element!(SerializeTuple, serialize_element);
compound_element!(SerializeTupleStruct, serialize_field);
compound_element!(SerializeTupleVariant, serialize_field);

impl SerializeMap for Compound<'_> {
    type Ok = ();
    type Error = CodecError;

    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<(), Self::Error> {
        key.serialize(&mut *self.encoder)
    }

    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        value.serialize(&mut *self.encoder)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

impl SerializeStruct for Compound<'_> {
    type Ok = ();
    type Error = CodecError;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        _key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        value.serialize(&mut *self.encoder)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

impl SerializeStructVariant for Compound<'_> {
    type Ok = ();
    type Error = CodecError;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        _key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        value.serialize(&mut *self.encoder)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

struct Decoder<'de> {
    remaining: &'de [u8],
}

impl<'de> Decoder<'de> {
    fn take(&mut self, length: usize) -> Result<&'de [u8], CodecError> {
        let (value, remaining) = self.remaining.split_at_checked(length).ok_or(CodecError)?;
        self.remaining = remaining;
        Ok(value)
    }

    fn length(&mut self) -> Result<usize, CodecError> {
        let bytes = self.take(4)?;
        Ok(u32::from_be_bytes(bytes.try_into().map_err(|_| CodecError)?) as usize)
    }
}

macro_rules! deserialize_number {
    ($method:ident, $visit:ident, $type:ty, $length:expr) => {
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
            let bytes = self.take($length)?;
            visitor.$visit(<$type>::from_be_bytes(
                bytes.try_into().map_err(|_| CodecError)?,
            ))
        }
    };
}

impl<'de> de::Deserializer<'de> for &mut Decoder<'de> {
    type Error = CodecError;

    fn deserialize_any<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, Self::Error> {
        Err(CodecError)
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.take(1)?[0] {
            0 => visitor.visit_bool(false),
            1 => visitor.visit_bool(true),
            _ => Err(CodecError),
        }
    }

    fn deserialize_i8<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_i8(self.take(1)?[0] as i8)
    }

    deserialize_number!(deserialize_i16, visit_i16, i16, 2);
    deserialize_number!(deserialize_i32, visit_i32, i32, 4);
    deserialize_number!(deserialize_i64, visit_i64, i64, 8);
    deserialize_number!(deserialize_i128, visit_i128, i128, 16);

    fn deserialize_u8<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_u8(self.take(1)?[0])
    }

    deserialize_number!(deserialize_u16, visit_u16, u16, 2);
    deserialize_number!(deserialize_u32, visit_u32, u32, 4);
    deserialize_number!(deserialize_u64, visit_u64, u64, 8);
    deserialize_number!(deserialize_u128, visit_u128, u128, 16);

    fn deserialize_f32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        let bytes = self.take(4)?;
        visitor.visit_f32(f32::from_bits(u32::from_be_bytes(
            bytes.try_into().map_err(|_| CodecError)?,
        )))
    }

    fn deserialize_f64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        let bytes = self.take(8)?;
        visitor.visit_f64(f64::from_bits(u64::from_be_bytes(
            bytes.try_into().map_err(|_| CodecError)?,
        )))
    }

    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        let bytes = self.take(4)?;
        let value = u32::from_be_bytes(bytes.try_into().map_err(|_| CodecError)?);
        visitor.visit_char(char::from_u32(value).ok_or(CodecError)?)
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        let length = self.length()?;
        let value = std::str::from_utf8(self.take(length)?).map_err(|_| CodecError)?;
        visitor.visit_borrowed_str(value)
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_str(visitor)
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        let length = self.length()?;
        visitor.visit_borrowed_bytes(self.take(length)?)
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        let length = self.length()?;
        visitor.visit_byte_buf(self.take(length)?.to_vec())
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.take(1)?[0] {
            0 => visitor.visit_none(),
            1 => visitor.visit_some(self),
            _ => Err(CodecError),
        }
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_unit()
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        let remaining = self.length()?;
        visitor.visit_seq(CountedAccess {
            decoder: self,
            remaining,
        })
    }

    fn deserialize_tuple<V: Visitor<'de>>(
        self,
        length: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_seq(CountedAccess {
            decoder: self,
            remaining: length,
        })
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        length: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_tuple(length, visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        let remaining = self.length()?;
        visitor.visit_map(CountedMapAccess {
            decoder: self,
            remaining,
            value_expected: false,
        })
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_seq(CountedAccess {
            decoder: self,
            remaining: fields.len(),
        })
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        let bytes = self.take(4)?;
        let variant = u32::from_be_bytes(bytes.try_into().map_err(|_| CodecError)?);
        visitor.visit_enum(VariantDecoder {
            decoder: self,
            variant,
        })
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_str(visitor)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(
        self,
        _visitor: V,
    ) -> Result<V::Value, Self::Error> {
        Err(CodecError)
    }
}

struct CountedAccess<'a, 'de> {
    decoder: &'a mut Decoder<'de>,
    remaining: usize,
}

impl<'de> SeqAccess<'de> for CountedAccess<'_, 'de> {
    type Error = CodecError;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, Self::Error> {
        if self.remaining == 0 {
            return Ok(None);
        }
        self.remaining -= 1;
        seed.deserialize(&mut *self.decoder).map(Some)
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.remaining)
    }
}

struct CountedMapAccess<'a, 'de> {
    decoder: &'a mut Decoder<'de>,
    remaining: usize,
    value_expected: bool,
}

impl<'de> MapAccess<'de> for CountedMapAccess<'_, 'de> {
    type Error = CodecError;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Self::Error> {
        if self.remaining == 0 {
            return Ok(None);
        }
        if self.value_expected {
            return Err(CodecError);
        }
        self.value_expected = true;
        seed.deserialize(&mut *self.decoder).map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> Result<V::Value, Self::Error> {
        if !self.value_expected {
            return Err(CodecError);
        }
        self.value_expected = false;
        self.remaining -= 1;
        seed.deserialize(&mut *self.decoder)
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.remaining)
    }
}

struct VariantDecoder<'a, 'de> {
    decoder: &'a mut Decoder<'de>,
    variant: u32,
}

impl<'a, 'de> EnumAccess<'de> for VariantDecoder<'a, 'de> {
    type Error = CodecError;
    type Variant = VariantPayload<'a, 'de>;

    fn variant_seed<V: DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Self::Variant), Self::Error> {
        let variant = seed.deserialize(self.variant.into_deserializer())?;
        Ok((
            variant,
            VariantPayload {
                decoder: self.decoder,
            },
        ))
    }
}

struct VariantPayload<'a, 'de> {
    decoder: &'a mut Decoder<'de>,
}

impl<'de> VariantAccess<'de> for VariantPayload<'_, 'de> {
    type Error = CodecError;

    fn unit_variant(self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(
        self,
        seed: T,
    ) -> Result<T::Value, Self::Error> {
        seed.deserialize(self.decoder)
    }

    fn tuple_variant<V: Visitor<'de>>(
        self,
        length: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        de::Deserializer::deserialize_tuple(self.decoder, length, visitor)
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        de::Deserializer::deserialize_tuple(self.decoder, fields.len(), visitor)
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    use super::{from_slice, to_vec};

    #[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
    struct Fixture {
        number: u64,
        values: Vec<Vec<u8>>,
        choice: Option<Choice>,
    }

    #[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
    enum Choice {
        Unit,
        Struct { name: String, enabled: bool },
    }

    #[test]
    fn round_trip_supported_serde_shapes() {
        let fixture = Fixture {
            number: 42,
            values: vec![vec![1, 2, 3], Vec::new()],
            choice: Some(Choice::Struct {
                name: "durable".to_owned(),
                enabled: true,
            }),
        };
        let encoded = to_vec(&fixture).unwrap();
        assert_eq!(from_slice::<Fixture>(&encoded).unwrap(), fixture);
        assert!(from_slice::<Fixture>(&encoded[..encoded.len() - 1]).is_err());
    }
}
