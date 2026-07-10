# 1FPGA menu-core (FPGA half)

FPGA bitstream that renders the 1FPGA main menu on a DE10-Nano. Replaces
MiSTer's `Menu.rbf` with a sprite/blit/alpha-capable 2D GPU whose ARM-side
driver streams commands via a shared DDR3 ring buffer.

This repo contains only the **FPGA portion** — Verilog/SystemVerilog,
Quartus project files, and the vendored MiSTer `sys/` framework. It is
released under **GPLv2** because `sys/` is a verbatim copy of
[`MiSTer-devel/Template_MiSTer`](https://github.com/MiSTer-devel/Template_MiSTer)
and `menu_core.sv` is derived from `Template.sv`. See `LICENSE`.

## Companion repository

The **ARM-side Rust driver**, the **binary protocol specification**
(`PROTOCOL.md`), and the **Quartus Docker build environment** live in
the public 1FPGA firmware repo:

> `1fpga/firmware`  (Apache-2.0)

That repo holds:

- `src/menu-core/` — Rust crate that implements the host side of the
  protocol (encodes commands, manages the DDR3 carve-out, drives the
  command ring).
- `cores/menu-core/PROTOCOL.md` — the canonical binary contract between
  the Rust host and this RTL. Source of truth for register offsets,
  command encoding, and memory layout.
- `docker/quartus/` — the Docker image used to compile this project.
- `justfile` — recipes (`just menu-core-image`, `just build-menu-core`,
  `just deploy-menu-core`) referencing this repo as a sibling checkout.

The `justfile` recipes assume this repo is checked out at
`<firmware>/cores/menu-core` (i.e., the path the public repo's tooling
expects). A typical local layout:

```
~/work/
├── firmware/                  # public repo (Apache 2.0)
│   ├── src/menu-core/
│   ├── docker/quartus/
│   ├── justfile
│   └── cores/
│       └── menu-core/  →  symlink or checkout of this repo
```

## Status

**M1 reached** — black screen at 1080p over HDMI via MISTER_FB,
framework integration proven (verified 2026-04-26 on DE10-Nano hardware
with the matching Rust skeleton). Subsequent milestones add the actual
rendering engine.

| Milestone | Status | Description |
|-----------|--------|-------------|
| **M1** | ✅ done | Stable 1080p HDMI output; framework integration proven; ARM mmap path verified. |
| **M2** | ⏳ | Blit engine + LW_H2F control registers + command ring consumer. First sprite on screen from the ARM host. |
| **M3** | ⏳ | Alpha blending (SrcAlpha / Additive), triple-buffer vsync swap, FENCE / PRESENT semantics. |
| **M4** | ⏳ | A8 textures + TTF font atlas; full GUI capabilities. |

## Architecture

```
ARM (Rust)                                 FPGA (this repo)
──────────                                 ────────────────
writes textures, commands  ──►   DDR3 reserved region (0x30000000, 256 MB)
                                        │
                                        │ reads
                                        ▼
                                 blit engine (M2+)
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
.
├── LICENSE                GPLv2
├── README.md              this file
├── menu_core.qpf          Quartus project file
├── menu_core.qsf          Quartus settings + MISTER_FB macros
├── menu_core.sdc          Timing constraints (framework defaults)
├── menu_core.srf          Suppressed warnings
├── menu_core.sv           Glue module (`emu`) with MISTER_FB wiring
├── files.qip              Explicit RTL file list
├── clean.bat              Quartus artifact cleanup (Windows)
├── .gitignore             Quartus build outputs
├── rtl/                   Core-specific RTL
│   ├── pll.qip            Wraps rtl/pll/pll.v
│   └── pll/pll.v          Core-side PLL (50→50 MHz pass-through, M1)
└── sys/                   Vendored MiSTer framework (DO NOT EDIT)
    └── VENDOR.md          Upstream pinning info
```

The `sys/` directory is a verbatim copy of the `sys/` folder from
[`MiSTer-devel/Template_MiSTer`](https://github.com/MiSTer-devel/Template_MiSTer)
at commit `cce023f4ea34a5088a5ce5b45c90ad2a4493c6ac`. Per MiSTer
convention, nothing inside `sys/` may be modified — any local changes
would be lost on the next re-sync.

## Build

The build environment lives in the companion repo. Quick recipe from
that repo's root:

```sh
just menu-core-image      # pull pre-built Quartus 17.0.2 image
just build-menu-core      # compile this project; output in cores/menu-core/output_files/
just deploy-menu-core     # scp the .rbf to /media/fat/menu_core.rbf
```

Direct invocation without `just`, if you only have this repo checked out:

```sh
docker run --rm -t \
    --platform linux/amd64 \
    -u "$(id -u):$(id -g)" \
    -v "$(pwd)":/work \
    theypsilon/quartus-lite-c5:17.0.2 \
    quartus_sh --flow compile menu_core.qpf
```

Output lands in `output_files/menu_core.rbf`.

## Build host compatibility

| Host | Status |
|------|--------|
| Native amd64 Linux | ✅ works (verified on Steam Deck, ~2 min compile) |
| Apple Silicon + Rosetta | ❌ Quartus binaries crash with `bss_size overflow` |
| Apple Silicon + QEMU | ❌ deadlocks during compile (`Can not read any output from quartus_map`) |

For Apple Silicon users: develop the host-side Rust on the Mac, compile
the FPGA half on a real amd64 box. The resulting `.rbf` is platform-
agnostic.

## Troubleshooting

**`.rbf` loads but display is garbage**
DDR3 at `0x30000000` is uninitialized after a fresh load. The ARM host
must `memset` the framebuffer to a known value before the user sees
anything sensible. From a shell on the device:

```sh
python3 -c "
import mmap, os
fd = os.open('/dev/mem', os.O_RDWR | os.O_SYNC)
size = 24 * 1024 * 1024
mm = mmap.mmap(fd, size, mmap.MAP_SHARED, mmap.PROT_READ | mmap.PROT_WRITE, offset=0x30000000)
mm[:] = b'\\x00' * size
mm.close()
os.close(fd)
"
```

(`dd if=/dev/zero of=/dev/mem` doesn't work — `/dev/mem`'s `write()`
path is more restricted than its `mmap()` path.)

**`Tcl Script File rtl/pll.qip not found`**
Required for synthesis to succeed (Cyclone V routing constraint
15836). The PLL is checked into this repo at `rtl/pll/pll.v` — verify
the file exists and `rtl/pll.qip` references it.

## License

GPLv2 (see `LICENSE`). The `sys/` subtree and `menu_core.sv` derive
from MiSTer-devel/Template_MiSTer (also GPLv2). Any contribution to
this repo must be GPLv2-compatible.
