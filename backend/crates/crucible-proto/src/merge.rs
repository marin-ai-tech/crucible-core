//! Prototype inheritance and merging.
//!
//! Components merge by name. Fields of components and prototypes are
//! overridden by the child. Untagged nested mappings merge recursively
//! like Robust's `PushInheritance`, while lists and scalars are replaced in full.
//! With multiple parents, a later parent overrides an earlier one.

use crate::node::{Node, Value};
use crate::set::Component;

/// Reserved record keys that do not describe fields.
pub(crate) const META_KEYS: [&str; 5] = ["type", "id", "parent", "abstract", "components"];

/// Fields and components of a single record, either original or merged.
#[derive(Clone, Default)]
pub(crate) struct Merged {
    pub fields: Vec<(Box<str>, Node)>,
    pub components: Vec<Component>,
}

/// The record's own fields and components. The validator reports malformed shapes separately.
pub(crate) fn own_parts(body: &Node) -> Merged {
    let mut out = Merged::default();
    let Some(entries) = body.as_map() else {
        return out;
    };
    for (key, value) in entries {
        let Some(name) = key.as_str() else { continue };
        if name == "components" {
            for item in value.as_seq().unwrap_or_default() {
                if let Some(c) = component_of(item) {
                    merge_component(&mut out.components, c);
                }
            }
        } else if !META_KEYS.contains(&name) {
            out.fields.push((name.into(), value.clone()));
        }
    }
    out
}

/// Turns `{ type: Name, field: value }` into a component without the `type` field.
pub(crate) fn component_of(item: &Node) -> Option<Component> {
    let entries = item.as_map()?;
    let type_node = item.get("type")?;
    let name = type_node.as_str()?;
    let rest = entries
        .iter()
        .filter(|(k, _)| k.as_str() != Some("type"))
        .cloned()
        .collect();
    Some(Component {
        name: name.into(),
        pos: type_node.pos,
        data: Node {
            type_tag: None,
            pos: item.pos,
            value: Value::Map(rest),
        },
    })
}

/// Merges `over` onto `base`.
pub(crate) fn merge(base: &mut Merged, over: &Merged) {
    for (key, value) in &over.fields {
        match base.fields.iter_mut().find(|(k, _)| k == key) {
            Some((_, slot)) => *slot = merge_node(slot, value),
            None => base.fields.push((key.clone(), value.clone())),
        }
    }
    for c in &over.components {
        merge_component(&mut base.components, c.clone());
    }
}

fn merge_component(list: &mut Vec<Component>, over: Component) {
    match list.iter_mut().find(|c| c.name == over.name) {
        Some(slot) => {
            slot.data = merge_node(&slot.data, &over.data);
            slot.pos = over.pos;
        }
        None => list.push(over),
    }
}

/// Combines untagged mappings by key and replaces other values with `over`.
pub(crate) fn merge_node(base: &Node, over: &Node) -> Node {
    if base.type_tag.is_some() || over.type_tag.is_some() {
        return over.clone();
    }
    let (Value::Map(b), Value::Map(o)) = (&base.value, &over.value) else {
        return over.clone();
    };
    let mut entries = b.clone();
    for (key, value) in o {
        let existing = key
            .as_str()
            .and_then(|name| entries.iter_mut().find(|(k, _)| k.as_str() == Some(name)));
        match existing {
            Some((_, slot)) => *slot = merge_node(slot, value),
            None => entries.push((key.clone(), value.clone())),
        }
    }
    Node {
        type_tag: None,
        pos: over.pos,
        value: Value::Map(entries),
    }
}
