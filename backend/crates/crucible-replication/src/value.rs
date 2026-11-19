//! Transport-independent component and network event values.
//! Protocol adapters encode these values for the wire.
//!
//! Struct fields retain names and values independently of network IDs and wire field order.
//! Robust maps names to C# type fields using its exported schema. CWP uses the
//! field descriptors in its handshake table.

use std::borrow::Cow;
use std::sync::Arc;

use crucible_ecs::{FieldMask, NetEntity};

/// Field name, normally a `&'static str` from a component schema.
/// Values supplied by an adapter may use an owned string.
pub type FieldName = Cow<'static, str>;

/// Value independent of a protocol's wire format.
///
/// Numeric variants preserve width, including distinct `F32` and `F64` variants.
/// Adapters can serialize their bits exactly. Physics and transforms use `f32`, and rotations use `f64`.
#[derive(Clone, Debug, PartialEq)]
pub enum NeutralValue {
    /// Absent value (`null` in C# or `None` in Rust).
    Null,
    /// Boolean.
    Bool(bool),
    /// Signed integer.
    Int(i64),
    /// Unsigned integer.
    UInt(u64),
    /// Single-precision floating-point number.
    F32(f32),
    /// Double-precision floating-point number.
    F64(f64),
    /// String, such as a prototype identifier or name.
    Str(Arc<str>),
    /// Raw bytes.
    Bytes(Arc<[u8]>),
    /// Entity reference using only its network ID. A server-side `EntityUid`
    /// never leaves the server.
    Entity(NetEntity),
    /// Two-dimensional `f32` vector for positions or velocities.
    Vec2([f32; 2]),
    /// List of values.
    List(Vec<NeutralValue>),
    /// Struct with named fields in insertion order.
    Struct(Vec<(FieldName, NeutralValue)>),
}

impl NeutralValue {
    /// Entity reference or [`NeutralValue::Null`].
    pub fn entity(net: Option<NetEntity>) -> NeutralValue {
        net.map_or(NeutralValue::Null, NeutralValue::Entity)
    }

    /// String value.
    pub fn str(text: impl Into<Arc<str>>) -> NeutralValue {
        NeutralValue::Str(text.into())
    }

    /// Looks up a struct field. Returns `None` if the value is not a struct or the field is absent.
    pub fn field(&self, name: &str) -> Option<&NeutralValue> {
        match self {
            NeutralValue::Struct(fields) => fields.iter().find(|(n, _)| n == name).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Field names in order. The iterator is empty for non-struct values.
    pub fn field_names(&self) -> Vec<&str> {
        match self {
            NeutralValue::Struct(fields) => fields.iter().map(|(n, _)| n.as_ref()).collect(),
            _ => Vec::new(),
        }
    }
}

/// Fields to serialize into a component state.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum FieldSelection {
    /// All fields, for full component state.
    All,
    /// Only fields selected by the mask, for a per-field delta.
    Fields(FieldMask),
}

impl FieldSelection {
    /// Whether the field at index `bit` in the component's `FIELD_NAMES` array is selected.
    pub fn includes(self, bit: u8) -> bool {
        match self {
            FieldSelection::All => true,
            FieldSelection::Fields(mask) => mask.contains(FieldMask::bit(bit)),
        }
    }
}

/// Serializer helper that builds a [`NeutralValue::Struct`] from the fields in
/// [`FieldSelection`]. Names resolve to indices in `NetworkedComponent::FIELD_NAMES`
/// so serializers do not need mask constants.
///
/// ```
/// use crucible_ecs::FieldMask;
/// use crucible_replication::{FieldSelection, NeutralValue, StructWriter};
///
/// const NAMES: &[&str] = &["open", "speed"];
/// let mut w = StructWriter::new(NAMES, FieldSelection::Fields(FieldMask::bit(1)));
/// w.field("open", || NeutralValue::Bool(true));
/// w.field("speed", || NeutralValue::F32(2.5));
/// assert_eq!(w.finish().field_names(), vec!["speed"]);
/// ```
pub struct StructWriter {
    names: &'static [&'static str],
    selection: FieldSelection,
    fields: Vec<(FieldName, NeutralValue)>,
}

impl StructWriter {
    /// Builds a writer for `names` and `selection`. A name's index is its mask bit.
    pub fn new(names: &'static [&'static str], selection: FieldSelection) -> Self {
        StructWriter {
            names,
            selection,
            fields: Vec::with_capacity(names.len()),
        }
    }

    /// Computes and writes the field only if selected. Fields absent from `names`
    /// do not use per-field deltas and are always written.
    pub fn field(&mut self, name: &'static str, value: impl FnOnce() -> NeutralValue) -> &mut Self {
        let selected = match self.names.iter().position(|n| *n == name) {
            Some(bit) => self.selection.includes(bit as u8),
            None => true,
        };
        if selected {
            self.fields.push((Cow::Borrowed(name), value()));
        }
        self
    }

    /// Finishes the struct.
    pub fn finish(self) -> NeutralValue {
        NeutralValue::Struct(self.fields)
    }
}
