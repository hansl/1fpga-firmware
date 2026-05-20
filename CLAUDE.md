# 1FPGA Firmware

MiSTer FPGA firmware replacement written in Rust, targeting the DE10-Nano (Intel Cyclone V SoC). Drop-in replacement for the MiSTer firmware with a modern codebase and JavaScript scripting support.

## Project Structure

Monorepo with a Rust workspace (13 crates) and NPM workspaces (5 packages).

### Rust Crates (`src/`)

| Crate | Purpose |
|-------|---------|
| `firmware` | Main binary (`one_fpga_bin`). CLI entry point, initializes tracing, pins CPU core, launches script engine. |
| `one-fpga` | Core `Core` trait — generic interface for all emulator cores (ROM, save states, input, settings). |
| `mister-fpga` | MiSTer core implementation. SPI protocol, framebuffer, OSD, config strings, user I/O. |
| `cyclone-v` | Low-level Cyclone V FPGA register access via `/dev/mem` mmap. |
| `de10-nano` | DE10-Nano board support (I2C battery monitoring). |
| `firmware-ui` | UI application: menus, panels, OSD rendering, platform abstraction (DE10/desktop). |
| `firmware-gui` | Linux framebuffer GUI framework using `calloop` event loop. |
| `firmware-script` | Boa JavaScript engine integration. Exposes 1FPGA APIs to scripts. |
| `mister-fpga-ini` | MiSTer INI/JSON5 config file parser. |
| `fce-movie-format` | NES movie format parser. |
| `taser` | Movie player CLI utility. |
| `video-test` | Framebuffer rendering test (`/dev/fb0`). |
| `games-db-converter` | Game database CSV/XML/JSON converter. |

### JavaScript/TypeScript (`js/`)

| Package | Purpose |
|---------|---------|
| `js/frontend` | Main TypeScript frontend, compiled via Rollup to `dist/main.js`. Runs inside the Boa JS engine on device. |
| `js/frontend-react` | Next.js React frontend for development/debugging without hardware. |
| `js/1fpga/schemas` | Zod schemas for catalogs, settings. Generates JSON schemas and TypeScript types. |
| `js/1fpga/types` | TypeScript type definitions for `1fpga:*` module namespace. |
| `scripts/patreon` | Patreon API integration for patron credits. |

## Build

### Prerequisites
- Rust 1.88 stable (via `rust-toolchain.toml`)
- Node.js + npm
- Docker (for ARM cross-compilation)
- `just` (https://github.com/casey/just) — build/deploy entry point

### Key Commands

Build/deploy is driven by `just` recipes in `./justfile`. There is no
Makefile. Run `just --list` (or `just` with no args) to see every
recipe.

```bash
# Full build (frontend + ARM binary via Docker). Default mode is
# release-dev; pass `release` for a fully-optimized build.
just build               # = just build release-dev
just build release

# Frontend only
just build-frontend
# or, equivalently: npm run build

# ARM binary only (requires Docker)
just build-1fpga          # = just build-1fpga release-dev
just build-1fpga release

# Build and sign binary. Positional args: mode then key path.
# Omit the key path and the recipe prompts for it.
just build-and-sign release /path/to/key

# Desktop build (no FPGA support, for UI development)
cargo run --bin one_fpga

# Menu-core (FPGA RBF) build via Dockerized Quartus
just build-menu-core
just deploy-menu-core     # build + scp .rbf to the device

# Menu-ui (host-side React-on-Boa runtime)
just build-menu-ui        # cross-compile via musl image
just deploy-menu-ui       # build + scp to /media/fat/menu_ui
just run-menu-ui          # ssh + run on device with the deployed bundle
just demo-menu-ui         # build + deploy bundle + assets + run in one shot

# Run tests
cargo test                # Rust tests
npm test                  # JS tests (@1fpga/schemas + @1fpga/frontend)

# Deploy frontend to device
just deploy-frontend      # rsync to MISTER_IP (default 192.168.1.79)

# Create new DB migration
just new-migration my_migration
```

Recipe arguments are positional, not env-var style (`just build
release`, not `make build MODE=release`). Override the deploy target
via `MISTER_IP=… just deploy-…` (the justfile reads `MISTER_IP` from
the environment).

### Docker Cross-Compilation

The ARM build uses a multi-stage Docker image (`docker/armv7/de10nano.Dockerfile`):
1. `cargo-chef` caches dependencies for fast rebuilds
2. Cross-compiles to `armv7-unknown-linux-gnueabihf`
3. Uses `mold` linker for speed

Target binary: `target/armv7-unknown-linux-gnueabihf/release/one_fpga`

### Feature Flags

- `platform_de10` — DE10-Nano hardware (default for ARM builds, `--no-default-features --features=platform_de10`)
- `platform_desktop` — Desktop simulator (default for `cargo run`)

## Architecture

### Boot Flow
1. `main.rs`: Parse CLI args, pin to CPU core 1, init tracing
2. `firmware_script::run()`: Initialize Boa JS engine
3. JS frontend (`main.js`) takes over: loads cores, manages UI

### Hardware Interaction
- **FPGA programming**: `cyclone-v` crate maps physical memory via `/dev/mem`
- **Core communication**: SPI protocol over 16-bit data bus (`mister-fpga`)
- **Display**: Linux framebuffer (`/dev/fb0`) + OSD overlay via SPI
- **Input**: SDL3 events → PS/2 scancodes / gamepad data → SPI commands

### Key Patterns
- `Core` trait in `one-fpga` — all emulator cores implement this
- `Rc<UnsafeCell<T>>` for single-threaded interior mutability (no Send/Sync)
- Unsafe code concentrated in: memory mapping, volatile register access, framebuffer lifetime management
- `thiserror` for error types throughout

## Code Style

### Rust
- Edition 2024 (some legacy crates still on 2021)
- `#[warn(unused_crate_dependencies)]` workspace-wide
- Release profile: LTO, panic=abort, stripped

### TypeScript/JavaScript
- Prettier: 100 char width, 2-space indent, single quotes, trailing commas
- Import ordering: `1fpga:*` first, then other namespaced, then `@/`, then relative
- Rollup for bundling with custom plugins (codegen, migrations, template literals)

## Testing
- Rust: `cargo test` — uses `rstest`, `pretty_assertions`
- JS: Jest with `ts-jest` — schema validation tests, version comparison tests
- Note: Rust tests require `cmake` installed locally (for native builds)

## Deployment

Target device: DE10-Nano at `MISTER_IP` (default `192.168.1.79`).

Prefer the `just` deploy recipes — they handle the kill-current-procs
+ scp + run dance automatically:

```bash
just deploy-menu-core      # FPGA bitstream
just deploy-menu-ui        # main launcher binary
just demo-menu-ui          # build + deploy bundle + assets + run
just probe-menu-core       # device-side probe / smoke tests
```

The recipes assume `MISTER_IP` (the env var or its default
`192.168.1.79`). Override via `MISTER_IP=10.0.0.5 just deploy-…`.

For one-off manual deploys of the firmware binary:

```bash
ssh root@$MISTER_IP 'killall MiSTer one_fpga'
scp target/armv7-unknown-linux-gnueabihf/release/one_fpga root@$MISTER_IP:/media/fat/one_fpga
ssh root@$MISTER_IP 'sync; /media/fat/one_fpga'
```

Frontend deployed via `just deploy-frontend` (rsync to `/root/frontend`).

## License

Apache 2.0
