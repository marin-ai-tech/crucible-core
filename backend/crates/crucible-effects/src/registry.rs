use std::collections::HashMap;

use crucible_ext::{CoreModule, RegisterContext};
use crucible_proto::{FieldDef, FieldType, Node};

use crate::{Condition, Effect, EffectBlock};

struct EntityCondition;
struct EntityEffect;

type Decoder<T> = fn(&Node) -> Result<Box<T>, String>;

/// Core module that provides a resource for registering types and reading blocks.
pub struct EffectsModule;

impl CoreModule for EffectsModule {
    fn id(&self) -> &'static str {
        "effects"
    }

    fn register(&self, ctx: &mut RegisterContext<'_>) {
        ctx.resource(EffectRegistry::new());
    }
}

/// Handlers for polymorphic types registered by modules and extensions.
pub struct EffectRegistry {
    conditions: HashMap<String, Decoder<dyn Condition>>,
    effects: HashMap<String, Decoder<dyn Effect>>,
}

impl EffectRegistry {
    fn new() -> Self {
        Self {
            conditions: HashMap::new(),
            effects: HashMap::new(),
        }
    }

    /// Registers a condition schema and decoder. The registering module or extension
    /// must declare a dependency on `effects`.
    pub fn register_condition(
        ctx: &mut RegisterContext<'_>,
        name: &str,
        fields: Vec<FieldDef>,
        decode: Decoder<dyn Condition>,
    ) {
        ctx.polymorphic_type_with_resource::<EntityCondition, Self>(name, fields, |registry| {
            registry.conditions.insert(name.to_owned(), decode);
        });
    }

    /// Registers an effect schema and decoder.
    pub fn register_effect(
        ctx: &mut RegisterContext<'_>,
        name: &str,
        fields: Vec<FieldDef>,
        decode: Decoder<dyn Effect>,
    ) {
        ctx.polymorphic_type_with_resource::<EntityEffect, Self>(name, fields, |registry| {
            registry.effects.insert(name.to_owned(), decode);
        });
    }

    /// Block schema for a component or prototype field defined by the mechanic's owner.
    pub fn field_type() -> FieldType {
        FieldType::Struct(vec![
            FieldDef::new("conditions", list_type::<EntityCondition>()),
            FieldDef::new("effects", list_type::<EntityEffect>()),
        ])
    }

    /// Reads a block from the final node of a validated prototype.
    pub fn decode(&self, node: &Node) -> Result<EffectBlock, String> {
        if !node.is_null() && node.as_map().is_none() {
            return Err(format!("{}: an effect block must be a mapping", node.pos));
        }
        Ok(EffectBlock::new(
            decode_list(node.get("conditions"), "conditions", &self.conditions)?,
            decode_list(node.get("effects"), "effects", &self.effects)?,
        ))
    }
}

fn list_type<Scope: 'static>() -> FieldType {
    FieldType::List(Box::new(
        RegisterContext::polymorphic_resource_field::<Scope>(),
    ))
}

fn decode_list<T: ?Sized>(
    node: Option<&Node>,
    field: &str,
    decoders: &HashMap<String, Decoder<T>>,
) -> Result<Vec<Box<T>>, String> {
    let Some(node) = node.filter(|node| !node.is_null()) else {
        return Ok(Vec::new());
    };
    let items = node
        .as_seq()
        .ok_or_else(|| format!("{}: field `{field}` must be a list", node.pos))?;
    items
        .iter()
        .map(|item| {
            let name = item.type_tag.as_deref().ok_or_else(|| {
                format!("{}: field `{field}` requires a !type:Name tag", item.pos)
            })?;
            let decode = decoders.get(name).ok_or_else(|| {
                format!(
                    "{}: type `{name}` is not registered for `{field}`",
                    item.pos
                )
            })?;
            decode(item).map_err(|error| format!("{}: type `{name}`: {error}", item.pos))
        })
        .collect()
}
