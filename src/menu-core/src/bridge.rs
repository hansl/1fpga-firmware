//! HPS-to-FPGA bridge enablement.
//!
//! On the DE10-Nano under MiSTer's kernel the LW_H2F bridge is brought
//! up at boot — userland just mmaps `0xFF21_0000` and reads. This module
//! is a best-effort safety net that touches the kernel's
//! `/sys/class/fpga_bridge/<name>/enable` interface in case the bridge
//! is in a freshly-loaded state where it hasn't been re-enabled.
//!
//! We do NOT poke `RSTMGR.BRGMODRST` or `L3REGS.REMAP` directly: modern
//! Linux kernels with `CONFIG_STRICT_DEVMEM` deny `/dev/mem` access to
//! kernel-claimed regions like the Reset Manager, which causes a SIGBUS
//! the moment we try. The sysfs interface is the kernel-blessed path
//! and works under the same access controls as any other root-owned
//! sysfs file.
//!
//! Reference: Linux kernel drivers/fpga/altera-hps2fpga.c.

use std::fs;
use std::io;
use std::path::Path;

use thiserror::Error;

const FPGA_BRIDGE_DIR: &str = "/sys/class/fpga_bridge";

/// Name fragments the kernel uses for the lightweight HPS-to-FPGA
/// bridge across kernel versions.
const LWH2F_NAME_FRAGMENTS: &[&str] = &["lwhps2fpga", "lw_hps2fpga", "h2f_lw", "lwh2f"];

#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: io::Error,
    },
}

/// Best-effort: enable the LW_H2F bridge via the kernel's fpga_bridge
/// sysfs interface. Idempotent. If the sysfs interface is not present
/// (older kernel without the SoCFPGA bridge driver) we log a warning
/// and return Ok(()), trusting that the bridge was either already up
/// or will be configured another way (e.g. U-Boot).
pub fn enable_lwh2f() -> Result<(), BridgeError> {
    let dir = Path::new(FPGA_BRIDGE_DIR);
    if !dir.exists() {
        tracing::warn!(
            "{FPGA_BRIDGE_DIR} not present; assuming LW_H2F bridge is enabled by boot"
        );
        return Ok(());
    }

    let entries = fs::read_dir(dir).map_err(|e| BridgeError::Io {
        path: FPGA_BRIDGE_DIR.into(),
        source: e,
    })?;

    let mut found_lwh2f = false;
    for entry in entries.flatten() {
        let name_path = entry.path().join("name");
        let bridge_name = match fs::read_to_string(&name_path) {
            Ok(s) => s.trim().to_owned(),
            Err(_) => continue,
        };

        if !LWH2F_NAME_FRAGMENTS
            .iter()
            .any(|frag| bridge_name.contains(frag))
        {
            tracing::debug!(bridge = %bridge_name, "skipping non-LW_H2F bridge");
            continue;
        }

        let enable_path = entry.path().join("enable");
        fs::write(&enable_path, "1").map_err(|e| BridgeError::Io {
            path: enable_path.display().to_string(),
            source: e,
        })?;
        tracing::debug!(bridge = %bridge_name, "enabled via sysfs");
        found_lwh2f = true;
    }

    if !found_lwh2f {
        tracing::warn!(
            "no LW_H2F bridge found under {FPGA_BRIDGE_DIR}; assuming it's already enabled"
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_fragments_cover_known_kernel_variants() {
        // Quick sanity that we'd recognise the canonical Linux kernel
        // names for the lightweight bridge.
        for name in &["lwhps2fpga", "ff20:lwhps2fpga", "fpga_lwh2f", "soc:bridge_lw_hps2fpga"] {
            assert!(
                LWH2F_NAME_FRAGMENTS.iter().any(|f| name.contains(f)),
                "expected to recognise bridge name {name}"
            );
        }
    }
}
