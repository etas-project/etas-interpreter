use std::fmt;

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};

use super::{File, Node};

pub(super) struct FileSeed(pub usize);

impl<'de> DeserializeSeed<'de> for FileSeed {
    type Value = File;
    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<File, D::Error> {
        decoder.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for FileSeed {
    type Value = File;
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a checkpoint file envelope")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<File, A::Error> {
        let (mut schema, mut root, mut nodes) = (None, None, None);
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "schema" => {
                    if schema.is_some() {
                        return Err(de::Error::duplicate_field("schema"));
                    }
                    schema = Some(map.next_value()?);
                }
                "root" => {
                    if root.is_some() {
                        return Err(de::Error::duplicate_field("root"));
                    }
                    root = Some(map.next_value()?);
                }
                "nodes" => {
                    if nodes.is_some() {
                        return Err(de::Error::duplicate_field("nodes"));
                    }
                    nodes = Some(map.next_value_seed(NodesSeed(self.0))?);
                }
                _ => return Err(de::Error::unknown_field(&key, &["schema", "root", "nodes"])),
            }
        }
        Ok(File {
            schema: schema.ok_or_else(|| de::Error::missing_field("schema"))?,
            root: root.ok_or_else(|| de::Error::missing_field("root"))?,
            nodes: nodes.ok_or_else(|| de::Error::missing_field("nodes"))?,
        })
    }
}

struct NodesSeed(usize);

impl<'de> DeserializeSeed<'de> for NodesSeed {
    type Value = Vec<Node>;
    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<Vec<Node>, D::Error> {
        decoder.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for NodesSeed {
    type Value = Vec<Node>;
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a bounded checkpoint node table")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Vec<Node>, A::Error> {
        let mut nodes = Vec::new();
        while nodes.len() < self.0 {
            match sequence.next_element()? {
                Some(node) => nodes.push(node),
                None => return Ok(nodes),
            }
        }
        if sequence.next_element::<de::IgnoredAny>()?.is_some() {
            return Err(de::Error::custom("checkpoint file exceeds node budget"));
        }
        Ok(nodes)
    }
}
