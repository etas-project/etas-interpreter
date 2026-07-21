use etas_core::{Diagnostic, Span};
use etas_effects::{InterpreterFeatureKind, InterpreterSupport};

use crate::{diagnostics, plan::InterpreterPlan};

use super::HostServiceAvailability;

pub(crate) fn validate_host_readiness(
    plan: &InterpreterPlan,
    availability: HostServiceAvailability,
    span: Span,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let Some(entry_support) = &plan.host_requirements.entry else {
        diagnostics.push(diagnostics::missing_checked_fact(
            span,
            "interpreter plan is missing host requirement facts for the selected entry",
        ));
        return diagnostics;
    };

    match entry_support {
        InterpreterSupport::LocalOnly => {}
        InterpreterSupport::RequiresHost(requirements) => {
            for kind in &requirements.kinds {
                if !availability.supports(*kind) {
                    diagnostics.push(diagnostics::missing_host_handler(
                        span,
                        format!("missing Phase 1 host service for {:?}", kind),
                    ));
                }
            }
        }
        InterpreterSupport::RequiresInterpreterOrchestration(requirements) => {
            for kind in &requirements.host.kinds {
                if !availability.supports(*kind) {
                    diagnostics.push(diagnostics::missing_host_handler(
                        span,
                        format!("missing Phase 1 host service for {:?}", kind),
                    ));
                }
            }
            let unsupported = requirements
                .features
                .kinds
                .iter()
                .copied()
                .filter(|kind| !supports_interpreter_feature(*kind))
                .collect::<Vec<_>>();
            if !unsupported.is_empty() {
                diagnostics.push(diagnostics::unsupported_phase2_runtime_feature(
                    span,
                    format!(
                        "entry requires interpreter orchestration features {:?}, which are not available in the Phase 1 runtime",
                        unsupported
                    ),
                ));
            }
        }
        InterpreterSupport::Rejected(reason) => {
            diagnostics.push(diagnostics::unsupported_phase2_runtime_feature(
                span,
                format!(
                    "entry is rejected by frontend interpreter-support classification: {reason:?}"
                ),
            ));
        }
    }

    diagnostics
}

fn supports_interpreter_feature(kind: InterpreterFeatureKind) -> bool {
    matches!(
        kind,
        InterpreterFeatureKind::EffectHandler | InterpreterFeatureKind::Checkpoint
    )
}
