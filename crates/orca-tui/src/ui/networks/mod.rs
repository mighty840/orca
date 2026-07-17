//! Cluster networks view — three-layer tree rendered as ASCII art:
//!   1. Public edge (domains) per node, with the service each domain routes to.
//!   2. Docker bridge networks (`orca-*`) per node, with attached services
//!      and their network aliases.
//!   3. Unreachable agents are listed with a placeholder so the operator
//!      sees who didn't respond.

mod render;
#[cfg(test)]
mod render_tests;
mod view;

pub use view::{draw_networks, rendered_line_count};
