# menu-core

This directory holds **only** the cross-language binary protocol
specification for the menu-core feature ([`PROTOCOL.md`](PROTOCOL.md)).

The two implementations of that protocol live elsewhere:

- **ARM-side host** (Apache-2.0, Rust) — in this same repo at
  [`src/menu-core/`](../../src/menu-core).
- **FPGA-side bitstream** (GPLv2, Verilog/SystemVerilog) — in a
  separate private repository at
  [`one-retro/1fpga-menu-core`](https://github.com/one-retro/1fpga-menu-core).

The FPGA half lives in its own repo because it incorporates the
[`MiSTer-devel/Template_MiSTer`](https://github.com/MiSTer-devel/Template_MiSTer)
framework, which is GPLv2-licensed. Keeping it separate avoids
mixing GPL and Apache code in a single tree and makes the licensing
obligations on each artifact explicit.

## Build flow

The Quartus Docker build environment (in [`docker/quartus/`](../../docker/quartus))
and the `just menu-core-image` / `just build-menu-core` /
`just deploy-menu-core` recipes still live in this public repo. They
expect the FPGA repo to be checked out as a sibling at
`cores/menu-core-fpga/` (gitignored).

To set up:

```sh
# From the repo root, after cloning this firmware repo:
git clone git@github.com:one-retro/1fpga-menu-core.git cores/menu-core-fpga

just menu-core-image     # one-time: pull Quartus 17.0.2 image
just build-menu-core     # compile -> cores/menu-core-fpga/output_files/menu_core.rbf
just deploy-menu-core    # scp to /media/fat/menu_core.rbf on $MISTER_IP
```

`build-menu-core` checks for `cores/menu-core-fpga/menu_core.qpf` and
exits with a friendly error if you forgot the clone step.

## Status

**M1 reached** as of 2026-04-26 — stable 1080p HDMI output on a
DE10-Nano via the framework's MISTER_FB scanout path. Subsequent
milestones (blit engine, control registers, command ring consumer,
real Rust host integration) are tracked in the FPGA repo.
