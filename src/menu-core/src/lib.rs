//! Host-side driver for the 1FPGA menu-core FPGA bitstream.
//!
//! This crate implements the binary contract defined in
//! `cores/menu-core/PROTOCOL.md` (protocol version 1). It provides:
//!
//! - Typed command encoding (`protocol::commands`) — writes the
//!   variable-length command stream the FPGA consumes from DDR3.
//! - Control register offsets and helpers (`protocol::registers`).
//! - Texture descriptor layout (`protocol::descriptors`).
//! - Memory layout of the 256 MB DDR3 carve-out (`mem`).
//! - SPSC ring writer with NOP-pad wrap (`ring`).
//! - A simple bump allocator for the texture pool (`allocator`).
//!
//! The library portion is platform-agnostic pure Rust and covered by
//! unit tests on any host. Hardware integration (mmap of `/dev/mem`,
//! FPGA bitstream loading) lives in the binary entry point.

pub mod allocator;
pub mod bridge;
pub mod mem;
pub mod protocol;
pub mod ring;

// The binary target shares this package's Cargo.toml, so deps only used
// by the binary (and by tests) still need to be referenced from the
// library to satisfy the workspace's `unused_crate_dependencies` lint.
use clap as _;
use clap_verbosity_flag as _;
use core_affinity as _;
use cyclone_v as _;
use mister_fpga as _;
#[cfg(test)]
use pretty_assertions as _;
use tracing as _;
use tracing_subscriber as _;
