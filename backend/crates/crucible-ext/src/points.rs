//! Core extension points.
//!
//! Each extension point is a typed slot with a default implementation. A module or extension
//! can make a point mandatory with [`ExtensionPoints::require`]. Startup fails with a list
//! of mandatory points that have no explicit implementation.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use crucible_ecs::{EntityUid, World};
use crucible_loop::{
    EventDirector, MapSelector, RoundEndPolicy, RoundPoints, RoundPreparationHook,
    RoundResultsHook, RuleSelector, SpawnPolicy,
};

/// An extension point.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ExtensionPointKind {
    /// Selects maps and generates outer zones. By default, it loads no maps.
    MapSelection,
    /// Defines how empty tiles behave. By default, they have no effect.
    VoidTile,
    /// Spawns player characters. By default, it creates an empty entity.
    SpawnPolicy,
    /// Ends a round normally. The default uses a timer from the server configuration.
    RoundEnd,
    /// Selects round rules. The default chooses the first eligible rule or the fallback rule.
    RuleSelection,
    /// Directs events. By default, it schedules none.
    EventDirector,
    /// Updates campaign state from round results. The default has no campaign.
    RoundResults,
    /// Prepares a round using campaign state. The default has no campaign.
    RoundPreparation,
}

impl ExtensionPointKind {
    /// All extension points.
    pub const ALL: [ExtensionPointKind; 8] = [
        ExtensionPointKind::MapSelection,
        ExtensionPointKind::VoidTile,
        ExtensionPointKind::SpawnPolicy,
        ExtensionPointKind::RoundEnd,
        ExtensionPointKind::RuleSelection,
        ExtensionPointKind::EventDirector,
        ExtensionPointKind::RoundResults,
        ExtensionPointKind::RoundPreparation,
    ];

    /// Human-readable name for diagnostics.
    pub fn title(self) -> &'static str {
        match self {
            ExtensionPointKind::MapSelection => "map selection and outer zone generation",
            ExtensionPointKind::VoidTile => "empty tile behavior",
            ExtensionPointKind::SpawnPolicy => "player spawn policy",
            ExtensionPointKind::RoundEnd => "normal round end",
            ExtensionPointKind::RuleSelection => "round rule selection",
            ExtensionPointKind::EventDirector => "event director",
            ExtensionPointKind::RoundResults => "round results to campaign state",
            ExtensionPointKind::RoundPreparation => "campaign state to round preparation",
        }
    }
}

impl fmt::Display for ExtensionPointKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.title())
    }
}

/// Defines what happens when an entity is over an empty tile.
/// The map module calls this policy. [`NoVoidEffect`] is the no-op default.
pub trait VoidTilePolicy: Send + Sync {
    /// An entity moved over an empty tile.
    fn over_void(&self, world: &mut World, entity: EntityUid);
}

/// Default policy that does nothing over an empty tile.
pub struct NoVoidEffect;
impl VoidTilePolicy for NoVoidEffect {
    fn over_void(&self, _: &mut World, _: EntityUid) {}
}

/// World resource containing the active empty tile policy.
#[derive(Clone)]
pub struct VoidTile(pub Arc<dyn VoidTilePolicy>);

/// Migrates server-side campaign data to the next version.
#[derive(Clone, Copy)]
pub struct CampaignDataMigration {
    pub from_version: u32,
    pub apply: fn(&mut BTreeMap<String, String>) -> Result<(), String>,
}

/// The active extension's campaign and data compatibility rules.
#[derive(Clone)]
pub struct CampaignDefinition {
    pub name: String,
    pub format_version: u32,
    pub initial_values: BTreeMap<String, String>,
    pub migrations: Vec<CampaignDataMigration>,
    pub validate: fn(&BTreeMap<String, String>) -> Result<(), String>,
}

/// Extension slots filled by modules first, then the extension.
/// Later implementations replace earlier ones. Campaign hooks accumulate.
#[derive(Default)]
pub struct ExtensionPoints {
    round: RoundPoints,
    campaign: Option<CampaignDefinition>,
    void_tile: Option<Arc<dyn VoidTilePolicy>>,
    filled: BTreeSet<ExtensionPointKind>,
    required: BTreeMap<ExtensionPointKind, Vec<String>>,
}

impl ExtensionPoints {
    /// Creates empty slots that use the default implementations.
    pub fn new() -> Self {
        ExtensionPoints::default()
    }

    /// Sets map selection and outer zone generation.
    pub fn set_map_selector(&mut self, selector: impl MapSelector + 'static) -> &mut Self {
        self.round.map_selector = Some(Box::new(selector));
        self.fill(ExtensionPointKind::MapSelection)
    }

    /// Sets empty tile behavior.
    pub fn set_void_tile(&mut self, policy: impl VoidTilePolicy + 'static) -> &mut Self {
        self.void_tile = Some(Arc::new(policy));
        self.fill(ExtensionPointKind::VoidTile)
    }

    /// Sets the player spawn policy.
    pub fn set_spawn_policy(&mut self, policy: impl SpawnPolicy + 'static) -> &mut Self {
        self.round.spawn_policy = Some(Box::new(policy));
        self.fill(ExtensionPointKind::SpawnPolicy)
    }

    /// Sets the normal round end policy.
    pub fn set_round_end(&mut self, policy: impl RoundEndPolicy + 'static) -> &mut Self {
        self.round.round_end = Some(Box::new(policy));
        self.fill(ExtensionPointKind::RoundEnd)
    }

    /// Sets round rule selection.
    pub fn set_rule_selector(&mut self, selector: impl RuleSelector + 'static) -> &mut Self {
        self.round.rule_selector = Some(Box::new(selector));
        self.fill(ExtensionPointKind::RuleSelection)
    }

    /// Sets the event director.
    pub fn set_event_director(&mut self, director: impl EventDirector + 'static) -> &mut Self {
        self.round.event_director = Some(Box::new(director));
        self.fill(ExtensionPointKind::EventDirector)
    }

    /// Adds a hook that updates campaign state from round results.
    pub fn add_round_results_hook(&mut self, hook: impl RoundResultsHook + 'static) -> &mut Self {
        self.round.results_hooks.push(Box::new(hook));
        self.fill(ExtensionPointKind::RoundResults)
    }

    /// Adds a hook that prepares the round from campaign state.
    pub fn add_round_preparation_hook(
        &mut self,
        hook: impl RoundPreparationHook + 'static,
    ) -> &mut Self {
        self.round.preparation_hooks.push(Box::new(hook));
        self.fill(ExtensionPointKind::RoundPreparation)
    }

    /// Declares campaign data that the server must persist.
    pub fn set_campaign_definition(&mut self, campaign: CampaignDefinition) -> &mut Self {
        self.campaign = Some(campaign);
        self
    }

    pub(crate) fn campaign_definition(&self) -> Option<&CampaignDefinition> {
        self.campaign.as_ref()
    }

    /// Marks a point as mandatory. Startup fails if it has no implementation.
    pub fn require(&mut self, point: ExtensionPointKind, by: &str) -> &mut Self {
        self.required.entry(point).or_default().push(by.to_owned());
        self
    }

    /// Checks whether a point has an explicit implementation.
    pub fn is_filled(&self, point: ExtensionPointKind) -> bool {
        self.filled.contains(&point)
    }

    /// Checks whether any campaign hooks are present.
    pub fn has_campaign(&self) -> bool {
        self.campaign.is_some()
            || self.is_filled(ExtensionPointKind::RoundResults)
            || self.is_filled(ExtensionPointKind::RoundPreparation)
    }

    /// Returns unfilled mandatory points and the sources that require them.
    pub fn missing_required(&self) -> Vec<(ExtensionPointKind, Vec<String>)> {
        self.required
            .iter()
            .filter(|(point, _)| !self.filled.contains(point))
            .map(|(point, by)| (*point, by.clone()))
            .collect()
    }

    fn fill(&mut self, point: ExtensionPointKind) -> &mut Self {
        self.filled.insert(point);
        self
    }

    pub(crate) fn into_parts(self) -> (RoundPoints, Arc<dyn VoidTilePolicy>) {
        (
            self.round,
            self.void_tile.unwrap_or_else(|| Arc::new(NoVoidEffect)),
        )
    }
}
