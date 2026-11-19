//! In-process test server and virtual clients.
//!
//! [`TestServer`] starts an extension with `Launcher`. [`VirtualClient`] applies
//! transport-independent frames. Module fixtures are available in [`fixtures`].

pub mod fixtures;
mod server;

pub use server::{TestServer, VirtualClient, VirtualEntity};
