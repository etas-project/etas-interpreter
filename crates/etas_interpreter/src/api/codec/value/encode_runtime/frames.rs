use super::*;
use crate::{
    control::{Frame, FrameLocals},
    value::graph::{FrameDiscovery, Node as GraphNode},
};
use std::collections::HashMap;

type Identity = (*const (), u64);

pub(super) struct FrameGraph {
    frames: Vec<Frame>,
    indices: HashMap<Identity, usize>,
}

impl FrameGraph {
    pub(super) fn discover(
        value: &InterpValue,
        purpose: ValuePurpose,
    ) -> Result<Self, InterpreterCodecError> {
        let roots = FrameDiscovery::default().frames_filtered(
            GraphNode::Value(value),
            false,
            |_| false,
            |value| purpose.redacts(value),
        );
        let mut graph = Self {
            frames: Vec::new(),
            indices: HashMap::new(),
        };
        for frame in roots {
            graph.insert(frame);
        }
        let mut edges = Vec::new();
        let mut index = 0;
        while index < graph.frames.len() {
            let frame = graph.frames[index].clone();
            let borrowed = frame.borrow_locals().map_err(InterpreterCodecError::new)?;
            let mut discovery = FrameDiscovery::default();
            let mut children = Vec::new();
            for (_, value) in borrowed.sorted() {
                for frame in discovery.frames_filtered(
                    GraphNode::Value(value),
                    false,
                    |_| false,
                    |value| purpose.redacts(value),
                ) {
                    children.push(graph.insert(frame));
                }
            }
            edges.push(children);
            index += 1;
        }
        let mut states = vec![0u8; edges.len()];
        for root in 0..edges.len() {
            if states[root] == 2 {
                continue;
            }
            let mut pending = vec![(root, false)];
            while let Some((index, finish)) = pending.pop() {
                if finish {
                    states[index] = 2;
                    continue;
                }
                match states[index] {
                    2 => continue,
                    1 => {
                        return Err(InterpreterCodecError::new(
                            "cyclic captured-frame graph cannot be projected as runtime JSON",
                        ));
                    }
                    _ => (),
                }
                states[index] = 1;
                pending.push((index, true));
                pending.extend(edges[index].iter().rev().map(|child| (*child, false)));
            }
        }
        Ok(graph)
    }

    fn insert(&mut self, frame: Frame) -> usize {
        if let Some(index) = self.indices.get(&frame.capture_identity()) {
            return *index;
        }
        let index = self.frames.len();
        self.indices.insert(frame.capture_identity(), index);
        self.frames.push(frame);
        index
    }

    pub(super) fn borrow(&self) -> Result<BorrowedFrames<'_>, InterpreterCodecError> {
        let definitions = self
            .frames
            .iter()
            .map(|frame| frame.borrow_locals().map_err(InterpreterCodecError::new))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(BorrowedFrames {
            definitions,
            indices: &self.indices,
        })
    }
}

pub(super) struct BorrowedFrames<'a> {
    definitions: Vec<FrameLocals<'a>>,
    indices: &'a HashMap<Identity, usize>,
}

impl BorrowedFrames<'_> {
    pub(super) fn get(&self, frame: &Frame) -> Result<&FrameLocals<'_>, InterpreterCodecError> {
        self.indices
            .get(&frame.capture_identity())
            .and_then(|index| self.definitions.get(*index))
            .ok_or_else(|| {
                InterpreterCodecError::new(
                    "runtime JSON callable has no prepared captured-frame definition",
                )
            })
    }
}
