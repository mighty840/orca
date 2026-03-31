//! NetBird WireGuard mesh networking integration.
//!
//! Manages NetBird installation, configuration, and lifecycle.
//! All inter-node traffic flows over the WireGuard mesh.

mod manager;

pub use manager::NetbirdManager;
