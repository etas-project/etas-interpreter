use super::*;
use crate::control::GenericBindings;
use etas_frontend::CheckedProgram;
use etas_types::{
    CallableGenericParam, CallableGenericParamKind, CallableSignature, EffectRowRef, FlowType,
    TypeId,
};

#[derive(Clone, Debug)]
pub(super) struct ProgramSpecialization {
    pub presented_types: Vec<(String, TypeId)>,
    pub presented_rows: Vec<(String, EffectRowRef)>,
    destination_bindings: GenericBindings,
    source_signature: FlowType,
    destination_signature: FlowType,
    source_program: CheckedProgram,
    destination_program: CheckedProgram,
}

impl ProgramSpecialization {
    pub fn new(
        call: &ProgramCall,
        source: &CheckedProgram,
        types: Vec<(String, TypeId)>,
        rows: Vec<(String, EffectRowRef)>,
    ) -> Result<Self, String> {
        let bindings = GenericBindings {
            types: types.iter().cloned().collect(),
            rows: rows.iter().cloned().collect(),
        };
        if bindings.types.len() != types.len() || bindings.rows.len() != rows.len() {
            return Err("program call contains duplicate generic bindings".into());
        }
        let destination = call.destination()?;
        let (source_template, destination_template, source_params, destination_params) =
            match call.target() {
                ProgramCallTarget::Agent(agent) => {
                    if !Arc::ptr_eq(source, agent.source_program()) {
                        return Err("generic revision call has the wrong source program".into());
                    }
                    let from = agent.source_signature()?;
                    let target = agent.target()?;
                    let to = if agent.frozen_agent().is_some() {
                        &target.original
                    } else {
                        &target.candidate
                    };
                    (
                        flow(from),
                        flow(to),
                        from.generic_params.as_slice(),
                        to.generic_params.as_slice(),
                    )
                }
                ProgramCallTarget::Callable(call) => {
                    if !Arc::ptr_eq(source, call.presented_program()) {
                        return Err("generic scoped call has the wrong source program".into());
                    }
                    (
                        call.presented_signature()?.clone(),
                        call.native_signature().clone(),
                        call.native_generic_parameters()?,
                        call.native_generic_parameters()?,
                    )
                }
            };
        validate_parameters(source_params, &bindings)?;
        if source_params.len() != destination_params.len()
            || source_params
                .iter()
                .zip(destination_params)
                .any(|(from, to)| from.kind != to.kind)
        {
            return Err("generic program call has inconsistent frozen binder identities".into());
        }
        let mut mapped = bindings.clone();
        for edge in call
            .action_projections()?
            .iter()
            .rev()
            .map(crate::plan::code::RevisionProjection::reversed)
        {
            mapped.types = mapped
                .types
                .into_iter()
                .map(|(name, ty)| Ok((name, edge.runtime_type(ty)?)))
                .collect::<Result<_, String>>()?;
            mapped.rows = mapped
                .rows
                .iter()
                .map(|(name, row)| Ok((name.clone(), edge.effect_row(row.clone())?)))
                .collect::<Result<_, String>>()?;
        }
        let mut destination_bindings = GenericBindings::new();
        for (from, to) in source_params.iter().zip(destination_params) {
            match from.kind {
                CallableGenericParamKind::Type => {
                    destination_bindings.insert(
                        to.name.clone(),
                        *mapped
                            .get(&from.name)
                            .ok_or("mapped generic type binding is missing")?,
                    );
                }
                CallableGenericParamKind::Effect => {
                    destination_bindings.rows.insert(
                        to.name.clone(),
                        mapped
                            .rows
                            .get(&from.name)
                            .cloned()
                            .ok_or("mapped generic effect-row binding is missing")?,
                    );
                }
            }
        }
        let source_signature = instantiate(source, &source_template, &bindings)
            .map_err(|error| format!("generic source signature is not materialized: {error}"))?;
        let destination_signature = instantiate(
            &destination.checked,
            &destination_template,
            &destination_bindings,
        )
        .map_err(|error| format!("generic destination signature is not materialized: {error}"))?;
        Ok(Self {
            presented_types: types,
            presented_rows: rows,
            destination_bindings,
            source_signature,
            destination_signature,
            source_program: source.clone(),
            destination_program: destination.checked,
        })
    }

    pub fn arguments(
        &self,
        call: &ProgramCall,
        program: &CheckedExecutionProgram,
        args: Vec<InterpValue>,
    ) -> Result<Vec<InterpValue>, String> {
        if !Arc::ptr_eq(&program.checked, &self.source_program) {
            return Err(
                "generic invocation arguments belong to a different checked program".into(),
            );
        }
        if args.len() != self.source_signature.input.len() {
            return Err("generic program argument count differs from its checked signature".into());
        }
        let edges = call
            .action_projections()?
            .iter()
            .rev()
            .map(crate::plan::code::RevisionProjection::reversed)
            .collect::<Vec<_>>();
        args.into_iter()
            .zip(&self.source_signature.input)
            .enumerate()
            .map(|(index, (mut value, ty))| {
                let mut ty = *ty;
                for edge in &edges {
                    (ty, value) = edge.value(ty, value, program)?;
                }
                if self.destination_signature.input.get(index) != Some(&ty) {
                    return Err(
                        "generic argument disagrees with its checked destination type".into(),
                    );
                }
                Ok(value)
            })
            .collect()
    }

    pub fn body(&self, mut target: CallTarget) -> Result<CallTarget, String> {
        let mut bindings = self.destination_bindings.clone();
        while let CallTarget::Specialized {
            target: inner,
            type_bindings,
            effect_row_bindings,
        } = target
        {
            for (name, ty) in type_bindings {
                if bindings.get(&name).is_some_and(|old| *old != ty) {
                    return Err("native callable has conflicting generic specializations".into());
                }
                bindings.insert(name, ty);
            }
            for (name, row) in effect_row_bindings {
                if bindings.rows.get(&name).is_some_and(|old| old != &row) {
                    return Err("native callable has conflicting effect-row specializations".into());
                }
                bindings.rows.insert(name, row);
            }
            target = inner.into_value();
        }
        Ok(CallTarget::Specialized {
            target: target.into(),
            type_bindings: bindings.types.into_iter().collect(),
            effect_row_bindings: bindings
                .rows
                .iter()
                .map(|(name, row)| (name.clone(), row.clone()))
                .collect(),
        })
    }

    pub fn result(
        &self,
        call: &ProgramCall,
        program: &CheckedExecutionProgram,
        value: InterpValue,
    ) -> Result<InterpValue, String> {
        let (ty, value) =
            self.project_outgoing(call, program, self.destination_signature.output, value)?;
        if ty != self.source_signature.output {
            return Err("generic result disagrees with its checked source type".into());
        }
        Ok(value)
    }

    pub fn project_outgoing(
        &self,
        call: &ProgramCall,
        program: &CheckedExecutionProgram,
        mut ty: TypeId,
        mut value: InterpValue,
    ) -> Result<(TypeId, InterpValue), String> {
        if !Arc::ptr_eq(&program.checked, &self.destination_program) {
            return Err("generic invocation result belongs to a different checked program".into());
        }
        for edge in call.action_projections()? {
            (ty, value) = edge.value(ty, value, program)?;
        }
        Ok((ty, value))
    }
}

fn flow(signature: &CallableSignature) -> FlowType {
    FlowType {
        input: signature.params.clone(),
        output: signature.output,
        effects: signature.effects.clone(),
    }
}

fn instantiate(
    program: &CheckedProgram,
    signature: &FlowType,
    bindings: &GenericBindings,
) -> Result<FlowType, String> {
    let mut substitution = bindings.substitution(&program.type_store);
    Ok(FlowType {
        input: signature
            .input
            .iter()
            .map(|ty| {
                substitution
                    .substitute(*ty)
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<_, _>>()?,
        output: substitution
            .substitute(signature.output)
            .map_err(|error| error.to_string())?,
        effects: signature
            .effects
            .clone()
            .map(|row| {
                substitution
                    .substitute_effect_row(row)
                    .map_err(|error| error.to_string())
            })
            .transpose()?,
    })
}

fn validate_parameters(
    parameters: &[CallableGenericParam],
    bindings: &GenericBindings,
) -> Result<(), String> {
    if parameters.len() != bindings.types.len() + bindings.rows.len() {
        return Err("program generic bindings differ from the checked binder count".into());
    }
    for parameter in parameters {
        let valid = match parameter.kind {
            CallableGenericParamKind::Type => bindings.types.contains_key(&parameter.name),
            CallableGenericParamKind::Effect => bindings.rows.contains_key(&parameter.name),
        };
        if !valid {
            return Err("program generic binding is missing or has the wrong kind".into());
        }
    }
    Ok(())
}
