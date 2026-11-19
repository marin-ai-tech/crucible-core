//! Core ECS with entities, components, events, and change tracking.
//!
//! This independent implementation follows RobustToolbox ECS semantics so SS14 event handlers
//! can be translated almost line for line. [`World`] methods expose the components and keep
//! sparse-set storage details internal.
//!
//! # Robust and Crucible API correspondence
//!
//! | Robust (C#) | Crucible (Rust) |
//! |---|---|
//! | `EntityUid`, `NetEntity` | [`EntityUid`] (index and generation), [`NetEntity`] (world counter) |
//! | `GetNetEntity(uid)`, `TryGetEntity(net, out uid)` | [`World::net_entity`], [`World::entity_by_net`] |
//! | `[RegisterComponent]`, `ComponentFactory` component registry | [`impl Component`](Component) + [`WorldBuilder::register_component`] |
//! | `Spawn(null)`, `CreateEntityUninitialized`, `InitializeAndStartEntity` | [`World::spawn`], [`World::spawn_uninitialized`], [`World::initialize_entity`] |
//! | `AddComp`, `EnsureComp`, `RemComp`, `HasComp`, `TryComp` | [`World::add_component`], [`World::ensure_component`], [`World::remove_component`], [`World::has`], [`World::get`] / [`World::get_mut`] |
//! | `Del`, `QueueDel`, `IsQueuedForDeletion`, `TerminatingOrDeleted` | [`World::delete`], [`World::queue_delete`], [`World::is_queued_for_deletion`], [`World::terminating_or_deleted`] |
//! | `EntitySystem`, `Initialize`, `Update(frameTime)` | [`System`], [`System::initialize`], [`System::update`] |
//! | `SubscribeLocalEvent<TComp, TEvent>(handler, before, after)` | [`SystemInit::subscribe`]`::<C, E>(handler)`[`.before::<S>()`](SubscriptionOrder::before)[`.after::<S>()`](SubscriptionOrder::after) |
//! | `SubscribeLocalEvent<TEvent>(handler)` (broadcast) | [`SystemInit::subscribe_broadcast`] |
//! | `RaiseLocalEvent(uid, ref ev)`, `RaiseLocalEvent(ref ev)` | [`World::raise`], [`World::raise_broadcast`] |
//! | `QueueLocalEvent(ev)` | [`World::queue_broadcast`], [`World::queue_raise`] |
//! | `CancellableEntityEventArgs`, `args.Cancel()`, `args.Cancelled` | [`Cancellable`] |
//! | `ComponentAdd`/`Init`/`Startup`/`Shutdown`/`Remove`, `ComponentLifeStage` | [`ComponentAdd`], [`ComponentInit`], [`ComponentStartup`], [`ComponentShutdown`], [`ComponentRemove`], [`ComponentLifeStage`] |
//! | `UpdatesBefore`, `UpdatesAfter` | [`SystemInit::updates_before`], [`SystemInit::updates_after`] |
//! | `Dirty(uid, comp)`, `[AutoNetworkedField]` + `DirtyField` | [`World::dirty`], [`macro@networked`] + `set_*` setters / [`World::dirty_fields`] |
//! | `EntityQueryEnumerator<A, B>` | [`World::query`]`::<(A, B)>()` + [`Query::next`] |
//! | `GameTick` | [`Tick`] |
//!
//! Handler conventions:
//! - A handler receives `&mut World` and [`EntityUid`] and looks up a component through
//!   [`World::get`] or [`World::get_mut`] for straightforward borrowing and nested events.
//! - Keep state required by handlers in components or world resources
//!   ([`World::insert_resource`]).
//! - Lifecycle events reach only subscribers to the component undergoing the lifecycle
//!   transition.
//! - Delivery continues after cancellation, and later handlers see the cancelled flag.
//!
//! # Translating an SS14 event handler
//!
//! An illustrative C# handler in the SS14 style:
//!
//! ```text
//! public sealed class DoorSystem : EntitySystem
//! {
//!     public override void Initialize()
//!     {
//!         SubscribeLocalEvent<DoorComponent, ActivateInWorldEvent>(OnActivate,
//!             after: new[] { typeof(LockSystem) });
//!     }
//!
//!     private void OnActivate(EntityUid uid, DoorComponent door, ActivateInWorldEvent args)
//!     {
//!         if (args.Handled) return;
//!         var attempt = new DoorOpenAttemptEvent();
//!         RaiseLocalEvent(uid, attempt);
//!         if (attempt.Cancelled) return;
//!         door.Open = true;
//!         DirtyField(uid, door, nameof(DoorComponent.Open));
//!         args.Handled = true;
//!     }
//! }
//!
//! public sealed class LockSystem : EntitySystem
//! {
//!     public override void Initialize()
//!     {
//!         SubscribeLocalEvent<LockComponent, DoorOpenAttemptEvent>(OnOpenAttempt);
//!     }
//!
//!     private void OnOpenAttempt(EntityUid uid, LockComponent lock, DoorOpenAttemptEvent args)
//!     {
//!         if (lock.Locked) args.Cancel();
//!     }
//! }
//! ```
//!
//! The corresponding Crucible handler:
//!
//! ```
//! use crucible_ecs::{Cancellable, Component, EntityUid, System, SystemInit, Tick, World, networked};
//!
//! #[networked]
//! #[derive(Default)]
//! pub struct Door {
//!     pub open: bool,
//! }
//! impl Component for Door {}
//!
//! #[derive(Default)]
//! pub struct Lock {
//!     pub locked: bool,
//! }
//! impl Component for Lock {}
//!
//! #[derive(Default)]
//! pub struct ActivateInWorld {
//!     pub handled: bool,
//! }
//!
//! #[derive(Default)]
//! pub struct DoorOpenAttempt {
//!     cancelled: bool,
//! }
//! impl Cancellable for DoorOpenAttempt {
//!     fn cancel(&mut self) {
//!         self.cancelled = true;
//!     }
//!     fn is_cancelled(&self) -> bool {
//!         self.cancelled
//!     }
//! }
//!
//! pub struct DoorSystem;
//! impl System for DoorSystem {
//!     fn initialize(&mut self, init: &mut SystemInit<'_>) {
//!         init.subscribe::<Door, ActivateInWorld, _>(on_activate).after::<LockSystem>();
//!     }
//! }
//!
//! fn on_activate(world: &mut World, uid: EntityUid, args: &mut ActivateInWorld) {
//!     if args.handled {
//!         return;
//!     }
//!     let mut attempt = DoorOpenAttempt::default();
//!     world.raise(uid, &mut attempt);
//!     if attempt.is_cancelled() {
//!         return;
//!     }
//!     Door::set_open(world, uid, true); // Write and mark the field as changed in this tick.
//!     args.handled = true;
//! }
//!
//! pub struct LockSystem;
//! impl System for LockSystem {
//!     fn initialize(&mut self, init: &mut SystemInit<'_>) {
//!         init.subscribe::<Lock, DoorOpenAttempt, _>(|world, uid, args| {
//!             if world.get::<Lock>(uid).is_some_and(|lock| lock.locked) {
//!                 args.cancel();
//!             }
//!         });
//!     }
//! }
//!
//! let mut builder = World::builder();
//! builder.add_system(DoorSystem).add_system(LockSystem);
//! let (mut world, _systems) = builder.build().expect("acyclic ordering");
//!
//! let door = world.spawn();
//! world.add_component(door, Door::default()).unwrap();
//! world.add_component(door, Lock { locked: true }).unwrap();
//!
//! world.set_current_tick(Tick(7));
//! world.raise(door, &mut ActivateInWorld::default());
//! assert!(!world.get::<Door>(door).unwrap().open); // The door is locked and the attempt is cancelled.
//!
//! world.get_mut::<Lock>(door).unwrap().locked = false;
//! world.set_current_tick(Tick(8));
//! let mut activate = ActivateInWorld::default();
//! world.raise(door, &mut activate);
//! assert!(activate.handled && world.get::<Door>(door).unwrap().open);
//! assert_eq!(world.fields_changed_since::<Door>(door, Tick(7)), Door::FIELD_OPEN);
//! ```

extern crate self as crucible_ecs;

mod builder;
mod change;
mod component;
mod entity;
mod event;
mod order;
mod query;
mod registry;
mod system;
mod world;

pub use builder::{BuildError, WorldBuilder};
pub use change::{FieldMask, NetworkedComponent, Tick};
pub use component::{
    Component, ComponentAdd, ComponentError, ComponentInit, ComponentLifeStage, ComponentRemove,
    ComponentShutdown, ComponentStartup, EntityTerminating,
};
pub use crucible_ecs_macros::networked;
pub use entity::{EntityLifeStage, EntityMeta, EntityUid, NetEntity};
pub use event::Cancellable;
pub use query::{ComponentSet, Query};
pub use registry::{
    ComponentRegistration, ComponentRegistry, FieldSchema, NetworkPolicy, RegistryError,
};
pub use system::{SubscriptionOrder, System, SystemInit, Systems};
pub use world::{DEFAULT_MAX_EVENT_DEPTH, World};
