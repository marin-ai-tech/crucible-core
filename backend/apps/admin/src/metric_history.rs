use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crucible_loop::LoopMetrics;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::Serialize;

const WINDOW: Duration = Duration::from_secs(5);
const MAX_OBSERVATIONS: usize = 16_384;
const RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1_000;
const MAX_ROWS: i64 = RETENTION_MS / WINDOW.as_millis() as i64;
const PAGE_SIZE: usize = 180;

pub struct Sampler {
    started: Instant,
    counters: LoopMetrics,
    durations: Vec<Duration>,
    overflowed: bool,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Sample {
    pub id: i64,
    pub ended_at_ms: i64,
    pub window_ms: i64,
    pub ticks: i64,
    pub skipped_ticks: i64,
    pub lagging_iterations: i64,
    pub mean_ns: Option<i64>,
    pub p95_ns: Option<i64>,
    pub p99_ns: Option<i64>,
    pub max_ns: Option<i64>,
}

impl Sampler {
    pub fn new(now: Instant, counters: LoopMetrics) -> Self {
        Self {
            started: now,
            counters,
            durations: Vec::with_capacity(MAX_OBSERVATIONS),
            overflowed: false,
        }
    }

    pub fn observe(&mut self, work: Duration) {
        if self.durations.len() < MAX_OBSERVATIONS {
            self.durations.push(work);
        } else {
            self.overflowed = true;
        }
    }

    pub fn flush(
        &mut self,
        now: Instant,
        ended_at_ms: i64,
        counters: LoopMetrics,
        force: bool,
    ) -> Option<Sample> {
        let elapsed = now.saturating_duration_since(self.started);
        if elapsed < WINDOW && (!force || counters == self.counters) {
            return None;
        }

        let ticks = counters.ticks.saturating_sub(self.counters.ticks);
        let mut sample = Sample {
            id: 0,
            ended_at_ms,
            window_ms: sqlite_integer(elapsed.as_millis()),
            ticks: sqlite_integer(ticks.into()),
            skipped_ticks: sqlite_integer(
                counters
                    .skipped_ticks
                    .saturating_sub(self.counters.skipped_ticks)
                    .into(),
            ),
            lagging_iterations: sqlite_integer(
                counters
                    .lagging_iterations
                    .saturating_sub(self.counters.lagging_iterations)
                    .into(),
            ),
            mean_ns: None,
            p95_ns: None,
            p99_ns: None,
            max_ns: None,
        };
        let count = self.durations.len();
        if ticks > 0 && !self.overflowed && count as u64 == ticks {
            self.durations.sort_unstable();
            let rank = |percent: usize| (count * percent).div_ceil(100) - 1;
            let total: u128 = self.durations.iter().map(Duration::as_nanos).sum();
            sample.mean_ns = Some(sqlite_integer(total / count as u128));
            sample.p95_ns = Some(sqlite_integer(self.durations[rank(95)].as_nanos()));
            sample.p99_ns = Some(sqlite_integer(self.durations[rank(99)].as_nanos()));
            sample.max_ns = Some(sqlite_integer(self.durations[count - 1].as_nanos()));
        }

        self.started = now;
        self.counters = counters;
        self.durations.clear();
        self.overflowed = false;
        Some(sample)
    }
}

fn sqlite_integer(value: u128) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

pub fn unix_time_ms() -> i64 {
    sqlite_integer(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
}

const SELECT_PAGE: &str = "SELECT id, ended_at_ms, window_ms, ticks, skipped_ticks,
    lagging_iterations, mean_ns, p95_ns, p99_ns, max_ns FROM tick_metric_samples
    WHERE id <= ?1 ORDER BY id DESC LIMIT ?2";

#[derive(Serialize)]
pub struct Page {
    pub samples: Vec<Sample>,
    pub next_before: Option<i64>,
}

pub struct History {
    db: Connection,
}

impl History {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let db = Connection::open(path)?;
        db.busy_timeout(Duration::from_secs(3))?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS tick_metric_samples (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ended_at_ms INTEGER NOT NULL,
                window_ms INTEGER NOT NULL,
                ticks INTEGER NOT NULL,
                skipped_ticks INTEGER NOT NULL,
                lagging_iterations INTEGER NOT NULL,
                mean_ns INTEGER,
                p95_ns INTEGER,
                p99_ns INTEGER,
                max_ns INTEGER
            );
            CREATE INDEX IF NOT EXISTS tick_metric_samples_ended
                ON tick_metric_samples (ended_at_ms);
            CREATE TABLE IF NOT EXISTS engine_runtime_state (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                json TEXT NOT NULL
            );",
        )?;
        db.execute(
            "DELETE FROM tick_metric_samples WHERE ended_at_ms < ?1",
            [unix_time_ms().saturating_sub(RETENTION_MS)],
        )?;
        Ok(Self { db })
    }

    pub fn open_readonly(path: &Path) -> rusqlite::Result<Self> {
        let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        db.busy_timeout(Duration::from_secs(3))?;
        db.prepare(SELECT_PAGE)?;
        let history = Self { db };
        history.runtime_state()?;
        Ok(history)
    }

    pub fn record(&mut self, sample: Sample) -> rusqlite::Result<()> {
        self.record_window(sample, None)
    }

    pub(crate) fn record_window(
        &mut self,
        sample: Sample,
        runtime_state: Option<&str>,
    ) -> rusqlite::Result<()> {
        let tx = self.db.transaction()?;
        tx.execute(
            "INSERT INTO tick_metric_samples
                (ended_at_ms, window_ms, ticks, skipped_ticks, lagging_iterations,
                 mean_ns, p95_ns, p99_ns, max_ns)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                sample.ended_at_ms,
                sample.window_ms,
                sample.ticks,
                sample.skipped_ticks,
                sample.lagging_iterations,
                sample.mean_ns,
                sample.p95_ns,
                sample.p99_ns,
                sample.max_ns,
            ],
        )?;
        let cutoff_id = tx.last_insert_rowid().saturating_sub(MAX_ROWS);
        tx.execute(
            "DELETE FROM tick_metric_samples WHERE ended_at_ms < ?1 OR id <= ?2",
            params![sample.ended_at_ms.saturating_sub(RETENTION_MS), cutoff_id],
        )?;
        if let Some(json) = runtime_state {
            upsert_runtime_state(&tx, json)?;
        }
        tx.commit()
    }

    pub fn page(&self, before: Option<i64>) -> rusqlite::Result<Page> {
        let through = before.map_or(i64::MAX, |id| id.saturating_sub(1));
        let mut statement = self.db.prepare_cached(SELECT_PAGE)?;
        let mut samples = statement
            .query_map(params![through, PAGE_SIZE as i64 + 1], |row| {
                Ok(Sample {
                    id: row.get(0)?,
                    ended_at_ms: row.get(1)?,
                    window_ms: row.get(2)?,
                    ticks: row.get(3)?,
                    skipped_ticks: row.get(4)?,
                    lagging_iterations: row.get(5)?,
                    mean_ns: row.get(6)?,
                    p95_ns: row.get(7)?,
                    p99_ns: row.get(8)?,
                    max_ns: row.get(9)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let next_before = if samples.len() > PAGE_SIZE {
            samples.truncate(PAGE_SIZE);
            Some(samples[PAGE_SIZE - 1].id)
        } else {
            None
        };
        samples.reverse();
        Ok(Page {
            samples,
            next_before,
        })
    }

    pub fn runtime_state(&self) -> rusqlite::Result<Option<String>> {
        self.db
            .query_row(
                "SELECT json FROM engine_runtime_state WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .optional()
    }

    pub fn replace_runtime_state(&mut self, json: &str) -> rusqlite::Result<()> {
        upsert_runtime_state(&self.db, json)
    }
}

fn upsert_runtime_state(db: &Connection, json: &str) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO engine_runtime_state (id, json) VALUES (1, ?1)
         ON CONFLICT(id) DO UPDATE SET json = excluded.json",
        [json],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_calculates_nearest_rank_percentiles_and_counter_deltas() {
        let start = Instant::now();
        let prior = LoopMetrics {
            ticks: 10,
            skipped_ticks: 1,
            lagging_iterations: 1,
        };
        let mut sampler = Sampler::new(start, prior);
        for nanos in (1..=100).rev() {
            sampler.observe(Duration::from_nanos(nanos));
        }
        let counters = LoopMetrics {
            ticks: 110,
            skipped_ticks: 3,
            lagging_iterations: 2,
        };
        assert!(
            sampler
                .flush(start + Duration::from_secs(4), 4_000, counters, false)
                .is_none()
        );
        let row = sampler
            .flush(start + WINDOW, 5_000, counters, false)
            .unwrap();
        assert_eq!(
            row,
            Sample {
                id: 0,
                ended_at_ms: 5_000,
                window_ms: 5_000,
                ticks: 100,
                skipped_ticks: 2,
                lagging_iterations: 1,
                mean_ns: Some(50),
                p95_ns: Some(95),
                p99_ns: Some(99),
                max_ns: Some(100),
            }
        );
        let empty = sampler
            .flush(start + WINDOW * 2, 10_000, counters, false)
            .unwrap();
        assert_eq!(empty.ticks, 0);
        assert_eq!(empty.skipped_ticks, 0);
        assert_eq!(empty.p95_ns, None);
        assert_eq!(empty.p99_ns, None);
        assert_eq!(empty.mean_ns, None);
        assert_eq!(empty.max_ns, None);
    }

    #[test]
    fn incomplete_windows_preserve_counts_and_omit_timings() {
        let start = Instant::now();
        let mut sampler = Sampler::new(start, LoopMetrics::default());
        for _ in 0..=MAX_OBSERVATIONS {
            sampler.observe(Duration::from_nanos(11));
        }
        let counters = LoopMetrics {
            ticks: MAX_OBSERVATIONS as u64 + 1,
            ..LoopMetrics::default()
        };
        let partial = sampler
            .flush(start + Duration::from_secs(1), 1_000, counters, true)
            .unwrap();
        assert_eq!(partial.window_ms, 1_000);
        assert_eq!(partial.ticks, MAX_OBSERVATIONS as i64 + 1);
        assert_eq!(partial.p95_ns, None);
        assert_eq!(partial.p99_ns, None);
        assert_eq!(partial.mean_ns, None);
        assert_eq!(partial.max_ns, None);
    }

    fn record_at(history: &mut History, ended_at_ms: i64) {
        history
            .record(Sample {
                id: 0,
                ended_at_ms,
                window_ms: 5_000,
                ticks: 1,
                skipped_ticks: 0,
                lagging_iterations: 0,
                mean_ns: Some(14),
                p95_ns: Some(14),
                p99_ns: Some(14),
                max_ns: Some(14),
            })
            .unwrap();
    }

    #[test]
    fn history_survives_reopen_and_pages_without_overlap() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("metrics.sqlite");
        let now = unix_time_ms();
        let mut writer = History::open(&path).unwrap();
        let reader = History::open_readonly(&path).unwrap();
        record_at(&mut writer, now);
        assert_eq!(reader.page(None).unwrap().samples.len(), 1);
        drop(reader);
        for i in 1..PAGE_SIZE + 2 {
            record_at(&mut writer, now + i as i64);
        }
        drop(writer);

        let reader = History::open_readonly(&path).unwrap();
        let latest = reader.page(None).unwrap();
        assert_eq!(latest.samples.len(), PAGE_SIZE);
        assert_eq!(latest.samples[0].id, 3);
        assert_eq!(latest.samples[0].p99_ns, Some(14));
        assert_eq!(latest.samples[PAGE_SIZE - 1].ended_at_ms, now + 181);
        let older = reader.page(latest.next_before).unwrap();
        assert_eq!(
            older
                .samples
                .iter()
                .map(|sample| sample.id)
                .collect::<Vec<_>>(),
            [1, 2],
        );
        assert_eq!(older.next_before, None);
    }

    #[test]
    fn history_applies_age_and_row_retention() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("metrics.sqlite");
        let mut history = History::open(&path).unwrap();
        let now = unix_time_ms();
        record_at(&mut history, now - RETENTION_MS);
        record_at(&mut history, now);
        assert_eq!(history.page(None).unwrap().samples.len(), 2);
        record_at(&mut history, now + 1);
        assert_eq!(history.page(None).unwrap().samples[0].id, 2);

        history
            .db
            .execute(
                "UPDATE sqlite_sequence SET seq = ?1 WHERE name = 'tick_metric_samples'",
                [MAX_ROWS + 2],
            )
            .unwrap();
        record_at(&mut history, now + 2);
        let recent = history.page(None).unwrap();
        assert_eq!(recent.samples.len(), 1);
        assert_eq!(recent.samples[0].id, MAX_ROWS + 3);
    }

    #[test]
    fn runtime_state_updates_in_place_and_survives_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("metrics.sqlite");
        let mut writer = History::open(&path).unwrap();
        let reader = History::open_readonly(&path).unwrap();
        assert_eq!(reader.runtime_state().unwrap(), None);
        let first = r#"{"label":"A 'quoted' value with \"escapes\" and <text>"}"#;
        writer.replace_runtime_state(first).unwrap();
        assert_eq!(reader.runtime_state().unwrap().as_deref(), Some(first));

        let second = r#"{"label":"updated"}"#;
        writer.replace_runtime_state(second).unwrap();
        drop(reader);
        drop(writer);
        let reader = History::open_readonly(&path).unwrap();
        assert_eq!(reader.runtime_state().unwrap().as_deref(), Some(second));
    }
}
