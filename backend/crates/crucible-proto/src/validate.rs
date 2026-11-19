//! Schema-based validation that accumulates errors across records.

use std::collections::BTreeSet;
use std::sync::Arc;

use crucible_loc::LocKeys;

use crate::diag::{Diagnostic, DiagnosticKind, ProtoRef, Report};
use crate::load::Entry;
use crate::node::{Node, Pos, Value, parse_bool};
use crate::schema::{FieldDef, FieldType, IgnoreCategory, Registry};

pub(crate) struct Ctx<'a> {
    pub registry: &'a Registry,
    /// Checks whether a prototype exists for the given kind and ID.
    pub exists: &'a dyn Fn(&str, &str) -> bool,
    pub loc: Option<&'a dyn LocKeys>,
    pub report: &'a mut Report,
    pub file: Arc<str>,
    pub proto: ProtoRef,
    /// Unimplemented components found in the record.
    pub not_implemented: BTreeSet<String>,
}

impl Ctx<'_> {
    fn emit(&mut self, pos: Pos, kind: DiagnosticKind) {
        self.report.push(Diagnostic {
            file: Some(self.file.clone()),
            pos: Some(pos),
            proto: Some(self.proto.clone()),
            kind,
        });
    }

    fn mismatch(&mut self, ty: &FieldType, node: &Node) {
        self.emit(
            node.pos,
            DiagnosticKind::TypeMismatch {
                expected: ty.to_string(),
                found: describe(node),
            },
        );
    }
}

fn describe(node: &Node) -> String {
    match &node.value {
        Value::Scalar(s) if s.text.is_empty() => "empty value".to_owned(),
        Value::Scalar(s) => format!("`{}`", s.text),
        Value::Seq(_) => "list".to_owned(),
        Value::Map(_) => "mapping".to_owned(),
    }
}

/// Validates reserved fields, kind fields, and components for a single record.
pub(crate) fn validate_entry(entry: &Entry, ctx: &mut Ctx<'_>) {
    let Some(kind) = ctx.registry.kind(&entry.kind) else {
        return;
    };
    let Some(entries) = entry.body.as_map() else {
        return;
    };
    for (key, value) in entries {
        let Some(name) = key.as_str() else { continue };
        match name {
            "type" | "id" => {}
            "abstract" | "parent" if !kind.inheritable => {
                ctx.emit(
                    key.pos,
                    DiagnosticKind::MalformedRecord(format!(
                        "prototype kind `{}` does not support field `{name}`",
                        kind.name
                    )),
                );
            }
            "abstract" => {
                if value.as_str().and_then(parse_bool).is_none() {
                    ctx.mismatch(&FieldType::Bool, value);
                }
            }
            "parent" => validate_parent(value, ctx),
            "components" if kind.has_components => validate_components(value, ctx),
            _ => match kind.fields.iter().find(|f| f.name == name) {
                Some(def) => check_value(
                    &def.ty,
                    value,
                    &format!("prototype kind `{}`", kind.name),
                    ctx,
                ),
                None => ctx.emit(
                    key.pos,
                    DiagnosticKind::UnknownField {
                        owner: format!("prototype kind `{}`", kind.name),
                        field: name.to_owned(),
                    },
                ),
            },
        }
    }
}

fn validate_parent(value: &Node, ctx: &mut Ctx<'_>) {
    let ok = match &value.value {
        Value::Scalar(_) => true,
        Value::Seq(items) => {
            for item in items {
                if item.as_str().is_none() {
                    ctx.mismatch(&FieldType::String, item);
                }
            }
            true
        }
        Value::Map(_) => false,
    };
    if !ok {
        ctx.mismatch(&FieldType::String, value);
    }
}

fn validate_components(value: &Node, ctx: &mut Ctx<'_>) {
    if value.is_null() {
        return;
    }
    let Some(items) = value.as_seq() else {
        ctx.mismatch(&FieldType::List(Box::new(FieldType::Any)), value);
        return;
    };
    for item in items {
        let Some(entries) = item.as_map() else {
            ctx.emit(
                item.pos,
                DiagnosticKind::MalformedRecord("a component record must be a mapping".into()),
            );
            continue;
        };
        let Some(type_node) = item.get("type").filter(|n| n.as_str().is_some()) else {
            ctx.emit(
                item.pos,
                DiagnosticKind::MalformedRecord("component is missing its `type` field".into()),
            );
            continue;
        };
        let name = type_node.as_str().unwrap_or_default();
        if let Some(def) = ctx.registry.component(name) {
            let owner = format!("component `{name}`");
            for (key, v) in entries {
                let Some(field) = key.as_str() else { continue };
                if field == "type" {
                    continue;
                }
                match def.fields.iter().find(|f| f.name == field) {
                    Some(f) => check_value(&f.ty, v, &owner, ctx),
                    None => ctx.emit(
                        key.pos,
                        DiagnosticKind::UnknownField {
                            owner: owner.clone(),
                            field: field.to_owned(),
                        },
                    ),
                }
            }
        } else {
            match ctx.registry.ignore_category(name) {
                Some(IgnoreCategory::ClientOnly) => {}
                Some(IgnoreCategory::NotImplemented) => {
                    ctx.not_implemented.insert(name.to_owned());
                }
                None => ctx.emit(
                    type_node.pos,
                    DiagnosticKind::UnknownComponent {
                        component: name.to_owned(),
                    },
                ),
            }
        }
    }
}

fn check_fields(fields: &[FieldDef], entries: &[(Node, Node)], owner: &str, ctx: &mut Ctx<'_>) {
    for (key, value) in entries {
        let Some(name) = key.as_str() else { continue };
        match fields.iter().find(|f| f.name == name) {
            Some(f) => check_value(&f.ty, value, owner, ctx),
            None => ctx.emit(
                key.pos,
                DiagnosticKind::UnknownField {
                    owner: owner.to_owned(),
                    field: name.to_owned(),
                },
            ),
        }
    }
}

/// Validates a value against its field type. `owner` describes the field owner in diagnostics.
pub(crate) fn check_value(ty: &FieldType, node: &Node, owner: &str, ctx: &mut Ctx<'_>) {
    if matches!(ty, FieldType::Any) {
        return;
    }
    if let Some(tag) = &node.type_tag
        && !matches!(ty, FieldType::Polymorphic(_))
    {
        ctx.emit(
            node.pos,
            DiagnosticKind::UnexpectedTypeTag {
                tag: tag.to_string(),
            },
        );
        return;
    }
    match ty {
        FieldType::Any => {}
        FieldType::String => match node.as_str() {
            Some(_) => {}
            None => ctx.mismatch(ty, node),
        },
        FieldType::LocKey => match node.as_str() {
            Some(key) => {
                if !key.is_empty()
                    && let Some(loc) = ctx.loc
                    && !loc.has_key(key)
                {
                    ctx.emit(
                        node.pos,
                        DiagnosticKind::MissingLocKey {
                            key: key.to_owned(),
                        },
                    );
                }
            }
            None => ctx.mismatch(ty, node),
        },
        FieldType::Bool => {
            if node.as_str().and_then(parse_bool).is_none() {
                ctx.mismatch(ty, node);
            }
        }
        FieldType::Int => {
            if node.as_str().and_then(|s| s.parse::<i64>().ok()).is_none() {
                ctx.mismatch(ty, node);
            }
        }
        FieldType::Float => {
            if node.as_str().and_then(|s| s.parse::<f64>().ok()).is_none() {
                ctx.mismatch(ty, node);
            }
        }
        FieldType::Prototype(kind) => match node.as_str() {
            Some(id) if !id.is_empty() => {
                if !(ctx.exists)(kind, id) {
                    ctx.emit(
                        node.pos,
                        DiagnosticKind::UnknownReference {
                            kind: kind.clone(),
                            id: id.to_owned(),
                        },
                    );
                }
            }
            _ => ctx.mismatch(ty, node),
        },
        FieldType::List(inner) => {
            if node.is_null() {
                return;
            }
            match node.as_seq() {
                Some(items) => items.iter().for_each(|i| check_value(inner, i, owner, ctx)),
                None => ctx.mismatch(ty, node),
            }
        }
        FieldType::Map(inner) => {
            if node.is_null() {
                return;
            }
            match node.as_map() {
                Some(entries) => entries
                    .iter()
                    .for_each(|(_, v)| check_value(inner, v, owner, ctx)),
                None => ctx.mismatch(ty, node),
            }
        }
        FieldType::Struct(fields) => {
            if node.is_null() {
                return;
            }
            match node.as_map() {
                Some(entries) => check_fields(fields, entries, owner, ctx),
                None => ctx.mismatch(ty, node),
            }
        }
        FieldType::Polymorphic(base) => {
            let Some(name) = &node.type_tag else {
                ctx.emit(
                    node.pos,
                    DiagnosticKind::MissingPolymorphicTag { base: base.clone() },
                );
                return;
            };
            let Some(def) = ctx.registry.polymorphic(base, name) else {
                ctx.emit(
                    node.pos,
                    DiagnosticKind::UnknownPolymorphicType {
                        base: base.clone(),
                        name: name.to_string(),
                    },
                );
                return;
            };
            // An empty tagged node is an instance of the type with its default values.
            if node.is_null() {
                return;
            }
            match node.as_map() {
                Some(entries) => check_fields(&def.fields, entries, &format!("type `{name}`"), ctx),
                None => ctx.mismatch(ty, node),
            }
        }
    }
}
