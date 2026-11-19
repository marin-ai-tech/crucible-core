//! Ticks and change tracking.

use std::fmt;
use std::ops::{BitOr, BitOrAssign};

use crate::component::Component;

/// Simulation tick number, equivalent to Robust's `GameTick`.
///
/// Tick 0 is reserved for the state before any confirmation. The simulation starts at tick 1.
/// The simulation loop advances the world tick through
/// [`World::set_current_tick`](crate::World::set_current_tick).
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Tick(pub u32);

impl Tick {
    /// Tick 0, before any changes.
    pub const ZERO: Tick = Tick(0);
}

impl fmt::Display for Tick {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Mask of changed fields for a component with per-field deltas. Bit `i` represents field `i`
/// (equivalent to Robust's `ChangedFields`, limited to 64 fields).
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct FieldMask(u64);

impl FieldMask {
    /// An empty mask.
    pub const EMPTY: FieldMask = FieldMask(0);

    /// Mask for one field at `index` (`0..64`). An out-of-range index causes a compile error
    /// during constant evaluation or a panic at runtime.
    pub const fn bit(index: u8) -> FieldMask {
        assert!(index < 64, "field index must be less than 64");
        FieldMask(1 << index)
    }

    /// Mask for the first `count` fields (`count <= 64`).
    pub const fn first(count: usize) -> FieldMask {
        assert!(count <= 64, "a field mask holds at most 64 fields");
        if count == 64 {
            FieldMask(u64::MAX)
        } else {
            FieldMask((1u64 << count) - 1)
        }
    }

    /// Mask from raw bits.
    pub const fn from_bits(bits: u64) -> FieldMask {
        FieldMask(bits)
    }

    /// Raw bits.
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Whether the mask is empty.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Whether the mask contains all bits in `other`.
    pub const fn contains(self, other: FieldMask) -> bool {
        self.0 & other.0 == other.0
    }

    /// Indices of set bits in ascending order.
    pub fn indices(self) -> impl Iterator<Item = u8> {
        (0..64u8).filter(move |i| self.0 & (1 << i) != 0)
    }
}

impl BitOr for FieldMask {
    type Output = FieldMask;
    fn bitor(self, rhs: FieldMask) -> FieldMask {
        FieldMask(self.0 | rhs.0)
    }
}

impl BitOrAssign for FieldMask {
    fn bitor_assign(&mut self, rhs: FieldMask) {
        self.0 |= rhs.0;
    }
}

impl fmt::Debug for FieldMask {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FieldMask({:#b})", self.0)
    }
}

/// Component with per-field deltas. The [`networked`](crate::networked) macro implements this trait.
/// Implement it by hand when a custom field order is required.
pub trait NetworkedComponent: Component {
    /// Replicated field names. Each index is the bit position in [`FieldMask`].
    const FIELD_NAMES: &'static [&'static str];

    /// Mask of all replicated fields.
    fn all_fields() -> FieldMask {
        FieldMask::first(Self::FIELD_NAMES.len())
    }
}
