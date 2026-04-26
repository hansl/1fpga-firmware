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
`just deploy-menu-core` recipes live in this public repo. They expect
the FPGA repo to be checked out at `cores/menu-core-fpga/`, which is
registered as a **git submodule** pointing at
[`one-retro/1fpga-menu-core`](https://github.com/one-retro/1fpga-menu-core).
You need read access to that private repo to fetch the submodule.

Once the submodule is initialized, the FPGA-repo HEAD that the public
repo is pinned to is recorded in this repo's tree, so a tagged release
of `1fpga-firmware` deterministically references a specific FPGA
build.

To set up:

```sh
# When initially cloning the public repo:
git clone --recurse-submodules git@github.com:1fpga/firmware.git
# Or, if you already cloned without --recurse-submodules:
git submodule update --init --recursive

just menu-core-image     # one-time: pull Quartus 17.0.2 image
just build-menu-core     # compile -> cores/menu-core-fpga/output_files/menu_core.rbf
just deploy-menu-core    # scp to /media/fat/menu_core.rbf on $MISTER_IP
```

`build-menu-core` checks for `cores/menu-core-fpga/menu_core.qpf` and
exits with a friendly error if the submodule isn't initialized.

To update the public repo's pin to a newer FPGA-repo commit:

```sh
cd cores/menu-core-fpga
git fetch origin
git checkout main && git pull
cd -
git add cores/menu-core-fpga
git commit -m "Bump menu-core-fpga to <short-sha>"
```

## Status

**M1 reached** as of 2026-04-26 — stable 1080p HDMI output on a
DE10-Nano via the framework's MISTER_FB scanout path. Subsequent
milestones (blit engine, control registers, command ring consumer,
real Rust host integration) are tracked in the FPGA repo.
