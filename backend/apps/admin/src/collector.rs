//! Collects core timings in memory and writes completed windows off the simulation thread.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crucible_loop::{LoopMetrics, TickPhase};
use serde::Serialize;

use crate::metric_history::{History, Sample, Sampler, unix_time_ms};

#[derive(Serialize)]
pub struct EngineMetadata {
    pub core_version: String,
    pub tick_rate: u32,
    pub extension: ExtensionInfo,
    pub modules: Vec<String>,
}

#[derive(Serialize)]
pub struct ExtensionInfo {
    pub id: String,
    pub version: String,
}

#[derive(Default)]
struct Work {
    calls: u64,
    total_ns: u64,
    max_ns: u64,
}

#[derive(Serialize)]
struct Mechanic {
    name: &'static str,
    phase: &'static str,
    calls: u64,
    total_ns: u64,
    max_ns: u64,
}

#[derive(Serialize)]
struct EngineState<'a> {
    #[serde(flatten)]
    metadata: &'a EngineMetadata,
    ended_at_ms: Option<i64>,
    mechanics: &'a [Mechanic],
}

struct Window {
    sample: Sample,
    mechanics: Vec<Mechanic>,
}

struct Measurements {
    sampler: Sampler,
    work: BTreeMap<(Option<TickPhase>, &'static str), Work>,
}

#[derive(Clone)]
pub struct Observations(Arc<Mutex<Measurements>>);

impl Observations {
    pub fn tick(&self, duration: Duration) {
        self.0
            .lock()
            .expect("telemetry samples")
            .sampler
            .observe(duration);
    }

    pub fn work(&self, phase: Option<TickPhase>, name: &'static str, duration: Duration) {
        let mut measurements = self.0.lock().expect("telemetry samples");
        let timing = measurements.work.entry((phase, name)).or_default();
        let nanos = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
        timing.calls = timing.calls.saturating_add(1);
        timing.total_ns = timing.total_ns.saturating_add(nanos);
        timing.max_ns = timing.max_ns.max(nanos);
    }

    fn take_window(&self, metrics: LoopMetrics, force: bool) -> Option<Window> {
        let mut measurements = self.0.lock().expect("telemetry samples");
        let sample = measurements
            .sampler
            .flush(Instant::now(), unix_time_ms(), metrics, force)?;
        let mechanics = std::mem::take(&mut measurements.work)
            .into_iter()
            .map(|((phase, name), timing)| Mechanic {
                name,
                phase: match phase {
                    None => "ECS",
                    Some(TickPhase::Input) => "Input",
                    Some(TickPhase::Physics) => "Physics",
                    Some(TickPhase::PostPhysics) => "PostPhysics",
                    Some(TickPhase::Replication) => "Replication",
                },
                calls: timing.calls,
                total_ns: timing.total_ns,
                max_ns: timing.max_ns,
            })
            .collect();
        Some(Window { sample, mechanics })
    }
}

pub struct Collector {
    observations: Observations,
    sender: SyncSender<Window>,
    writer: JoinHandle<()>,
}

impl Collector {
    /// Opens persistent history and starts a bounded asynchronous writer.
    pub fn start(
        path: &Path,
        metadata: EngineMetadata,
        metrics: LoopMetrics,
    ) -> Result<Self, String> {
        let mut history =
            History::open(path).map_err(|e| format!("metrics database {}: {e}", path.display()))?;
        let initial_state = encode_state(&metadata, None, &[])?;
        history
            .replace_runtime_state(&initial_state)
            .map_err(|e| format!("save metrics state: {e}"))?;
        let observations = Observations(Arc::new(Mutex::new(Measurements {
            sampler: Sampler::new(Instant::now(), metrics),
            work: BTreeMap::new(),
        })));
        let (sender, receiver) = sync_channel::<Window>(16);
        let writer = thread::Builder::new()
            .name("metrics-writer".into())
            .spawn(move || {
                for Window { sample, mechanics } in receiver {
                    let state = encode_state(&metadata, Some(sample.ended_at_ms), &mechanics);
                    let result = state.and_then(|json| {
                        history
                            .record_window(sample, Some(&json))
                            .map_err(|e| e.to_string())
                    });
                    if let Err(error) = result {
                        tracing::warn!(%error, "could not save the metrics window");
                    }
                }
            })
            .map_err(|e| format!("start metrics writer: {e}"))?;
        Ok(Self {
            observations,
            sender,
            writer,
        })
    }

    pub fn observations(&self) -> Observations {
        self.observations.clone()
    }

    /// Queues a completed window without blocking on database work.
    pub fn poll(&self, metrics: LoopMetrics) {
        if let Some(window) = self.observations.take_window(metrics, false)
            && let Err(error) = self.sender.try_send(window)
        {
            tracing::warn!(%error, "metrics window was not queued");
        }
    }

    /// Writes pending windows and the final partial window on orderly shutdown.
    pub fn finish(self, metrics: LoopMetrics) {
        if let Some(window) = self.observations.take_window(metrics, true)
            && self.sender.send(window).is_err()
        {
            tracing::warn!("metrics writer closed before the final window");
        }
        drop(self.sender);
        if self.writer.join().is_err() {
            tracing::warn!("metrics writer stopped unexpectedly");
        }
    }
}

fn encode_state(
    metadata: &EngineMetadata,
    ended_at_ms: Option<i64>,
    mechanics: &[Mechanic],
) -> Result<String, String> {
    serde_json::to_string(&EngineState {
        metadata,
        ended_at_ms,
        mechanics,
    })
    .map_err(|e| format!("encode metrics state: {e}"))
}
