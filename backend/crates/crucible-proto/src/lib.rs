//! Prototype YAML loading, inheritance, and validation.
//!
//! Loading runs in two passes:
//! 1. [`parse_str`] parses YAML into [`Node`] trees with source positions and `!type:` tags.
//! 2. [`PrototypeLoader`] indexes records by kind and ID, resolves overrides and inheritance,
//!    discards ignored components, and validates records against the [`Registry`]. Errors
//!    accumulate in the [`Report`], and the resulting [`PrototypeSet`] is immutable.
//!
//! A resolved prototype exposes component data ([`Component`], a name and node tree).
//! Entity creation can build on these values independently of an ECS implementation.

mod diag;
mod load;
mod merge;
mod node;
mod parse;
mod schema;
mod set;
mod validate;

pub use diag::{Diagnostic, DiagnosticKind, ProtoRef, Report, Severity};
pub use load::{DataRoot, LoadResult, PrototypeLoader, RootId};
pub use node::{Node, Pos, Scalar, Value, parse_bool};
pub use parse::{ParseError, ParseErrorKind, parse_str};
pub use schema::{
    ComponentDef, FieldDef, FieldType, IgnoreCategory, KindDef, PolymorphicDef, Registry,
    RegistryError,
};
pub use set::{Component, InstantiateError, Prototype, PrototypeSet};
