//! Confirmed campaign state and retryable result commits.

use std::fmt;

use crucible_ecs::Tick;

use crate::round::CampaignState;
use crate::time::SimTime;

/// A confirmed revision and its state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CampaignRevision {
    /// Revision in campaign storage.
    pub revision: u64,
    /// Revision data after compatibility checks.
    pub state: CampaignState,
}

/// Result identifier and base revision for one round.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CampaignRound {
    /// Stable result identifier, unique across rounds in a campaign.
    pub result_id: String,
    /// Revision used to prepare the round.
    pub base: CampaignRevision,
}

/// Campaign read or write error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CampaignPersistenceError {
    /// The operation can be retried with the same input. The write acknowledgement may have been lost.
    Retryable(String),
    /// The operation needs intervention. Automatic retries are disabled and the result is required.
    Rejected(String),
}

impl fmt::Display for CampaignPersistenceError {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Retryable(message) | Self::Rejected(message) => fmt.write_str(message),
        }
    }
}

impl std::error::Error for CampaignPersistenceError {}

/// Storage adapter for one campaign.
pub trait CampaignPersistence: Send {
    /// Reads a confirmed revision, applies migrations, and allocates a result identifier.
    /// Storage must keep identifiers unique across restarts and concurrent runs.
    fn begin_round(&mut self) -> Result<CampaignRound, CampaignPersistenceError>;

    /// Atomically saves a new revision and a journal record.
    /// Repeating an identifier with the same input returns the already confirmed revision.
    /// An unknown identifier with a stale base revision is rejected.
    fn commit_round(
        &mut self,
        round: &CampaignRound,
        staged: &CampaignState,
    ) -> Result<CampaignRevision, CampaignPersistenceError>;
}

struct PendingResult {
    round: CampaignRound,
    staged: CampaignState,
}

pub(crate) struct DurableCampaign {
    pub(crate) revision: Option<u64>,
    pub(crate) error: Option<CampaignPersistenceError>,
    persistence: Box<dyn CampaignPersistence>,
    round: Option<CampaignRound>,
    pending: Option<PendingResult>,
    retry_at: Option<Tick>,
}

impl DurableCampaign {
    pub(crate) fn new(persistence: Box<dyn CampaignPersistence>) -> Self {
        Self {
            persistence,
            revision: None,
            error: None,
            round: None,
            pending: None,
            retry_at: None,
        }
    }

    pub(crate) fn can_auto_attempt(&self, now: Tick) -> bool {
        self.error.is_none() || self.retry_at.is_some_and(|at| now >= at)
    }

    pub(crate) fn begin_round(&mut self, time: SimTime) -> Option<CampaignState> {
        match self.persistence.begin_round() {
            Ok(round) => {
                self.revision = Some(round.base.revision);
                self.clear_error();
                let state = round.base.state.clone();
                self.round = Some(round);
                Some(state)
            }
            Err(error) => {
                tracing::error!(target: "crucible_loop::campaign", %error, "could not load campaign for this round");
                self.record_error(error, time);
                None
            }
        }
    }

    pub(crate) fn stage_result(&mut self, changed: Option<CampaignState>) {
        let round = self
            .round
            .take()
            .expect("preparation allocated a campaign result identifier");
        self.pending = changed.map(|staged| PendingResult { round, staged });
    }

    pub(crate) fn commit_pending(&mut self, time: SimTime) -> Option<CampaignState> {
        if !self.can_auto_attempt(time.tick()) {
            return None;
        }
        let pending = self.pending.as_ref()?;
        match self
            .persistence
            .commit_round(&pending.round, &pending.staged)
        {
            Ok(confirmed) => {
                self.revision = Some(confirmed.revision);
                self.pending = None;
                self.clear_error();
                Some(confirmed.state)
            }
            Err(error) => {
                tracing::error!(
                    target: "crucible_loop::campaign",
                    result_id = %pending.round.result_id,
                    %error,
                    "campaign result write was not confirmed"
                );
                self.record_error(error, time);
                None
            }
        }
    }

    pub(crate) fn is_commit_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub(crate) fn discard_round(&mut self) {
        self.round = None;
    }

    fn record_error(&mut self, error: CampaignPersistenceError, time: SimTime) {
        // Avoid retrying expensive I/O every tick immediately after a failure.
        self.retry_at = matches!(error, CampaignPersistenceError::Retryable(_))
            .then(|| Tick(time.tick().0.saturating_add(time.tick_rate())));
        self.error = Some(error);
    }

    fn clear_error(&mut self) {
        self.error = None;
        self.retry_at = None;
    }
}
