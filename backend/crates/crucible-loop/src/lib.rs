//! The `crucible-loop` crate provides fixed ticks, simulation time, timers, reproducible
//! random streams, system ordering, and round phases and flow.
//!
//! The fixed-tick simulation is implemented independently and does not adapt Robust's
//! `GameTiming` or `GameLoop`.
//!
//! # Robust and Crucible API correspondence
//!
//! | Robust / SS14 (C#) | Crucible (Rust) |
//! |---|---|
//! | `GameLoop` (fixed step and catch-up ticks) | [`GameLoop`], [`FixedTimestep`] |
//! | `IGameTiming.CurTick`, `CurTime`, `TickPeriod` | [`SimTime::tick`], [`SimTime::elapsed`], [`SimTime::tick_duration`] |
//! | `Timer.Spawn` or `TimerComponent` on game time | [`WorldTimeExt::start_timer`] |
//! | `IRobustRandom` | [`WorldTimeExt::rng_stream`] (named ChaCha8 streams from the simulation seed) |
//! | `EntitySystem.UpdatesBefore/After` and slower subsystem updates | `crucible-ecs` system ordering and [`EveryN`] |
//! | `GameRunLevel` (`PreRoundLobby`, `InRound`, `PostRound`) | [`RoundPhase`] (`Lobby`, `PreRound`, `InRound`, `PostRound`) |
//! | `GameTicker.StartRound/EndRound/RestartRound` | [`RoundFlow`] |
//! | `GameRunLevelChangedEvent`, `RoundStartingEvent`, `RoundEndTextAppendEvent`, `RoundRestartCleanupEvent` | [`RoundPhaseChanged`], [`RoundStarting`], [`RoundEnded`], [`RoundRestartCleanup`] |
//!
//! # Example
//!
//! ```
//! use std::time::Duration;
//! use crucible_ecs::World;
//! use crucible_loop::{GameLoop, LoopConfig, ManualClock, WorldTimeExt};
//!
//! let (world, systems) = World::builder().build().unwrap();
//! let clock = ManualClock::new();
//! let mut game = GameLoop::new(LoopConfig { seed: Some(42), ..LoopConfig::default() }, world, systems)
//!     .with_clock(Box::new(clock.clone()));
//!
//! clock.advance(Duration::from_millis(100)); // Three ticks at 30 Hz
//! assert_eq!(game.run_iteration().run, 3);
//! assert_eq!(game.world().sim_time().tick().0, 3);
//! ```

mod campaign;
mod game_loop;
mod phase;
mod rng;
mod round;
mod schedule;
mod time;
mod timestep;

pub use campaign::{
    CampaignPersistence, CampaignPersistenceError, CampaignRevision, CampaignRound,
};
pub use game_loop::{GameLoop, PhaseHandler, TickPhase};
pub use phase::{
    CurrentRoundPhase, RoundPhase, RoundPhaseChanged, RoundPhaseMachine, TransitionError,
};
pub use rng::{ChaCha8Rng, RngCore, SeedableRng, SimRng};
pub use round::{
    CampaignState, DEFAULT_ROLE, DEFAULT_RULE_ID, DefaultRule, EmptyEntitySpawn, EventDirector,
    FirstEligibleRule, GameRule, LateAdmissions, LateJoinError, LobbyCountdown, MapSelector,
    NoMaps, ObjectiveOutcome, Participant, RoundConfig, RoundEndPolicy, RoundEndReason,
    RoundEndSource, RoundEnded, RoundFlow, RoundPoints, RoundPreparation, RoundPreparationFailure,
    RoundPreparationHook, RoundRestartCleanup, RoundResultsHook, RoundStarted, RoundStarting,
    RoundSummary, RuleSelector, SpawnPolicy, TimerRoundEnd, clear_world,
};
pub use schedule::EveryN;
pub use time::{SimTime, TimerId, Timers, WorldTimeExt};
pub use timestep::{
    Clock, DEFAULT_MAX_CATCH_UP_TICKS, DEFAULT_TICK_RATE, FixedTimestep, LoopConfig, LoopMetrics,
    ManualClock, Steps, SystemClock,
};
