use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    path::Path,
    sync::{Arc, Mutex},
};

/// Explicit terminal-content recorder, kept separate from ordinary trace events.
/// The descriptor belongs to this invocation; existing files are never reopened.
#[derive(Clone)]
pub struct PtyRecording(Arc<Mutex<File>>);

impl std::fmt::Debug for PtyRecording {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PtyRecording(<private>)")
    }
}

impl PtyRecording {
    pub fn create(path: &Path) -> io::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)?;
            Ok(Self(Arc::new(Mutex::new(file))))
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "private PTY recordings are unavailable on this platform",
            ))
        }
    }

    pub(crate) fn record(
        &self,
        request: &etas_host::CommandProcessRequest,
        session: &str,
        phase: &str,
        bytes: &[u8],
    ) -> io::Result<()> {
        use etas_host::{CommandProcessOperation, PtyOperation};
        let action = match &request.operation {
            CommandProcessOperation::Pty(PtyOperation::Write { .. }) => "Command.write_pty",
            CommandProcessOperation::Pty(PtyOperation::Read { .. }) => "Command.read_pty",
            _ => unreachable!("only raw terminal I/O belongs in the content recording"),
        };
        #[derive(serde::Serialize)]
        struct Entry<'a> {
            trace: String,
            id: u32,
            action: &'a str,
            session: &'a str,
            phase: &'a str,
            bytes: &'a [u8],
        }
        let entry = Entry {
            trace: request.trace.trace_id.to_hex(),
            id: request.id.0,
            action,
            session,
            phase,
            bytes,
        };
        let mut file = self.0.lock().expect("PTY recording mutex");
        serde_json::to_writer(&mut *file, &entry)?;
        file.write_all(b"\n")
    }
}
