use etas_host::{
    StorageLimits,
    session::{SessionHistoryFence, SessionPublishedContext},
};

pub(super) trait Payload {
    fn measure(&self, budget: &mut ViewBudget<'_>, depth: usize) -> Result<(), String>;
}

pub(super) struct MessageView<'a, P: ?Sized> {
    pub id: &'a str,
    pub session: Option<&'a str>,
    pub from: Option<&'a str>,
    pub to: Option<&'a str>,
    pub created_at: &'a str,
    pub payload: &'a P,
    pub provenance: Option<(Option<&'a str>, Option<&'a str>)>,
}

pub(super) struct ViewBudget<'a> {
    pub limits: &'a StorageLimits,
    session: &'a str,
    bytes: usize,
    nodes: usize,
}

pub(super) fn exceeded() -> String {
    "conversation storage resource limit exceeded".into()
}

impl<'a> ViewBudget<'a> {
    pub fn new(
        limits: &'a StorageLimits,
        session: &'a str,
        messages: usize,
    ) -> Result<Self, String> {
        limits.validate().map_err(|e| e.message)?;
        if messages > limits.max_scan_rows {
            return Err(exceeded());
        }
        let mut budget = Self {
            limits,
            session,
            bytes: 0,
            nodes: 0,
        };
        budget.charge(session.len(), 0)?;
        Ok(budget)
    }

    // One canonical accounting model for borrowed runtime, snapshot and JSON views.
    pub fn charge(&mut self, bytes: usize, depth: usize) -> Result<(), String> {
        self.nodes = self.nodes.checked_add(1).ok_or_else(exceeded)?;
        self.bytes = self.bytes.checked_add(bytes).ok_or_else(exceeded)?;
        if depth > self.limits.max_depth
            || self.nodes > self.limits.max_nodes
            || self.bytes > self.limits.max_result_bytes
            || bytes > self.limits.max_value_bytes
        {
            return Err(exceeded());
        }
        Ok(())
    }

    pub fn node(&mut self, depth: usize) -> Result<(), String> {
        self.charge(std::mem::size_of::<etas_host::HostValue>(), depth)
    }

    pub fn message<P: Payload + ?Sized>(
        &mut self,
        message: MessageView<'_, P>,
    ) -> Result<(), String> {
        if message.session != Some(self.session) {
            return Err("conversation message belongs to another session".into());
        }
        self.envelope(message, 0)
    }

    pub fn envelope<P: Payload + ?Sized>(
        &mut self,
        message: MessageView<'_, P>,
        depth: usize,
    ) -> Result<(), String> {
        let start = self.bytes;
        self.node(depth)?;
        for text in [
            Some(message.id),
            message.session,
            message.from,
            message.to,
            Some(message.created_at),
        ]
        .into_iter()
        .flatten()
        {
            self.charge(text.len(), depth)?;
        }
        message.payload.measure(self, depth + 1)?;
        if let Some((trace, source)) = message.provenance {
            self.node(depth)?;
            for (name, value) in [("trace_id", trace), ("source", source)] {
                self.charge(name.len(), depth)?;
                self.node(depth + 1)?;
                if let Some(value) = value {
                    self.charge(value.len(), depth + 1)?;
                }
            }
        }
        if self.bytes - start > self.limits.max_value_bytes {
            return Err(exceeded());
        }
        Ok(())
    }

    pub fn metadata(
        &mut self,
        cursor: Option<&str>,
        fence: Option<&SessionHistoryFence>,
        context: Option<&SessionPublishedContext>,
    ) -> Result<(), String> {
        if let Some(cursor) = cursor {
            self.charge(cursor.len(), 0)?;
        }
        if let Some(fence) = fence {
            if fence.session_id() != self.session {
                return Err("conversation history fence belongs to another session".into());
            }
            self.charge(fence.as_token().len(), 0)?;
        }
        if let Some(context) = context {
            if context.fence.session_id() != self.session {
                return Err("conversation selected context belongs to another session".into());
            }
            context.storage_size(self.limits).map_err(|e| e.message)?;
            self.context(
                &context.content.text,
                context.fence.as_token(),
                context
                    .content
                    .provenance
                    .iter()
                    .map(|(k, v)| Ok((k.as_str(), v.as_str()))),
            )?;
        }
        Ok(())
    }

    pub fn context<'b>(
        &mut self,
        text: &str,
        fence: &str,
        provenance: impl Iterator<Item = Result<(&'b str, &'b str), String>>,
    ) -> Result<(), String> {
        let start = self.bytes;
        self.node(0)?;
        self.charge(text.len(), 0)?;
        self.charge(fence.len(), 0)?;
        for pair in provenance {
            let (key, value) = pair?;
            self.node(1)?;
            self.charge(key.len(), 1)?;
            self.charge(value.len(), 1)?;
        }
        if self.bytes - start > self.limits.max_value_bytes {
            return Err(exceeded());
        }
        Ok(())
    }
}
