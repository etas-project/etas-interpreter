//! Indexed, immutable ABI records produced while installing the SDK.
use super::AbiShape;
use etas_types::TypeId;
use std::{
    collections::{BTreeMap, HashMap},
    sync::OnceLock,
};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct ShapeArchive {
    data: ByteBuffer,
    slots: BTreeMap<TypeId, ShapeSlot>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct ShapeSlot {
    start: usize,
    len: usize,
    #[serde(skip)]
    value: OnceLock<AbiShape>,
}

impl ShapeArchive {
    pub(super) fn encode(
        shapes: &HashMap<TypeId, AbiShape>,
    ) -> Result<Self, bincode::error::EncodeError> {
        let mut data = Vec::new();
        let mut slots = BTreeMap::new();
        for (ty, shape) in shapes.iter().collect::<BTreeMap<_, _>>() {
            let start = data.len();
            bincode::serde::encode_into_std_write(shape, &mut data, bincode::config::standard())?;
            slots.insert(
                *ty,
                ShapeSlot {
                    start,
                    len: data.len() - start,
                    value: OnceLock::new(),
                },
            );
        }
        Ok(Self {
            data: ByteBuffer(data),
            slots,
        })
    }

    pub(super) fn get(&self, ty: TypeId) -> Option<&AbiShape> {
        let slot = self.slots.get(&ty)?;
        Some(slot.value.get_or_init(|| {
            bincode::serde::decode_from_slice(
                &self.data.0[slot.start..slot.start + slot.len],
                bincode::config::standard(),
            )
            .expect("installed checked ABI shape")
            .0
        }))
    }
}

/// Byte-buffer encoding lets the SDK decoder copy the records without visiting
/// every byte as a serde sequence element.
#[derive(Clone, Debug)]
struct ByteBuffer(Vec<u8>);
impl serde::Serialize for ByteBuffer {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(&self.0)
    }
}
impl<'de> serde::Deserialize<'de> for ByteBuffer {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Bytes;
        impl<'de> serde::de::Visitor<'de> for Bytes {
            type Value = ByteBuffer;
            fn expecting(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                out.write_str("encoded ABI records")
            }
            fn visit_bytes<E: serde::de::Error>(self, bytes: &[u8]) -> Result<Self::Value, E> {
                Ok(ByteBuffer(bytes.to_vec()))
            }
            fn visit_byte_buf<E: serde::de::Error>(self, bytes: Vec<u8>) -> Result<Self::Value, E> {
                Ok(ByteBuffer(bytes))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut input: A,
            ) -> Result<Self::Value, A::Error> {
                let mut bytes = Vec::with_capacity(input.size_hint().unwrap_or(0));
                while let Some(byte) = input.next_element()? {
                    bytes.push(byte);
                }
                Ok(ByteBuffer(bytes))
            }
        }
        deserializer.deserialize_byte_buf(Bytes)
    }
}
