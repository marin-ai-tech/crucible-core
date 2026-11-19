//! Creates entities from resolved prototypes.
//!
//! A type-erased component factory registry bridges `crucible-ecs` and `crucible-proto`.
//! The registry uses names as keys. During registration,
//! [`ComponentSpec::from_prototype`](crate::ComponentSpec::from_prototype) takes a closure
//! for component type `C` that reads resolved fields from the node tree and inserts
//! the value into the world with [`World::add_component`].

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use crucible_ecs::{Component, ComponentError, EntityUid, World};
use crucible_players::{NetUserId, PlayerManager, Players, SessionStatus};
use crucible_proto::{InstantiateError, Node, Pos, PrototypeSet, Registry, Value, parse_bool};

/// Entity prototype kind.
pub const ENTITY_KIND: &str = "entity";

/// Typed access to component fields from a resolved prototype.
#[derive(Clone, Copy)]
pub struct ComponentData<'a> {
    component: &'a str,
    node: &'a Node,
}

impl<'a> ComponentData<'a> {
    /// Creates a component data view for `component` from its field mapping.
    pub fn new(component: &'a str, node: &'a Node) -> Self {
        ComponentData { component, node }
    }

    /// Component name.
    pub fn component(&self) -> &'a str {
        self.component
    }

    /// The full node containing the fields.
    pub fn node(&self) -> &'a Node {
        self.node
    }

    /// The field node if declared.
    pub fn get(&self, field: &str) -> Option<&'a Node> {
        self.node.get(field).filter(|n| !n.is_null())
    }

    fn error(&self, field: &str, node: &Node, message: String) -> FieldError {
        FieldError {
            component: self.component.to_owned(),
            field: field.to_owned(),
            pos: Some(node.pos),
            message,
        }
    }

    fn scalar(&self, field: &str) -> Result<Option<(&'a Node, &'a str)>, FieldError> {
        match self.get(field) {
            None => Ok(None),
            Some(n) => match n.as_str() {
                Some(text) => Ok(Some((n, text))),
                None => Err(self.error(field, n, "expected a scalar".into())),
            },
        }
    }

    /// The original string.
    pub fn string(&self, field: &str) -> Result<Option<String>, FieldError> {
        Ok(self.scalar(field)?.map(|(_, t)| t.to_owned()))
    }

    /// A boolean (`true` or `false`).
    pub fn bool(&self, field: &str) -> Result<Option<bool>, FieldError> {
        match self.scalar(field)? {
            None => Ok(None),
            Some((n, t)) => parse_bool(t).map(Some).ok_or_else(|| {
                self.error(field, n, format!("expected true or false, found `{t}`"))
            }),
        }
    }

    /// An integer.
    pub fn int(&self, field: &str) -> Result<Option<i64>, FieldError> {
        match self.scalar(field)? {
            None => Ok(None),
            Some((n, t)) => t
                .parse()
                .map(Some)
                .map_err(|_| self.error(field, n, format!("expected an integer, found `{t}`"))),
        }
    }

    /// A number.
    pub fn float(&self, field: &str) -> Result<Option<f64>, FieldError> {
        match self.scalar(field)? {
            None => Ok(None),
            Some((n, t)) => t
                .parse()
                .map(Some)
                .map_err(|_| self.error(field, n, format!("expected a number, found `{t}`"))),
        }
    }

    /// A list of strings.
    pub fn string_list(&self, field: &str) -> Result<Option<Vec<String>>, FieldError> {
        let Some(n) = self.get(field) else {
            return Ok(None);
        };
        let Value::Seq(items) = &n.value else {
            return Err(self.error(field, n, "expected a list".into()));
        };
        items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| self.error(field, item, "expected a string".into()))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }
}

/// The field value could not be decoded as part of the component.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FieldError {
    /// Component name.
    pub component: String,
    /// Field name.
    pub field: String,
    /// Source position of the value.
    pub pos: Option<Pos>,
    /// Cause.
    pub message: String,
}

impl fmt::Display for FieldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(pos) = self.pos {
            write!(f, "{pos}: ")?;
        }
        write!(f, "{}.{}: {}", self.component, self.field, self.message)
    }
}

impl std::error::Error for FieldError {}

/// Failed to construct a component from fields with [`ComponentFactories::add_component`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ComponentDataError {
    /// A field value could not be read.
    Field(FieldError),
    /// The world rejected the component, for example because it already exists.
    Component(ComponentError),
}

impl fmt::Display for ComponentDataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ComponentDataError::Field(e) => write!(f, "{e}"),
            ComponentDataError::Component(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ComponentDataError {}

type Insert = Box<dyn FnOnce(&mut World, EntityUid) -> Result<(), ComponentError> + Send>;
type Build = dyn Fn(&ComponentData<'_>) -> Result<Insert, FieldError> + Send + Sync;

/// Reads a component of type `C` from prototype fields.
pub type ReadComponent<C> = Arc<dyn Fn(&ComponentData<'_>) -> Result<C, FieldError> + Send + Sync>;

/// Component factories keyed by name, corresponding to prototype-based creation
/// in the Robust `ComponentFactory`.
#[derive(Clone, Default)]
pub struct ComponentFactories {
    by_name: HashMap<String, Arc<Build>>,
}

impl ComponentFactories {
    /// Creates an empty registry.
    pub fn new() -> Self {
        ComponentFactories::default()
    }

    /// Registers a factory for component `C` under `name`. Returns `false` if the name is taken.
    pub fn insert<C: Component>(&mut self, name: &str, read: ReadComponent<C>) -> bool {
        if self.by_name.contains_key(name) {
            return false;
        }
        let build: Arc<Build> = Arc::new(move |data: &ComponentData<'_>| {
            let value = read(data)?;
            let insert: Insert =
                Box::new(move |world: &mut World, uid: EntityUid| world.add_component(uid, value));
            Ok(insert)
        });
        self.by_name.insert(name.to_owned(), build);
        true
    }

    /// Adds component `name` to entity `uid` from the resolved field mapping `data`.
    /// Returns `Ok(false)` if no factory exists. The map loader uses this with
    /// file fields applied over the prototype fields.
    pub fn add_component(
        &self,
        world: &mut World,
        uid: EntityUid,
        name: &str,
        data: &Node,
    ) -> Result<bool, ComponentDataError> {
        let Some(build) = self.by_name.get(name) else {
            return Ok(false);
        };
        let insert = build(&ComponentData::new(name, data)).map_err(ComponentDataError::Field)?;
        insert(world, uid).map_err(ComponentDataError::Component)?;
        Ok(true)
    }

    /// Checks whether a factory is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }

    /// Number of factories.
    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

/// An error that prevents entity creation from a prototype.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SpawnError {
    /// The prototype is missing or abstract.
    Prototype(InstantiateError),
    /// The world has no [`Prototypes`] resource.
    NoPrototypes,
    /// No factory is registered for a component in the prototype.
    MissingFactory {
        /// Prototype identifier.
        prototype: String,
        /// Component name.
        component: String,
    },
    /// A field value could not be read.
    Field {
        /// Prototype identifier.
        prototype: String,
        /// Field error.
        error: FieldError,
    },
    /// The world rejected the component.
    Component {
        /// Prototype identifier.
        prototype: String,
        /// Component error.
        error: ComponentError,
    },
}

impl fmt::Display for SpawnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpawnError::Prototype(e) => write!(f, "{e}"),
            SpawnError::NoPrototypes => f.write_str("the world has no prototype set"),
            SpawnError::MissingFactory {
                prototype,
                component,
            } => write!(
                f,
                "prototype `{prototype}`: no factory for component `{component}`"
            ),
            SpawnError::Field { prototype, error } => write!(f, "prototype `{prototype}`: {error}"),
            SpawnError::Component { prototype, error } => {
                write!(f, "prototype `{prototype}`: {error}")
            }
        }
    }
}

impl std::error::Error for SpawnError {}

/// Immediately creates an entity from a resolved `entity` prototype with all its field values.
/// Emits `ComponentAdd` on insertion, followed by `ComponentInit` and `ComponentStartup`
/// during initialization. The entity metadata retains the prototype identifier.
/// A missing or abstract prototype or a field decoding error prevents entity creation.
/// All component values are read before the entity is created.
pub fn spawn_entity(
    world: &mut World,
    prototypes: &PrototypeSet,
    factories: &ComponentFactories,
    id: &str,
) -> Result<EntityUid, SpawnError> {
    let proto = prototypes
        .instantiable(ENTITY_KIND, id)
        .map_err(SpawnError::Prototype)?;
    let mut inserts = Vec::with_capacity(proto.components.len());
    for component in &proto.components {
        let build =
            factories
                .by_name
                .get(&*component.name)
                .ok_or_else(|| SpawnError::MissingFactory {
                    prototype: id.to_owned(),
                    component: component.name.to_string(),
                })?;
        let data = ComponentData::new(&component.name, &component.data);
        inserts.push(build(&data).map_err(|error| SpawnError::Field {
            prototype: id.to_owned(),
            error,
        })?);
    }
    let uid = world.spawn_uninitialized();
    world.set_entity_prototype(uid, Some(Arc::clone(&proto.id)));
    for insert in inserts {
        if let Err(error) = insert(world, uid) {
            world.delete(uid);
            return Err(SpawnError::Component {
                prototype: id.to_owned(),
                error,
            });
        }
    }
    world.initialize_entity(uid);
    Ok(uid)
}

/// World resource with the prototype set and factories for use by systems and extension points
/// through [`SpawnExt::spawn_prototype`]. Hot reload replaces the entire set.
#[derive(Clone)]
pub struct Prototypes {
    /// Resolved prototypes.
    pub set: Arc<PrototypeSet>,
    /// Component factories.
    pub factories: Arc<ComponentFactories>,
    /// Registry used to load the set, including prototype kinds and component definitions.
    /// The map loader uses the registry to identify ignored components.
    pub registry: Arc<Registry>,
}

/// World methods for creating entities from prototypes.
pub trait SpawnExt {
    /// Creates an entity from `entity` prototype `id` with [`spawn_entity`].
    fn spawn_prototype(&mut self, id: &str) -> Result<EntityUid, SpawnError>;
}

impl SpawnExt for World {
    fn spawn_prototype(&mut self, id: &str) -> Result<EntityUid, SpawnError> {
        let Prototypes { set, factories, .. } = self
            .resource::<Prototypes>()
            .cloned()
            .ok_or(SpawnError::NoPrototypes)?;
        spawn_entity(self, &set, &factories, id)
    }
}

/// A spawn policy that creates each participant from `prototype`.
///
/// If the world has a [`Players`] resource and the participant has a session,
/// [`PlayerManager::attach`] attaches the character. A connected lobby player then enters
/// the game through [`PlayerManager::enter_game`]
/// with the newly created round character.
pub struct SpawnFromPrototype {
    /// Identifier of an `entity` prototype.
    pub prototype: String,
}

impl crucible_loop::SpawnPolicy for SpawnFromPrototype {
    fn spawn(&mut self, world: &mut World, player: &NetUserId, _role: &str) -> Option<EntityUid> {
        let uid = match world.spawn_prototype(&self.prototype) {
            Ok(uid) => uid,
            Err(error) => {
                tracing::error!(target: "crucible_ext::spawn", %player, %error, "failed to spawn character");
                return None;
            }
        };
        let status = world
            .resource::<Players>()
            .and_then(|p| p.session(player))
            .map(|s| s.status());
        if status == Some(SessionStatus::Connected)
            && let Err(error) = PlayerManager::enter_game(world, player)
        {
            tracing::warn!(target: "crucible_ext::spawn", %player, %error, "player failed to enter the game");
        }
        if status.is_some()
            && let Err(error) = PlayerManager::attach(world, player, uid)
        {
            tracing::warn!(target: "crucible_ext::spawn", %player, %error, "failed to attach character to player session");
        }
        Some(uid)
    }
}

/// Broadcast event for a prototype reload in development builds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PrototypesReloaded {
    /// Number of prototypes in the new set.
    pub prototypes: usize,
}
