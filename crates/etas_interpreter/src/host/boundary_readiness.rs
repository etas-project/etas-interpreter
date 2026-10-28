use etas_core::Diagnostic;

use super::HostServiceAvailability;
use crate::{
    control::HostBoundaryRequest,
    eval::machine::{PendingBoundary, PendingToolDispatch},
};

/// Scoped programs may have caller-owned handlers. Check the actual default
/// boundary after handler selection, without granting or fabricating a service.
pub(crate) fn validate_boundary_readiness(
    boundary: &PendingBoundary,
    available: HostServiceAvailability,
) -> Option<Diagnostic> {
    let (ready, service, span) = match boundary {
        PendingBoundary::Perform(_) => return None,
        PendingBoundary::Console(pending) => (available.console, "Console", pending.span),
        PendingBoundary::Command(pending) => (available.command, "Command", pending.span),
        PendingBoundary::Memory(pending) => (available.memory, "DurableMemory", pending.span),
        PendingBoundary::Session(pending) => (available.session, "Session", pending.span),
        PendingBoundary::Model(pending) => (available.model, "Agentic", pending.span),
        PendingBoundary::Tool(pending) => match pending.dispatch {
            PendingToolDispatch::Source => return None,
            PendingToolDispatch::Host(_) => (available.tool, "ToolCall", pending.span),
        },
        PendingBoundary::Host(pending) => {
            let (ready, service) = match pending.request {
                HostBoundaryRequest::SessionHistory(_) | HostBoundaryRequest::SessionContext(_) => {
                    (available.session, "Session")
                }
                HostBoundaryRequest::MemoryWrite(_) => (available.memory, "DurableMemory"),
                HostBoundaryRequest::Filesystem(_) => (available.filesystem, "FileIO"),
                HostBoundaryRequest::Tcp(_) => (available.tcp, "Tcp"),
                HostBoundaryRequest::Stream(_) => (available.stream, "Stream"),
                HostBoundaryRequest::Tls(_) => (available.tls, "Tls"),
                HostBoundaryRequest::Secret(_) => (available.secret_access, "SecretAccess"),
                HostBoundaryRequest::Browser(_) => (available.browser, "Browser"),
            };
            (ready, service, pending.span)
        }
    };
    (!ready).then(|| {
        crate::diagnostics::missing_host_handler(
            span,
            format!("missing Phase 1 host service for {service} support"),
        )
    })
}
