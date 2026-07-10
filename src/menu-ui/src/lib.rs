//! `menu-ui` — React-on-Boa UI framework over `menu-core-host`.
//!
//! The crate ships as both a library (the framework runtime) and a
//! binary (`menu_ui`, the launcher). Apps are written in TypeScript /
//! JSX, bundled by Rollup into `dist/menu_ui.js`, and either embedded
//! into the binary or loaded from disk via `--bundle <path>`.
//!
//! At the highest level: JS calls [`1fpga:gui`] host functions to
//! mutate a Rust-side host node tree ([`vdom`]); the [`runtime`] frame
//! loop walks the tree, lays it out, [`paint`]s each node into a
//! `menu_core_host::Frame`, and submits.
//!
//! N1 (current milestone) ships the smallest possible vertical slice:
//! one host module, one node type (`div`), absolute positioning only,
//! no layout engine, no React. Subsequent milestones layer Taffy,
//! text, images, react-reconciler, and input on top.

pub mod db;
pub mod display_list;
pub mod font;
pub mod fs_mod;
pub mod host;
pub mod image;
pub mod input;
pub mod layout;
pub mod paint;
pub mod runtime;
pub mod style;
pub mod text;
pub mod vdom;

pub use runtime::{RunConfig, RuntimeError, run};

// Crates the binary uses; declared here to satisfy the workspace
// `unused_crate_dependencies` lint on the lib target.
use clap as _;
use clap_verbosity_flag as _;
use core_affinity as _;
use tracing_subscriber as _;
