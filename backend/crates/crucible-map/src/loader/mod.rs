//! Loader for the SS14 map format.
//!
//! The YAML contains `meta` (`format` 6 or 7), `tilemap` (file tile numbers mapped
//! to tile prototypes), `nullspace`, and `entities`, grouped as `{ proto, entities:
//! [{ uid, components, missingComponents }] }`. Each tile chunk is base64-encoded
//! in row-major order. Each tile holds a little-endian `i32` number, flags, a variant,
//! and, in chunk format 7, a rotation and reflection value.
//!
//! The loader parses into a compact [`tree`] and then:
//! 1. Validates the sections, `tilemap`, and entity prototypes without mutating the world.
//! 2. Creates uninitialized entities and merges prototype components with file
//!    overrides, removing `missingComponents`.
//! 3. Builds maps, grids with tiles, and parent-first transforms, then anchors entities.
//! 4. Passes other components to the [`ComponentSpawner`] and skips components
//!    marked as ignored in the prototype registry.
//! 5. Initializes maps and grids, then other entities in file order, and raises
//!    [`OverVoid`](crate::OverVoid).
//!
//! Any error rolls back the operation and removes the created entities. Diagnostics
//! report their file, line, entity `uid`, and prototype when available.

mod tree;

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::Path;
use std::sync::Arc;

use base64::Engine as _;
use crucible_ecs::{EntityUid, World};
use crucible_proto::{Node, Pos, Prototype, PrototypeSet, Registry, Value, parse_bool};

use crate::grid::{self, MapGrid, MapId};
use crate::math::{Vec2, Vector2i};
use crate::tile::{Tile, TileDefinitions};
use crate::transform;
use tree::{CNode, MapTree};

/// Kind of entity prototypes.
const ENTITY_KIND: &str = "entity";

/// Creates components beyond `Transform`, `Map`, and `MapGrid`, bridging the map
/// loader and data-driven component creation.
///
/// An assembled server uses `ComponentFactories` from `crucible-ext`, as in
/// [`load_map`](crate::load_map). Loader tests and map-corpus benchmarks without
/// prototypes may substitute a stub.
pub trait ComponentSpawner {
    /// Adds component `name` with merged `data` fields, excluding the `type` key.
    /// `Ok(false)` signals an unknown component. `Err` signals invalid component data.
    /// Use `entities` to resolve file `uid` references to world entities.
    fn add_component(
        &mut self,
        world: &mut World,
        uid: EntityUid,
        name: &str,
        data: &Node,
        entities: &FileEntities,
    ) -> Result<bool, String>;
}

/// Skips unknown metadata without an error. The core has no `MetaData` component
/// and stores the entity's prototype name in `EntityMeta`.
const META_DATA: &str = "MetaData";

/// How to handle entities whose prototypes are missing from the set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MissingPrototypes {
    /// Return a load error. This is the default.
    #[default]
    Error,
    /// Create the entity from file components alone, for corpus benchmarks without prototypes.
    Ignore,
}

/// Mapping from file `uid` values to world entities.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileEntities {
    map: HashMap<u32, EntityUid>,
}

impl FileEntities {
    /// Finds the entity with the given file `uid`.
    pub fn get(&self, file_uid: u32) -> Option<EntityUid> {
        self.map.get(&file_uid).copied()
    }

    /// Resolves a component-data reference such as `"123"`. `"invalid"` resolves to `None`.
    pub fn resolve(&self, text: &str) -> Option<EntityUid> {
        self.get(text.trim().parse().ok()?)
    }

    /// Number of entities.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether the mapping is empty.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Iterates through file number and entity pairs in unspecified order.
    pub fn iter(&self) -> impl Iterator<Item = (u32, EntityUid)> + '_ {
        self.map.iter().map(|(k, v)| (*k, *v))
    }
}

/// Result of a successful map load.
#[derive(Clone, Debug)]
pub struct LoadedMap {
    /// The primary map, chosen from the file or created for a grid-only file.
    pub map_id: MapId,
    /// Entity for the primary map.
    pub map_uid: EntityUid,
    /// All maps from the file, including one created by the loader if needed.
    pub maps: Vec<(MapId, EntityUid)>,
    /// Grids in file order.
    pub grids: Vec<EntityUid>,
    /// All entities loaded from the file.
    pub entities: FileEntities,
}

/// A map loading diagnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapDiagnostic {
    /// One-based line number when known.
    pub line: Option<u32>,
    /// Entity `uid` in the map file.
    pub entity: Option<u32>,
    /// Entity prototype.
    pub prototype: Option<String>,
    pub message: String,
}

/// Errors encountered while loading one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapLoadError {
    pub file: String,
    pub diagnostics: Vec<MapDiagnostic>,
}

impl fmt::Display for MapLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, d) in self.diagnostics.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{}", self.file)?;
            if let Some(line) = d.line {
                write!(f, ":{line}")?;
            }
            write!(f, ": ")?;
            if let Some(uid) = d.entity {
                write!(f, "entity uid {uid}")?;
                if let Some(p) = &d.prototype {
                    write!(f, " (prototype `{p}`)")?;
                }
                write!(f, ": ")?;
            }
            write!(f, "{}", d.message)?;
        }
        Ok(())
    }
}

impl std::error::Error for MapLoadError {}

/// Map loader configured with an entity prototype set, prototype registry for ignored
/// components, and tile table.
pub struct MapLoader<'a> {
    tiles: &'a TileDefinitions,
    prototypes: Option<&'a PrototypeSet>,
    registry: Option<&'a Registry>,
    missing_prototypes: MissingPrototypes,
}

/// Entity read from the file before spawning.
struct Pending<'t> {
    file_uid: u32,
    line: u32,
    proto_name: Option<&'t str>,
    proto: Option<Arc<Prototype>>,
    components: Option<&'t CNode>,
    missing: Vec<&'t str>,
}

/// Entity component after merging.
struct Merged {
    name: Box<str>,
    line: u32,
    data: Node,
}

impl<'a> MapLoader<'a> {
    pub fn new(tiles: &'a TileDefinitions) -> Self {
        MapLoader {
            tiles,
            prototypes: None,
            registry: None,
            missing_prototypes: MissingPrototypes::Error,
        }
    }

    /// Sets the entity prototype set. Without it, [`MissingPrototypes`] controls how
    /// to handle entities that have a `proto` value.
    pub fn with_prototypes(mut self, prototypes: &'a PrototypeSet) -> Self {
        self.prototypes = Some(prototypes);
        self
    }

    /// Sets the prototype registry used to identify ignored components.
    pub fn with_registry(mut self, registry: &'a Registry) -> Self {
        self.registry = Some(registry);
        self
    }

    /// Sets how to handle missing prototypes.
    pub fn missing_prototypes(mut self, policy: MissingPrototypes) -> Self {
        self.missing_prototypes = policy;
        self
    }

    /// Loads a map file.
    pub fn load_file(
        &self,
        world: &mut World,
        components: &mut dyn ComponentSpawner,
        path: &Path,
    ) -> Result<LoadedMap, MapLoadError> {
        let file = path.to_string_lossy().replace('\\', "/");
        let text = std::fs::read_to_string(path).map_err(|e| MapLoadError {
            file: file.clone(),
            diagnostics: vec![MapDiagnostic {
                line: None,
                entity: None,
                prototype: None,
                message: format!("could not read file: {e}"),
            }],
        })?;
        self.load_str(world, components, &file, &text)
    }

    /// Loads a map from text and uses `file` as its name in diagnostics.
    pub fn load_str(
        &self,
        world: &mut World,
        components: &mut dyn ComponentSpawner,
        file: &str,
        text: &str,
    ) -> Result<LoadedMap, MapLoadError> {
        let fail = |diagnostics: Vec<MapDiagnostic>| MapLoadError {
            file: file.to_owned(),
            diagnostics,
        };
        let tree = tree::parse(text).map_err(|errors| {
            fail(
                errors
                    .into_iter()
                    .map(|e| MapDiagnostic {
                        line: Some(e.line),
                        entity: None,
                        prototype: None,
                        message: e.message,
                    })
                    .collect(),
            )
        })?;
        let mut run = Run {
            loader: self,
            tree: &tree,
            diags: Vec::new(),
            spawned: Vec::new(),
            created_maps: Vec::new(),
        };
        match run.load(world, components) {
            Some(loaded) if run.diags.is_empty() => Ok(loaded),
            _ => {
                let diags = std::mem::take(&mut run.diags);
                run.rollback(world);
                Err(fail(diags))
            }
        }
    }
}

struct Run<'l, 'a, 't> {
    loader: &'l MapLoader<'a>,
    tree: &'t MapTree,
    diags: Vec<MapDiagnostic>,
    spawned: Vec<EntityUid>,
    created_maps: Vec<MapId>,
}

impl<'t> Run<'_, '_, 't> {
    fn error(
        &mut self,
        line: Option<u32>,
        entity: Option<&Pending<'_>>,
        message: impl Into<String>,
    ) {
        self.diags.push(MapDiagnostic {
            line,
            entity: entity.map(|e| e.file_uid),
            prototype: entity.and_then(|e| e.proto_name.map(str::to_owned)),
            message: message.into(),
        });
    }

    fn rollback(&mut self, world: &mut World) {
        for uid in self.spawned.drain(..).rev() {
            world.delete(uid);
        }
        for map_id in self.created_maps.drain(..) {
            if let Some(uid) = grid::map_entity(world, map_id) {
                world.delete(uid);
            }
            crate::state::get_mut(world).unregister_map(map_id);
        }
    }

    fn load(
        &mut self,
        world: &mut World,
        components: &mut dyn ComponentSpawner,
    ) -> Option<LoadedMap> {
        let tree = self.tree;
        let Some(root) = tree.docs.first().filter(|d| d.as_map().is_some()) else {
            self.error(None, None, "a map file must be a mapping of sections");
            return None;
        };

        // meta.format
        match root.get(tree, "meta").and_then(|m| m.get(tree, "format")) {
            Some(f) => match f.as_str(tree) {
                Some("6" | "7") => {}
                other => self.error(
                    Some(f.line()),
                    None,
                    format!(
                        "unsupported map format version `{}` (supported versions are 6 and 7)",
                        other.unwrap_or("?")
                    ),
                ),
            },
            None => self.error(Some(root.line()), None, "missing `meta.format`"),
        }

        // Map file tile numbers to core tile IDs.
        let mut tilemap: HashMap<i32, i32> = HashMap::new();
        if let Some(tm) = root.get(tree, "tilemap") {
            for (key, value) in tm.as_map().unwrap_or(&[]) {
                let file_id = tree.atoms.text(*key).trim().parse::<i32>();
                let name = value.as_str(tree).unwrap_or("");
                match (file_id, self.loader.tiles.id(name)) {
                    (Ok(fid), Some(id)) => {
                        tilemap.insert(fid, id);
                    }
                    (Err(_), _) => self.error(
                        Some(value.line()),
                        None,
                        format!(
                            "`tilemap`: tile number `{}` is not an integer",
                            tree.atoms.text(*key)
                        ),
                    ),
                    (_, None) => self.error(
                        Some(value.line()),
                        None,
                        format!("`tilemap`: unknown tile prototype `{name}`"),
                    ),
                }
            }
        }

        let nullspace: HashSet<u32> = root
            .get(tree, "nullspace")
            .and_then(CNode::as_seq)
            .unwrap_or(&[])
            .iter()
            .filter_map(|n| n.as_str(tree)?.parse().ok())
            .collect();

        // Read the file's entities.
        let mut pending: Vec<Pending<'t>> = Vec::new();
        let mut seen: HashSet<u32> = HashSet::new();
        let groups = root
            .get(tree, "entities")
            .and_then(CNode::as_seq)
            .unwrap_or(&[]);
        for group in groups {
            let proto_name = group
                .get(tree, "proto")
                .and_then(|p| p.as_str(tree))
                .filter(|p| !p.is_empty());
            let proto = match proto_name {
                None => None,
                Some(id) => match self.loader.prototypes.and_then(|s| s.get(ENTITY_KIND, id)) {
                    Some(p) if p.is_abstract => {
                        self.error(
                            Some(group.line()),
                            None,
                            format!("prototype `{id}` is abstract"),
                        );
                        None
                    }
                    Some(p) => Some(p.clone()),
                    None if self.loader.missing_prototypes == MissingPrototypes::Ignore => None,
                    None => {
                        self.error(
                            Some(group.line()),
                            None,
                            format!("entity prototype `{id}` not found"),
                        );
                        None
                    }
                },
            };
            for ent in group
                .get(tree, "entities")
                .and_then(CNode::as_seq)
                .unwrap_or(&[])
            {
                let file_uid = ent
                    .get(tree, "uid")
                    .and_then(|u| u.as_str(tree))
                    .and_then(|u| u.parse::<u32>().ok());
                let Some(file_uid) = file_uid else {
                    self.error(Some(ent.line()), None, "entity is missing a numeric `uid`");
                    continue;
                };
                let p = Pending {
                    file_uid,
                    line: ent.line(),
                    proto_name,
                    proto: proto.clone(),
                    components: ent.get(tree, "components"),
                    missing: ent
                        .get(tree, "missingComponents")
                        .and_then(CNode::as_seq)
                        .unwrap_or(&[])
                        .iter()
                        .filter_map(|n| n.as_str(tree))
                        .collect(),
                };
                if !seen.insert(file_uid) {
                    self.error(Some(p.line), Some(&p), "duplicate file `uid`");
                    continue;
                }
                pending.push(p);
            }
        }
        if !self.diags.is_empty() {
            return None;
        }

        // Create entities and merge components.
        let mut entities = FileEntities::default();
        for p in &pending {
            let uid = world.spawn_uninitialized();
            self.spawned.push(uid);
            world.set_entity_prototype(uid, p.proto_name.map(Arc::from));
            entities.map.insert(p.file_uid, uid);
        }
        let merged: Vec<Vec<Merged>> = pending.iter().map(|p| self.merge(p)).collect();
        if !self.diags.is_empty() {
            return None;
        }
        let comp = |i: usize, name: &str| merged[i].iter().find(|m| &*m.name == name);

        // Create maps.
        let mut maps = Vec::new();
        for (i, p) in pending.iter().enumerate() {
            if let Some(m) = comp(i, "Map") {
                let uid = entities.map[&p.file_uid];
                let paused = m
                    .data
                    .get("mapPaused")
                    .and_then(Node::as_str)
                    .and_then(parse_bool)
                    .unwrap_or(false);
                let map_id = grid::add_map(world, uid, paused);
                self.created_maps.push(map_id);
                maps.push((map_id, uid));
            }
        }
        let target = match maps.first() {
            Some(&m) => m,
            None => {
                let (uid, map_id) = grid::create_map(world);
                self.created_maps.push(map_id);
                maps.push((map_id, uid));
                (map_id, uid)
            }
        };

        // Create grids and tiles.
        let mut grids = Vec::new();
        for (i, p) in pending.iter().enumerate() {
            if let Some(m) = comp(i, "MapGrid") {
                let uid = entities.map[&p.file_uid];
                if let Some(g) = self.decode_grid(p, m, &tilemap, world.current_tick()) {
                    world.add_component(uid, g).expect("new entity has no grid");
                    grids.push(uid);
                }
            }
        }

        // Create parent transforms before their children.
        let index: HashMap<u32, usize> = pending
            .iter()
            .enumerate()
            .map(|(i, p)| (p.file_uid, i))
            .collect();
        let mut state = vec![0u8; pending.len()]; // 0: unseen, 1: in progress, 2: done
        let mut over_void = Vec::new();
        let mut anchored = Vec::new();
        for start in 0..pending.len() {
            let mut stack = vec![start];
            while let Some(&i) = stack.last() {
                if state[i] == 2 {
                    stack.pop();
                    continue;
                }
                let p = &pending[i];
                let uid = entities.map[&p.file_uid];
                let data = comp(i, "Transform").map(|m| &m.data);
                let parent_ref = data
                    .and_then(|d| d.get("parent"))
                    .and_then(Node::as_str)
                    .filter(|s| *s != "invalid");
                let parent_idx = match parent_ref {
                    None => None,
                    Some(r) => match r.parse::<u32>().ok().and_then(|u| index.get(&u)) {
                        Some(&pi) => Some(pi),
                        None => {
                            self.error(
                                data.map(|d| d.pos.line),
                                Some(p),
                                format!(
                                    "`Transform.parent`: entity `{r}` is missing from the file"
                                ),
                            );
                            state[i] = 2;
                            stack.pop();
                            continue;
                        }
                    },
                };
                if let Some(pi) = parent_idx
                    && state[pi] != 2
                {
                    if state[pi] == 1 || pi == i {
                        self.error(
                            data.map(|d| d.pos.line),
                            Some(p),
                            "parent cycle in `Transform.parent`",
                        );
                        state[i] = 2;
                        stack.pop();
                        continue;
                    }
                    state[i] = 1;
                    stack.push(pi);
                    continue;
                }
                state[i] = 2;
                stack.pop();

                let is_map = world.has::<grid::Map>(uid);
                let parent = match parent_idx {
                    Some(pi) => Some(entities.map[&pending[pi].file_uid]),
                    None if is_map || nullspace.contains(&p.file_uid) => None,
                    // Set the primary map as parent for roots in grid-only files.
                    None => Some(target.1),
                };
                let (pos, rot, anchor) = match data {
                    Some(d) => match transform_fields(d) {
                        Ok(v) => v,
                        Err(e) => {
                            self.error(Some(d.pos.line), Some(p), e);
                            continue;
                        }
                    },
                    None => (Vec2::ZERO, 0.0, false),
                };
                if let Err(e) =
                    transform::place_deferred(world, uid, parent, pos, rot, &mut over_void)
                {
                    self.error(Some(p.line), Some(p), e.to_string());
                    continue;
                }
                if anchor {
                    anchored.push(uid);
                }
            }
        }
        for uid in anchored {
            // As in Robust, entities outside grids are left unanchored.
            let _ = transform::anchor(world, uid);
        }

        // Spawn the remaining components.
        let ignored = |name: &str| {
            self.loader
                .registry
                .is_some_and(|r| r.ignore_category(name).is_some())
        };
        for (i, p) in pending.iter().enumerate() {
            let uid = entities.map[&p.file_uid];
            for m in &merged[i] {
                if matches!(&*m.name, "Transform" | "Map" | "MapGrid") || ignored(&m.name) {
                    continue;
                }
                match components.add_component(world, uid, &m.name, &m.data, &entities) {
                    Ok(true) => {}
                    Ok(false) if &*m.name == META_DATA => {}
                    Ok(false) => self.error(
                        Some(m.line),
                        Some(p),
                        format!("unknown component `{}`", m.name),
                    ),
                    Err(e) => self.error(
                        Some(m.line),
                        Some(p),
                        format!("component `{}`: {e}", m.name),
                    ),
                }
            }
        }
        if !self.diags.is_empty() {
            return None;
        }

        // Initialize maps, grids, and then remaining entities in file order.
        for &(_, uid) in &maps {
            world.initialize_entity(uid);
        }
        for &uid in &grids {
            world.initialize_entity(uid);
        }
        for p in &pending {
            world.initialize_entity(entities.map[&p.file_uid]);
        }
        self.spawned.clear();
        self.created_maps.clear();
        transform::raise_over_void(world, over_void);

        Some(LoadedMap {
            map_id: target.0,
            map_uid: target.1,
            maps,
            grids,
            entities,
        })
    }

    /// Prototype components except `missingComponents`, then file fields overriding
    /// their values and any extra components from the file.
    fn merge(&mut self, p: &Pending<'t>) -> Vec<Merged> {
        let tree = self.tree;
        let mut out: Vec<Merged> = Vec::new();
        if let Some(proto) = &p.proto {
            for c in &proto.components {
                if p.missing.contains(&&*c.name) {
                    continue;
                }
                out.push(Merged {
                    name: c.name.clone(),
                    line: p.line,
                    data: c.data.clone(),
                });
            }
        }
        let Some(list) = p.components else {
            return out;
        };
        let Some(items) = list.as_seq() else {
            self.error(Some(list.line()), Some(p), "`components` must be a list");
            return out;
        };
        for item in items {
            let Some(name) = item.get(tree, "type").and_then(|t| t.as_str(tree)) else {
                self.error(Some(item.line()), Some(p), "component is missing `type`");
                continue;
            };
            let mut fields = Vec::new();
            for (k, v) in item.as_map().unwrap_or(&[]) {
                let key = tree.atoms.text(*k);
                if key != "type" {
                    fields.push((
                        Node::scalar(
                            Pos {
                                line: v.line(),
                                col: 0,
                            },
                            key,
                        ),
                        v.to_proto(tree),
                    ));
                }
            }
            match out.iter_mut().find(|m| &*m.name == name) {
                Some(m) => {
                    m.line = item.line();
                    overlay(&mut m.data, fields);
                }
                None => out.push(Merged {
                    name: name.into(),
                    line: item.line(),
                    data: Node::new(
                        Pos {
                            line: item.line(),
                            col: 0,
                        },
                        Value::Map(fields),
                    ),
                }),
            }
        }
        out
    }

    fn decode_grid(
        &mut self,
        p: &Pending<'_>,
        m: &Merged,
        tilemap: &HashMap<i32, i32>,
        tick: crucible_ecs::Tick,
    ) -> Option<MapGrid> {
        let line = Some(m.line);
        let chunk_size = match m.data.get("chunkSize").and_then(Node::as_str) {
            None => crate::grid::DEFAULT_CHUNK_SIZE,
            Some(s) => match s.parse::<u8>() {
                Ok(v) if v > 0 => v,
                _ => {
                    self.error(
                        line,
                        Some(p),
                        format!("`MapGrid.chunkSize`: `{s}` is not an integer from 1 to 255"),
                    );
                    return None;
                }
            },
        };
        let mut grid = MapGrid::new(chunk_size);
        let cs = usize::from(chunk_size);
        let empty = Vec::new();
        let chunks = m
            .data
            .get("chunks")
            .and_then(Node::as_map)
            .unwrap_or(&empty);
        for (_, chunk) in chunks {
            let cl = Some(chunk.pos.line);
            let ind = chunk
                .get("ind")
                .and_then(Node::as_str)
                .and_then(parse_pair_i32);
            let tiles = chunk.get("tiles").and_then(Node::as_str);
            let version = chunk.get("version").and_then(Node::as_str).unwrap_or("6");
            let (Some(ind), Some(tiles)) = (ind, tiles) else {
                self.error(cl, Some(p), "chunk is missing `ind` or `tiles`");
                continue;
            };
            let per_tile = match version {
                "6" => 6,
                "7" => 7,
                v => {
                    self.error(
                        cl,
                        Some(p),
                        format!("unsupported chunk format version `{v}` (supported versions are 6 and 7)"),
                    );
                    continue;
                }
            };
            let bytes = match base64::engine::general_purpose::STANDARD.decode(tiles.trim()) {
                Ok(b) => b,
                Err(e) => {
                    self.error(cl, Some(p), format!("`tiles`: invalid base64: {e}"));
                    continue;
                }
            };
            if bytes.len() != cs * cs * per_tile {
                self.error(
                    cl,
                    Some(p),
                    format!(
                        "`tiles`: got {} bytes, expected {} ({cs}×{cs} tiles at {per_tile} bytes each)",
                        bytes.len(),
                        cs * cs * per_tile
                    ),
                );
                continue;
            }
            let base = Vector2i::new(ind.x * i32::from(chunk_size), ind.y * i32::from(chunk_size));
            for (slot, raw) in bytes.chunks_exact(per_tile).enumerate() {
                let file_id = i32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
                let Some(&type_id) = tilemap.get(&file_id) else {
                    self.error(
                        cl,
                        Some(p),
                        format!("tile number {file_id} is not defined in `tilemap`"),
                    );
                    return None;
                };
                let tile = Tile {
                    type_id,
                    flags: raw[4],
                    variant: raw[5],
                    rotation_mirroring: if per_tile == 7 { raw[6] } else { 0 },
                };
                if !tile.is_empty() {
                    let at =
                        Vector2i::new(base.x + (slot % cs) as i32, base.y + (slot / cs) as i32);
                    grid.put(at, tile, tick);
                }
            }
        }
        Some(grid)
    }
}

/// Adds or replaces fields in `base` with the same keys from `fields`.
fn overlay(base: &mut Node, fields: Vec<(Node, Node)>) {
    let Value::Map(entries) = &mut base.value else {
        base.value = Value::Map(fields);
        return;
    };
    for (k, v) in fields {
        match entries.iter_mut().find(|(ek, _)| ek.as_str() == k.as_str()) {
            Some((_, ev)) => *ev = v,
            None => entries.push((k, v)),
        }
    }
}

fn parse_pair_i32(s: &str) -> Option<Vector2i> {
    let (x, y) = s.split_once(',')?;
    Some(Vector2i::new(
        x.trim().parse().ok()?,
        y.trim().parse().ok()?,
    ))
}

fn parse_pair_f32(s: &str) -> Option<Vec2> {
    let (x, y) = s.split_once(',')?;
    Some(Vec2::new(x.trim().parse().ok()?, y.trim().parse().ok()?))
}

/// Parses an angle such as `1.57 rad` or `90 deg`, or a bare value in radians.
fn parse_angle(s: &str) -> Option<f64> {
    let s = s.trim();
    if let Some(v) = s.strip_suffix("rad") {
        v.trim().parse().ok()
    } else if let Some(v) = s.strip_suffix("deg") {
        v.trim().parse::<f64>().ok().map(f64::to_radians)
    } else {
        s.parse().ok()
    }
}

/// Parses `pos`, `rot`, and `anchored` from `Transform` fields.
fn transform_fields(data: &Node) -> Result<(Vec2, f64, bool), String> {
    let pos = match data.get("pos").and_then(Node::as_str) {
        None => Vec2::ZERO,
        Some(s) => parse_pair_f32(s)
            .ok_or_else(|| format!("`Transform.pos`: `{s}` is not an `x,y` pair"))?,
    };
    let rot = match data.get("rot").and_then(Node::as_str) {
        None => 0.0,
        Some(s) => {
            parse_angle(s).ok_or_else(|| format!("`Transform.rot`: `{s}` is not an angle"))?
        }
    };
    let anchored = match data.get("anchored").and_then(Node::as_str) {
        None => false,
        Some(s) => {
            parse_bool(s).ok_or_else(|| format!("`Transform.anchored`: `{s}` is not a boolean"))?
        }
    };
    Ok((pos, rot, anchored))
}
