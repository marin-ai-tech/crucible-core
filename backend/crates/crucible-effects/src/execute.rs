use crucible_ecs::{EntityUid, World};

/// A check of world state for a target entity.
pub trait Condition: Send + Sync {
    /// Returns whether the target satisfies the condition.
    fn evaluate(&self, world: &World, target: EntityUid) -> bool;
}

/// An operation on a target entity in the world.
pub trait Effect: Send + Sync {
    /// Applies the effect to the target synchronously.
    fn apply(&self, world: &mut World, target: EntityUid);
}

/// The outcome of running an effect block.
#[derive(Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// The block completed and all effects ran.
    Applied,
    /// A condition returned false, so the effects were skipped.
    ConditionNotMet,
    /// The target was missing or being removed before a condition or effect ran.
    /// Any effects already applied are retained.
    TargetUnavailable,
}

/// Prepared conditions and effects in declaration order.
pub struct EffectBlock {
    conditions: Vec<Box<dyn Condition>>,
    effects: Vec<Box<dyn Effect>>,
}

impl EffectBlock {
    pub(crate) fn new(conditions: Vec<Box<dyn Condition>>, effects: Vec<Box<dyn Effect>>) -> Self {
        Self {
            conditions,
            effects,
        }
    }

    /// Checks the conditions and applies the effects to the target immediately.
    ///
    /// Conditions are checked on every call. The first false result
    /// stops evaluation before any effects are applied.
    pub fn apply(&self, world: &mut World, target: EntityUid) -> ApplyOutcome {
        if world.terminating_or_deleted(target) {
            return ApplyOutcome::TargetUnavailable;
        }
        if !self
            .conditions
            .iter()
            .all(|condition| condition.evaluate(world, target))
        {
            return ApplyOutcome::ConditionNotMet;
        }
        for effect in &self.effects {
            if world.terminating_or_deleted(target) {
                return ApplyOutcome::TargetUnavailable;
            }
            effect.apply(world, target);
        }
        ApplyOutcome::Applied
    }
}
