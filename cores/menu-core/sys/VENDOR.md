# Vendored framework — DO NOT EDIT

This directory is a verbatim copy of the `sys/` folder from MiSTer's
`Template_MiSTer` repository, pinned to the commit below. Per MiSTer
convention, files in this directory must not be modified by the core —
they are shared infrastructure and any local changes would be lost on
the next framework re-sync.

## Source

- Upstream: https://github.com/MiSTer-devel/Template_MiSTer
- Branch: `master`
- Commit: `cce023f4ea34a5088a5ce5b45c90ad2a4493c6ac`
- Committed: 2026-03-25T11:38:41Z
- Vendored on: 2026-04-23

## Re-syncing

To update this vendored copy:

```sh
cd /tmp && rm -rf Template_MiSTer \
  && git clone --depth 1 https://github.com/MiSTer-devel/Template_MiSTer.git \
  && cd Template_MiSTer \
  && git log -1 --format="%H %ci"  # record this SHA/date
cp -R /tmp/Template_MiSTer/sys/* <repo>/cores/menu-core/sys/
# Update the commit hash in this file.
```

After re-sync, rebuild the core via `just build-menu-core`. Expect the
build to still work — the framework aims for backwards compatibility
across revisions — but review any diffs in `sys_top.v`, `hps_io.sv`,
or `ascal.vhd` for interface changes that require updates to our
`menu_core.sv` glue module.

## Contents of note

- `sys_top.v` — top-level wrapper; hosts HDMI (ADV7513 driver), HPS
  bridges, audio, user I/O SPI, OSD.
- `hps_io.sv` — SPI `user_io` protocol endpoint; decodes CONF_STR,
  status bits, forwards video timing, etc.
- `ascal.vhd` — the ASCAL scaler; handles arbitrary input resolution
  to the current HDMI output mode.
- `sys.qip` — the canonical file list for Quartus; referenced from the
  project's top-level `files.qip`.
- `pll_hdmi.v`, `pll_audio.v`, `pll_cfg.v` — PLLs used by the
  framework (not the core-specific PLL, which lives in `../rtl/pll`).
