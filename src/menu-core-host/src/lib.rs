//! Host runtime for the 1FPGA menu-core FPGA bitstream.
//!
//! This crate implements the binary contract defined in
//! `cores/menu-core/PROTOCOL.md` (protocol version 1) and provides an
//! ergonomic Rust API on top of it. Three layers stack from low to high:
//!
//! 1. **Protocol primitives** — typed command encoding
//!    ([`protocol::commands`]), control register offsets and helpers
//!    ([`protocol::registers`]), texture descriptor layout
//!    ([`protocol::descriptors`]), DDR3 memory layout ([`mem`]).
//! 2. **Ring + allocator** — SPSC ring writer with NOP-pad wrap
//!    ([`ring`]) and a bump allocator over the texture pool
//!    ([`allocator`]).
//! 3. **Device runtime** — bridge enable ([`bridge`]), volatile mmap
//!    helpers ([`devmem`]), error decode ([`error`]), the [`device::Device`]
//!    handle that owns mappings and ring state, and per-frame command
//!    builders ([`frame::Frame`], [`frame::FenceToken`]).
//!
//! Layer 1+2 are platform-agnostic pure Rust and unit-tested on any
//! host. Layer 3 requires a Linux `/dev/mem` and the LW_H2F bridge.

pub mod allocator;
pub mod bridge;
pub mod mem;
pub mod protocol;
pub mod ring;

pub mod devmem;
pub mod device;
pub mod error;
pub mod frame;
pub mod texture;

pub use protocol::{BlendMode, Filter, Rect, Rgba, TextureFormat};

#[cfg(test)]
use pretty_assertions as _;
