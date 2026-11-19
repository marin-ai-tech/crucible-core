//! Prototype-defined conditions and effects that operate on entities.

mod execute;
mod registry;

pub use execute::{ApplyOutcome, Condition, Effect, EffectBlock};
pub use registry::{EffectRegistry, EffectsModule};
