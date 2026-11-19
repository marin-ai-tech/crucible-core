//! Benchmark helpers for a read-only mirror of the SS14 map corpus.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use crucible_ecs::{EntityUid, World};
use crucible_map::{
    ComponentSpawner, FileEntities, LoadedMap, MapLoader, MissingPrototypes, TileDefinitions,
};
use crucible_proto::Node;

/// Corpus directory from `CRUCIBLE_SS14_MAPS`.
pub fn corpus_dir() -> Option<PathBuf> {
    std::env::var_os("CRUCIBLE_SS14_MAPS").map(PathBuf::from)
}

/// All `*.yml` files in the directory tree, sorted by path.
pub fn map_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "yml") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Tile table from the file's `tilemap` section. File tile number 0 is the empty tile.
pub fn tiles_of(text: &str) -> TileDefinitions {
    let mut empty = None;
    let mut names = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if line == "tilemap:" {
            inside = true;
            continue;
        }
        if inside {
            let Some(rest) = line.strip_prefix("  ") else {
                break;
            };
            if let Some((id, name)) = rest.split_once(": ") {
                if id.trim() == "0" {
                    empty = Some(name.trim().to_owned());
                }
                names.push(name.trim().to_owned());
            }
        }
    }
    TileDefinitions::new(empty.as_deref(), names.iter())
}

/// Stub component spawner that accepts any component without constructing its data.
#[derive(Default)]
pub struct AcceptAll {
    pub count: usize,
}

impl ComponentSpawner for AcceptAll {
    fn add_component(
        &mut self,
        _world: &mut World,
        _uid: EntityUid,
        _name: &str,
        _data: &Node,
        _entities: &FileEntities,
    ) -> Result<bool, String> {
        self.count += 1;
        Ok(true)
    }
}

/// Loads a map into a new world.
pub fn load(tiles: &TileDefinitions, path: &Path, text: &str) -> (World, LoadedMap, usize) {
    let mut builder = World::builder();
    crucible_map::register(&mut builder).unwrap();
    let (mut world, _) = builder.build().unwrap();
    let mut components = AcceptAll::default();
    let loaded = MapLoader::new(tiles)
        .missing_prototypes(MissingPrototypes::Ignore)
        .load_str(&mut world, &mut components, &path.to_string_lossy(), text)
        .unwrap_or_else(|e| panic!("{e}"));
    (world, loaded, components.count)
}

/// Value of a field from `/proc/self/status`, in KiB (Linux).
pub fn status_kib(field: &str) -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with(field))?;
    line[field.len()..]
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse()
        .ok()
}

/// Resets the process's RSS high-water mark (`VmHWM`) to its current value (Linux).
pub fn reset_peak_rss() {
    let _ = std::fs::write("/proc/self/clear_refs", "5");
}
