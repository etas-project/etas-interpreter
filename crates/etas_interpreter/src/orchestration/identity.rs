use std::fmt::Debug;

use etas_frontend::CheckedProject;
use etas_hir::{HirItem, HirItemId};

use super::CheckpointCompilationIdentity;

pub(crate) const CHECKPOINT_ARTIFACT_SCHEMA: &str = "etas.cli.interpreter-checkpoint.v30";

impl CheckpointCompilationIdentity {
    pub(crate) fn for_project(
        checked: &CheckedProject,
        entry_item: HirItemId,
    ) -> Result<Self, String> {
        Ok(Self {
            schema_version: CHECKPOINT_ARTIFACT_SCHEMA.to_owned(),
            compiler_version: checked.compiler_version.clone(),
            project_fingerprint: project_fingerprint(checked),
            checked_hir_fingerprint: checked_hir_fingerprint(checked),
            dependency_metadata_fingerprints: checked.dependency_metadata_fingerprints.clone(),
            entry_semantic_identity: entry_semantic_identity(checked, entry_item)?,
        })
    }

    pub(crate) fn validate_for_project(
        &self,
        checked: &CheckedProject,
        entry_item: HirItemId,
    ) -> Result<(), String> {
        let expected = Self::for_project(checked, entry_item)?;
        if self.schema_version != expected.schema_version {
            return Err(format!(
                "checkpoint schema identity `{}` does not match runtime schema `{}`",
                self.schema_version, expected.schema_version
            ));
        }
        if self.compiler_version != expected.compiler_version {
            return Err(format!(
                "checkpoint compiler version `{}` does not match current compiler `{}`",
                self.compiler_version, expected.compiler_version
            ));
        }
        if self.project_fingerprint != expected.project_fingerprint {
            return Err("checkpoint project fingerprint does not match current sources".to_owned());
        }
        if self.checked_hir_fingerprint != expected.checked_hir_fingerprint {
            return Err(
                "checkpoint checked-HIR fingerprint does not match current compilation".to_owned(),
            );
        }
        if self.dependency_metadata_fingerprints != expected.dependency_metadata_fingerprints {
            return Err(
                "checkpoint dependency metadata fingerprints do not match current dependencies"
                    .to_owned(),
            );
        }
        if self.entry_semantic_identity != expected.entry_semantic_identity {
            return Err(
                "checkpoint entry semantic identity does not match the selected entry".to_owned(),
            );
        }
        Ok(())
    }
}

fn project_fingerprint(checked: &CheckedProject) -> String {
    let mut sources = checked.sources.sources.iter().collect::<Vec<_>>();
    sources.sort_by(|left, right| {
        left.path
            .as_ref()
            .map(|path| path.to_string_lossy())
            .cmp(&right.path.as_ref().map(|path| path.to_string_lossy()))
            .then_with(|| left.id.0.cmp(&right.id.0))
    });
    let mut hasher = blake3::Hasher::new();
    update(&mut hasher, b"etas-checkpoint-project:v1");
    update(
        &mut hasher,
        checked.project_environment_fingerprint.as_bytes(),
    );
    for source in sources {
        update(&mut hasher, &source.id.0.to_le_bytes());
        update(
            &mut hasher,
            source
                .path
                .as_ref()
                .map(|path| path.to_string_lossy())
                .as_deref()
                .unwrap_or("<anonymous>")
                .as_bytes(),
        );
        update(&mut hasher, source.text().as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

fn checked_hir_fingerprint(checked: &CheckedProject) -> String {
    let mut hasher = blake3::Hasher::new();
    update(&mut hasher, b"etas-checkpoint-hir:v1");
    update(&mut hasher, checked.compiler_version.as_bytes());
    for (_, module) in checked.hir.modules_arena.iter() {
        update_debug(&mut hasher, module);
    }
    for (_, item) in checked.hir.items.iter() {
        update_debug(&mut hasher, item);
    }
    for (_, expr) in checked.hir.exprs.iter() {
        update_debug(&mut hasher, expr);
    }
    for (_, arm) in checked.hir.handler_arms.iter() {
        update_debug(&mut hasher, arm);
    }
    for (_, stmt) in checked.hir.stmts.iter() {
        update_debug(&mut hasher, stmt);
    }
    for (_, pat) in checked.hir.pats.iter() {
        update_debug(&mut hasher, pat);
    }
    for (_, ty) in checked.hir.types.iter() {
        update_debug(&mut hasher, ty);
    }
    for (_, block) in checked.hir.blocks.iter() {
        update_debug(&mut hasher, block);
    }
    for symbol in checked.symbols.iter() {
        update_debug(&mut hasher, symbol);
    }
    for scope in checked.scopes.iter() {
        update_debug(
            &mut hasher,
            &(
                scope.id,
                scope.parent,
                scope.owner,
                &scope.symbols,
                scope.span,
            ),
        );
    }
    hasher.finalize().to_hex().to_string()
}

fn entry_semantic_identity(
    checked: &CheckedProject,
    entry_item: HirItemId,
) -> Result<String, String> {
    let item = checked
        .hir
        .items
        .get(entry_item)
        .ok_or_else(|| format!("entry item {} is missing from checked HIR", entry_item.0))?;
    let (kind, symbol) = match item {
        HirItem::Flow(item) => ("flow", item.symbol),
        HirItem::Agent(item) => ("agent", item.symbol),
        HirItem::Tool(item) => ("tool", item.symbol),
        _ => {
            return Err(format!(
                "checkpoint entry item {} is not an executable callable",
                entry_item.0
            ));
        }
    };
    let symbol = checked
        .symbols
        .get(symbol)
        .ok_or_else(|| "checkpoint entry symbol is missing from checked HIR".to_owned())?;
    let module = checked
        .entry_fact
        .requested
        .module
        .as_ref()
        .map(|module| module.segments.join("."))
        .unwrap_or_else(|| "<single-file>".to_owned());
    Ok(format!("{kind}:{module}::{}", symbol.name))
}

fn update_debug(hasher: &mut blake3::Hasher, value: &impl Debug) {
    update(hasher, format!("{value:?}").as_bytes());
}

fn update(hasher: &mut blake3::Hasher, bytes: &[u8]) {
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}
