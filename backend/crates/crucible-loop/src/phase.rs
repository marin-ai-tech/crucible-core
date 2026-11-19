//! Round phase machine: Lobby, Preparation, In Round, Results, then back to Lobby.
//! Implements round phase ordering.

use std::fmt;

use crucible_ecs::World;

/// Round phase, corresponding to Robust's `GameRunLevel` with an internal preparation phase.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum RoundPhase {
    /// Lobby, where players gather and mark themselves ready.
    Lobby,
    /// Preparation of maps, rules, roles, and character spawning.
    PreRound,
    /// Round in progress.
    InRound,
    /// Round results.
    PostRound,
}

impl RoundPhase {
    /// The only permitted next phase.
    pub fn next(self) -> RoundPhase {
        match self {
            RoundPhase::Lobby => RoundPhase::PreRound,
            RoundPhase::PreRound => RoundPhase::InRound,
            RoundPhase::InRound => RoundPhase::PostRound,
            RoundPhase::PostRound => RoundPhase::Lobby,
        }
    }

    /// Whether a transition to `to` is allowed.
    pub fn can_transition_to(self, to: RoundPhase) -> bool {
        self.next() == to
    }

    /// Display name for logs and diagnostics.
    pub fn title(self) -> &'static str {
        match self {
            RoundPhase::Lobby => "Lobby",
            RoundPhase::PreRound => "Preparation",
            RoundPhase::InRound => "In Round",
            RoundPhase::PostRound => "Results",
        }
    }
}

impl fmt::Display for RoundPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.title())
    }
}

/// Broadcast event raised on every round phase transition.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RoundPhaseChanged {
    /// Previous phase.
    pub from: RoundPhase,
    /// New phase.
    pub to: RoundPhase,
    /// Round number. It starts at 1 and increments when the lobby enters preparation.
    pub round: u32,
}

/// A rejected transition that left the phase unchanged.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TransitionError {
    /// Current phase.
    pub from: RoundPhase,
    /// Requested phase.
    pub to: RoundPhase,
}

impl fmt::Display for TransitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.from.can_transition_to(self.to) {
            return write!(
                f,
                "round phase transition blocked: {} -> {}",
                self.from, self.to
            );
        }
        write!(
            f,
            "invalid round phase transition: {} -> {} ('{}' can only advance to '{}')",
            self.from,
            self.to,
            self.from,
            self.from.next()
        )
    }
}

impl std::error::Error for TransitionError {}

/// Current round phase exposed to systems as a world resource. [`RoundPhaseMachine`] updates it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CurrentRoundPhase {
    /// Current phase.
    pub phase: RoundPhase,
    /// Current or most recent round number, or 0 before the first round.
    pub round: u32,
}

/// Round phase machine.
#[derive(Debug)]
pub struct RoundPhaseMachine {
    phase: RoundPhase,
    round: u32,
    rejected: Vec<TransitionError>,
}

impl Default for RoundPhaseMachine {
    fn default() -> Self {
        RoundPhaseMachine::new()
    }
}

impl RoundPhaseMachine {
    /// Starts in the lobby before the first round.
    pub fn new() -> Self {
        RoundPhaseMachine {
            phase: RoundPhase::Lobby,
            round: 0,
            rejected: Vec::new(),
        }
    }

    /// Current phase.
    pub fn phase(&self) -> RoundPhase {
        self.phase
    }

    /// Current or most recent round number.
    pub fn round(&self) -> u32 {
        self.round
    }

    /// Rejected transition requests in arrival order, for diagnostics.
    pub fn rejected(&self) -> &[TransitionError] {
        &self.rejected
    }

    /// Transitions to `to` when allowed, updates [`CurrentRoundPhase`], and raises
    /// [`RoundPhaseChanged`]. A rejected transition leaves the phase unchanged and records
    /// the diagnostic in the log and in [`rejected`](RoundPhaseMachine::rejected).
    pub fn transition(&mut self, world: &mut World, to: RoundPhase) -> Result<(), TransitionError> {
        let from = self.phase;
        if !from.can_transition_to(to) {
            let error = TransitionError { from, to };
            tracing::warn!(target: "crucible_loop::round_phase", %error, "round phase transition rejected");
            self.rejected.push(error);
            return Err(error);
        }
        if to == RoundPhase::PreRound {
            self.round += 1;
        }
        self.publish(world, from, to);
        Ok(())
    }

    pub(crate) fn cancel_preparation(&mut self, world: &mut World) {
        debug_assert_eq!(self.phase, RoundPhase::PreRound);
        self.publish(world, RoundPhase::PreRound, RoundPhase::Lobby);
    }

    fn publish(&mut self, world: &mut World, from: RoundPhase, to: RoundPhase) {
        self.phase = to;
        let current = CurrentRoundPhase {
            phase: to,
            round: self.round,
        };
        world.insert_resource(current);
        tracing::info!(target: "crucible_loop::round_phase", round = self.round, "round phase: {from} -> {to}");
        world.raise_broadcast(&mut RoundPhaseChanged {
            from,
            to,
            round: self.round,
        });
    }
}
