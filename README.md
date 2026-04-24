# 1FPGA menu-core

FPGA bitstream that renders the 1FPGA main menu on a DE10-Nano. Replaces
MiSTer's Menu core with a sprite/blit/alpha-capable 2D GPU whose ARM-side
driver (`src/menu-core/`) streams commands via a shared DDR3 ring buffer.

**Status:** v0 skeleton. The current RTL produces a configured MISTER_FB
pointed at the reserved DDR3 region; the blit engine, control registers,
and command ring consumer are not yet implemented. This milestone (M1)
proves the build-system and framework-integration paths.

## Architecture

See [`PROTOCOL.md`](PROTOCOL.md) for the complete host/FPGA contract.

High-level data flow:

```
ARM (Rust, src/menu-core)       FPGA (this directory)
─────────────────────────       ─────────────────────
writes textures, commands  ──►  DDR3 reserved region (0x30000000, 256 MB)
                                       │
                                       │ reads
                                       ▼
                                blit engine  (future)
                                       │ AXI writes
                                       ▼
                                framebuffer slot (BGRA8888 @ 1920×1080)
                                       │
                                       │ read by framework's ASCAL
                                       ▼
                                ADV7513  ──►  HDMI
```

The framework's `MISTER_FB` path (enabled via `VERILOG_MACRO "MISTER_FB=1"`
in `menu_core.qsf`) handles HDMI scanout, pixel clock, timing, and
ADV7513 I2C. Our RTL is limited to the blit engine, LW_H2F control
registers, and command ring consumer — everything else comes from
`sys/`.

## Directory layout

```
cores/menu-core/
├── PROTOCOL.md            Binary contract (source of truth)
├── README.md              (this file)
├── menu_core.qpf          Quartus project file
├── menu_core.qsf          Quartus settings + MISTER_FB macros
├── menu_core.sdc          Timing constraints (framework defaults)
├── menu_core.srf          Suppressed warnings
├── menu_core.sv           Glue module (`emu`) with MISTER_FB wiring
├── files.qip              Explicit RTL file list
├── clean.bat              Quartus artifact cleanup (Windows)
├── .gitignore             Quartus build outputs
├── rtl/                   Core-specific RTL (empty at M1)
│   └── README.md          Planned layout
└── sys/                   Vendored MiSTer framework (DO NOT EDIT)
    └── VENDOR.md          Upstream pinning info
```

The `sys/` directory is a verbatim copy of the `sys/` folder from
[`MiSTer-devel/Template_MiSTer`](https://github.com/MiSTer-devel/Template_MiSTer)
at commit `cce023f4ea34a5088a5ce5b45c90ad2a4493c6ac`. Per MiSTer
convention, nothing inside `sys/` may be modified — any local changes
would be lost on the next re-sync.

## Prerequisites

- **Quartus Prime Lite 17.0.2** — the only version supported by the
  current MiSTer framework. Newer Quartus versions introduce project
  file incompatibilities. The build runs in a Docker container so no
  host install is required; see next section.
- **Docker** — for the Quartus build environment.
- **A DE10-Nano** with the stock MiSTer kernel (provides the
  `memmap=513M$511M` reservation our carve-out lives in).

## Build

### One-time: prepare the Quartus Docker image

```sh
just menu-core-image
```

This pulls `theypsilon/quartus-lite-c5:17.0.2` from Docker Hub
(~2.8 GB) — a community-maintained Quartus 17.0.2 image pre-loaded
with Cyclone V device support. Build time is dominated by the image
pull (typically a few minutes on broadband).

**Background:** we originally built Quartus ourselves from Intel's
tarball. Intel's 17.0.2 installer has a reproducible deadlock in
unattended mode that reliably hangs after ~10-30 minutes on both
macOS/QEMU and native amd64 Linux (confirmed 2026-04-24 on a Steam
Deck). The community image sidesteps this entirely.

**Apple Silicon (M-series Mac):** Docker Desktop's QEMU emulation of
x86_64 has separately been shown to hang Quartus's compile tools.
Build on a real amd64 host (Linux box, Steam Deck, cloud VM, CI).
Once the RBF is produced, deployment to the DE10-Nano is identical
regardless of where it was compiled.

Advanced: if you need to pin a different Quartus version, update the
`FROM` line in `docker/quartus/Dockerfile` to another tag (upstream
publishes 17.0, 17.0.2, 17.1, 18.0, 18.1, 19.1; the `-heavy` suffix
keeps docs/examples/extra device packs). MiSTer's framework
specifically requires the 17.0.x series — do not upgrade without
understanding the project-file compatibility implications.

### Build the core

```sh
just build-menu-core
```

This runs `quartus_sh --flow compile menu_core.qpf` inside the container.
Output lands in `cores/menu-core/output_files/menu_core.rbf`. First
build is ~5–10 min; subsequent builds take ~2–3 min thanks to Quartus's
`SMART_RECOMPILE ON`.

### Interactive Quartus shell

```sh
just quartus-shell
```

Drops you into a bash shell inside the container with the project
mounted at `/work`. Useful for running `quartus_sta` for timing
analysis or `quartus_fit` in isolation.

## Deploy to device

```sh
just deploy-menu-core
```

Builds (if needed) and `scp`s the `.rbf` to `/media/fat/menu_core.rbf`
on the MiSTer device (defaults to `192.168.1.79`, override with
`MISTER_IP=...` in the environment).

Once on the device, the ARM-side host (`one_fpga_menu_core`, in
`src/menu-core/`) will program it into the FPGA, configure the control
registers, and begin rendering. (That host binary is currently a
skeleton — tracked in a later milestone.)

## Milestones

| Milestone | Status | Description |
|-----------|--------|-------------|
| **M1** | 🟡 scaffolded | Black screen at 1080p over HDMI via MISTER_FB; framework integration proven. |
| **M2** | ⏳ | Blit engine + LW_H2F control registers + command ring consumer. First sprite on screen from the ARM host. |
| **M3** | ⏳ | Alpha blending (SrcAlpha / Additive), triple-buffer vsync swap, FENCE / PRESENT semantics. |
| **M4** | ⏳ | A8 textures + TTF font atlas; full GUI capabilities. |

## Troubleshooting

**Docker image pull fails or is very slow**  
The base image (`theypsilon/quartus-lite-c5:17.0.2`) is hosted on
Docker Hub. Rate limits or transient network issues can slow the
pull. Retry `just menu-core-image` — layers are cached once pulled.
To probe connectivity:

```sh
docker manifest inspect theypsilon/quartus-lite-c5:17.0.2
```

**"sys_top is not a synthesizable entity"**  
The `sys/sys.tcl` pipeline didn't run. Verify `menu_core.qsf` still
has the `source sys/sys.tcl` line and that the `sys/` directory was
not accidentally emptied.

**`.rbf` loads but display is garbage**  
DDR3 at `0x30000000` is uninitialized. The ARM host must `memset`
the framebuffer to zero before the core is enabled, or FB_FORCE_BLANK
must be asserted until the first frame is drawn.

## References

- `PROTOCOL.md` — full binary contract
- `src/menu-core/` — ARM-side Rust host driver
- [`MiSTer-devel/Template_MiSTer`](https://github.com/MiSTer-devel/Template_MiSTer)
  — upstream framework
- DE10-Nano User Manual — pinout and board-level docs
- Intel Cyclone V SoC HPS Technical Reference Manual — HPS bridges and
  DDR3 controller specifics
