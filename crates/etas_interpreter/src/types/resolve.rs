use std::collections::{HashMap, HashSet};

use etas_types::{
    EffectArgRef, EffectRowRef, HandlerProducedEffects, ResourceHandleType, Type, TypeId,
};

use super::{
    CheckedTypeCatalog, CheckedTypeIdentity, TypeHead,
    identity::{BinaryType, EffectHead, StaticArgument, UnaryType},
};

struct NodeInput {
    head: TypeHead,
    children: Vec<TypeId>,
}

impl CheckedTypeCatalog {
    pub(crate) fn resolve(
        &self,
        template: TypeId,
        types: &HashMap<String, CheckedTypeIdentity>,
        rows: &HashMap<String, EffectRowRef>,
    ) -> Result<CheckedTypeIdentity, String> {
        let mut memo = HashMap::<TypeId, CheckedTypeIdentity>::new();
        let mut active = HashSet::new();
        let mut pending = vec![(template, None)];
        while let Some((current, input)) = pending.pop() {
            if memo.contains_key(&current) {
                continue;
            }
            if let Some(NodeInput { head, children }) = input {
                let arguments = children
                    .into_iter()
                    .map(|child| {
                        memo.get(&child)
                            .cloned()
                            .ok_or("checked type expression has an unresolved child".to_owned())
                    })
                    .collect::<Result<_, _>>()?;
                memo.insert(
                    current,
                    CheckedTypeIdentity::new(self.compilation, head, arguments)?,
                );
                active.remove(&current);
                continue;
            }
            if !active.insert(current) {
                return Err("cyclic structural type expression".into());
            }
            let data = self
                .program
                .type_store
                .get(current)
                .ok_or("type template is absent from checked facts")?;
            if let Type::Named(parameter) = data
                && let Some(argument) = types.get(&parameter.name)
            {
                self.validate_identity(argument)?;
                memo.insert(current, argument.clone());
                active.remove(&current);
                continue;
            }
            let input = self.node_input(current, data, rows)?;
            let children = input.children.clone();
            pending.push((current, Some(input)));
            pending.extend(children.into_iter().rev().map(|child| (child, None)));
        }
        memo.remove(&template)
            .ok_or("checked type resolver produced no root".into())
    }

    fn node_input(
        &self,
        template: TypeId,
        data: &Type,
        rows: &HashMap<String, EffectRowRef>,
    ) -> Result<NodeInput, String> {
        let mut children = Vec::new();
        let head = match data {
            Type::Primitive(primitive) => TypeHead::Primitive(*primitive),
            Type::Nominal(_) | Type::Enum(_) | Type::Named(_) => {
                TypeHead::Declaration(self.declaration(template, 0)?)
            }
            Type::Applied { constructor, args } => {
                children.extend(args);
                TypeHead::Declaration(self.declaration(TypeId(constructor.0), args.len())?)
            }
            Type::Array(child)
            | Type::List(child)
            | Type::Set(child)
            | Type::Range { index: child }
            | Type::Slice(child)
            | Type::Option(child)
            | Type::Schema(child)
            | Type::Message(child)
            | Type::MemorySelection(child)
            | Type::MemoryRegion(child) => {
                children.push(*child);
                TypeHead::Unary(match data {
                    Type::Array(_) => UnaryType::Array,
                    Type::List(_) => UnaryType::List,
                    Type::Set(_) => UnaryType::Set,
                    Type::Range { .. } => UnaryType::Range,
                    Type::Slice(_) => UnaryType::Slice,
                    Type::Option(_) => UnaryType::Option,
                    Type::Schema(_) => UnaryType::Schema,
                    Type::Message(_) => UnaryType::Message,
                    Type::MemorySelection(_) => UnaryType::MemorySelection,
                    Type::MemoryRegion(_) => UnaryType::MemoryRegion,
                    _ => return Err("inconsistent checked unary type".into()),
                })
            }
            Type::Map { key, value }
            | Type::Store { key, value }
            | Type::Result {
                ok: key,
                err: value,
            } => {
                children.extend([*key, *value]);
                TypeHead::Binary(match data {
                    Type::Map { .. } => BinaryType::Map,
                    Type::Store { .. } => BinaryType::Store,
                    Type::Result { .. } => BinaryType::Result,
                    _ => return Err("inconsistent checked binary type".into()),
                })
            }
            Type::Tuple(types) => {
                children.extend(types);
                TypeHead::Tuple
            }
            Type::Record(record) => {
                let mut names = HashSet::new();
                for field in &record.fields {
                    if !names.insert(&field.name) {
                        return Err("checked record type has duplicate field names".into());
                    }
                    children.push(field.ty);
                }
                TypeHead::Record(
                    record
                        .fields
                        .iter()
                        .map(|field| field.name.clone())
                        .collect(),
                )
            }
            Type::Function(flow) => {
                children.extend(&flow.input);
                children.push(flow.output);
                let effects = flow
                    .effects
                    .as_ref()
                    .map(|row| self.row_head(row, rows, &mut children))
                    .transpose()?;
                TypeHead::Function {
                    inputs: flow.input.len(),
                    effects,
                }
            }
            Type::Handler(handler) => {
                let handled = self.row_head(&handler.handled, rows, &mut children)?;
                let HandlerProducedEffects::Explicit(produced) = &handler.produced else {
                    return Err("runtime handler type has unresolved produced effects".into());
                };
                let produced = self.row_head(produced, rows, &mut children)?;
                children.extend(handler.result);
                TypeHead::Handler {
                    handled,
                    produced,
                    has_result: handler.result.is_some(),
                }
            }
            Type::Refined { base, predicate } => {
                children.push(*base);
                TypeHead::Refined {
                    compilation: self.compilation,
                    predicate: predicate.0,
                }
            }
            Type::Trust { wrapper, inner } => {
                children.push(*inner);
                TypeHead::Trust(*wrapper)
            }
            Type::Prompt => TypeHead::Prompt,
            Type::PromptPart => TypeHead::PromptPart,
            Type::MemoryPlace(place) => TypeHead::MemoryPlace(place.segments.clone()),
            Type::ResourceHandle(ResourceHandleType::MemoryRegion { schema }) => {
                children.push(*schema);
                TypeHead::Unary(UnaryType::RegionHandle)
            }
            Type::ResourceHandle(ResourceHandleType::ExternalTool { signature }) => {
                children.push(*signature);
                TypeHead::Unary(UnaryType::ToolHandle)
            }
            Type::ResourceHandle(ResourceHandleType::Other { name, args }) => {
                children.extend(args);
                TypeHead::Resource(name.clone())
            }
            Type::IntegerLiteral { .. } | Type::Var(_) => {
                return Err("runtime type expression contains an unresolved type variable".into());
            }
        };
        Ok(NodeInput { head, children })
    }

    fn row_head(
        &self,
        root: &EffectRowRef,
        bindings: &HashMap<String, EffectRowRef>,
        children: &mut Vec<TypeId>,
    ) -> Result<Vec<EffectHead>, String> {
        let mut row = root;
        let mut seen = HashSet::new();
        let mut result = Vec::new();
        loop {
            for effect in &row.effects {
                let arguments = effect
                    .args
                    .iter()
                    .map(|arg| match arg {
                        EffectArgRef::Type(ty) => {
                            let position = children.len();
                            children.push(*ty);
                            StaticArgument::Type(position)
                        }
                        EffectArgRef::Path(path) => StaticArgument::Path(path.clone()),
                        EffectArgRef::String(value) => StaticArgument::String(value.clone()),
                        EffectArgRef::Int(value) => StaticArgument::Int(value.clone()),
                        EffectArgRef::Wildcard => StaticArgument::Wildcard,
                    })
                    .collect();
                result.push(EffectHead {
                    name: effect.name.clone(),
                    arguments,
                });
            }
            let Some(tail) = &row.tail else { break };
            if !seen.insert(tail) {
                return Err("cyclic runtime effect row binding".into());
            }
            row = bindings
                .get(tail)
                .ok_or("runtime type expression has an open effect row")?;
        }
        Ok(result)
    }
}
