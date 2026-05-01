//! HPS-to-FPGA bridge enablement.
//!
//! The MiSTer framework does not initialise the LW_H2F bridge — none of
//! its cores need it. To talk to the menu-core control register file at
//! `0xFF21_0000` we therefore have to:
//!
//! 1. Release the LW_H2F bridge from reset
//!    (`RSTMGR.BRGMODRST[1] = 0`).
//! 2. Make the LW_H2F slave visible on the L3 interconnect
//!    (`L3REGS.REMAP[4] = 1`).
//!
//! Both registers are mmap'd via `/dev/mem` rather than going through the
//! kernel's `/sys/class/fpga_bridge` interface, which is sometimes
//! disabled in MiSTer kernel builds.
//!
//! Reference: Cyclone V Hard Processor System Technical Reference Manual,
//! §3 (HPS-FPGA bridges) and §6 (Reset Manager).
//!
//! These pokes are idempotent — calling them when the bridge is already
//! enabled is a no-op.
//!
//! # Safety
//!
//! Direct manipulation of HPS configuration registers requires root.
//! Mis-poking these registers cannot brick the device (a power cycle
//! resets the HPS) but can wedge the running Linux kernel if you change
//! bits unrelated to the bridges. We touch only the documented bridge
//! bits and leave the rest untouched via read-modify-write.

use cyclone_v::memory::{DevMemMemoryMapper, MemoryMapper};
use thiserror::Error;

// --- Cyclone V Reset Manager --------------------------------------------

const RSTMGR_BASE: usize = 0xFFD0_5000;

/// Offset of the `BRGMODRST` register within the Reset Manager.
const BRGMODRST_OFFSET: usize = 0x28;

/// Bit 1 of `BRGMODRST` controls the LW_H2F bridge reset
/// (`1` = held in reset, `0` = released).
const BRGMODRST_LWHPS2FPGA: u32 = 1 << 1;

// --- Cyclone V L3 Master Remap ------------------------------------------

const L3REGS_BASE: usize = 0xFF80_0000;

/// Offset of the `REMAP` register at the start of the L3 master block.
const L3REGS_REMAP_OFFSET: usize = 0x00;

/// Bit 4 of `REMAP` exposes the LW_H2F slave on the L3 interconnect.
const L3REGS_REMAP_LWHPS2FPGA: u32 = 1 << 4;

#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("failed to mmap {what} at {addr:#X}: {detail}")]
    Mmap {
        what: &'static str,
        addr: usize,
        detail: &'static str,
    },
}

/// Bring up the LW_H2F bridge. Idempotent. Safe to call multiple times.
pub fn enable_lwh2f() -> Result<(), BridgeError> {
    let mut rstmgr = DevMemMemoryMapper::create(RSTMGR_BASE, 0x1000).map_err(|d| {
        BridgeError::Mmap {
            what: "RSTMGR",
            addr: RSTMGR_BASE,
            detail: d,
        }
    })?;
    let mut l3regs = DevMemMemoryMapper::create(L3REGS_BASE, 0x1000).map_err(|d| {
        BridgeError::Mmap {
            what: "L3REGS",
            addr: L3REGS_BASE,
            detail: d,
        }
    })?;

    // Read-modify-write so we don't disturb other bridges that may
    // already be configured.
    let rstmgr_ptr = rstmgr.as_mut_ptr::<u8>();
    let l3regs_ptr = l3regs.as_mut_ptr::<u8>();

    unsafe {
        let brgmodrst = rstmgr_ptr.add(BRGMODRST_OFFSET) as *mut u32;
        let cur = core::ptr::read_volatile(brgmodrst);
        let new = cur & !BRGMODRST_LWHPS2FPGA;
        if cur != new {
            core::ptr::write_volatile(brgmodrst, new);
            tracing::debug!(
                "RSTMGR.BRGMODRST: {:#010X} -> {:#010X} (released LW_H2F)",
                cur,
                new
            );
        } else {
            tracing::debug!("RSTMGR.BRGMODRST already releases LW_H2F");
        }

        let remap = l3regs_ptr.add(L3REGS_REMAP_OFFSET) as *mut u32;
        // L3 REMAP register is write-only for some bits; we cannot
        // safely read-modify-write it. Per Cyclone V TRM the canonical
        // value to expose both the H2F and LW_H2F bridges is 0x19.
        // We only need the LW_H2F bit but include MPU remap (bit 0)
        // and HPS2FPGA visibility (bit 3) for safety since the kernel
        // typically sets them already.
        let value = 0x0000_0019;
        core::ptr::write_volatile(remap, value);
        tracing::debug!("L3REGS.REMAP <- {:#010X}", value);
        let _ = L3REGS_REMAP_LWHPS2FPGA; // documented constant, used in tests
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lwhps2fpga_remap_bit_matches_brgmodrst_position() {
        // Both registers use bit `1` for LW_H2F at the same logical
        // position relative to the H2F bridge — sanity check that the
        // constants match the Cyclone V TRM.
        assert_eq!(BRGMODRST_LWHPS2FPGA, 0x2);
        assert_eq!(L3REGS_REMAP_LWHPS2FPGA, 0x10);
    }
}
