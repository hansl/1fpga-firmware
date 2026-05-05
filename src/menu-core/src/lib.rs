//! Test/demo binary support code.
//!
//! The actual host runtime lives in the [`menu_core_host`] crate. This
//! crate exists only to host the binaries (`one_fpga_menu_core`,
//! `menu_demo`) and a binary-side font-rasterization helper
//! ([`text`]) that depends on `fontdue` — kept out of the runtime
//! library so it stays lean.

pub use menu_core_host::{allocator, bridge, devmem, device, error, frame, mem, protocol, ring,
    texture};

pub mod text;

// The binary targets share this package's Cargo.toml, so deps only used
// by them (and by tests) still need to be referenced from the library
// to satisfy the workspace's `unused_crate_dependencies` lint.
use clap as _;
use clap_verbosity_flag as _;
use core_affinity as _;
use cyclone_v as _;
#[cfg(test)]
use pretty_assertions as _;
use thiserror as _;
use tracing as _;
use tracing_subscriber as _;
