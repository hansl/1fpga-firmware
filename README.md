# 1FPGA Firmware

This repo is the main code repo for the 1FPGA Firmware and its companion libraries and binaries.

## Crates

This repo is a monorepo, containing multiple Rust crates and JavaScript/TypeScript packages.
The main crate is `firmware` (binary named `one_fpga`), which is the actual firmware for 1FPGA.
It is meant as a drop-in replacement for MiSTer.

The Rust crates live in `src/` and include libraries for Cyclone V FPGA programming, DE10-Nano board support, MiSTer core compatibility, UI rendering, and a JavaScript scripting engine (via [Boa](https://boajs.dev/)).

The TypeScript packages live in `js/` and include the main frontend (which runs inside the Boa engine on device), shared schemas and type definitions, and a React-based development frontend for working without hardware.

## FAQ

### What is this?

1FPGA wants to be a replacement for the MiSTer Firmware, achieving the same emulation capabilities, but with a better codebase (more maintainable) and an easier, user-focused interface.

From MiSTer's [wiki](https://github.com/MiSTer-devel/Wiki_MiSTer/wiki):

> MiSTer is an open project that aims to recreate various classic computers, game consoles and arcade machines, using modern hardware.
> It allows software and game images to run as they would on original hardware, using peripherals such as mice, keyboards, joysticks and other game controllers.

### What are you doing to it?

The original MiSTer code is written in legacy C and C++.
It is hard to maintain, hard to build, and hard to contribute to.

1FPGA is written in easier-to-maintain (but still Open Source) Rust, with a TypeScript frontend for the user interface.
The design of the application has been made from top down to enable contributions and maintenance.
It is easier to read this code.

This is also an opportunity to improve the user experience greatly.

### How can I help?

Try it, get up to speed with the MiSTer project itself, and get ready to contribute when the time comes.

## Development

### Structure

The Rust code is in the `src/` directory, organized as a Cargo workspace with 13 crates.
The main entry point is `src/firmware/`, which handles CLI parsing, logging, and launches the JavaScript scripting engine.

The JavaScript/TypeScript code is in the `js/` directory, organized as NPM workspaces.
The main frontend (`js/frontend/`) is compiled via Rollup and bundled into the firmware.

Other directories:

- `docker/` contains the Dockerfile for ARM cross-compilation.
- `docs/` contains internal documentation (memory model, OSD protocol, startup sequence).
- `scripts/` contains utility scripts.

### Prerequisites

You'll need the following installed:

- The Rust toolchain. The easiest way to do this is to use `rustup`.
  Instructions can be found [here](https://rustup.rs/).
  The repo includes a `rust-toolchain.toml` that will install the right version automatically.
- [Node.js](https://nodejs.org/) and npm, for building the TypeScript frontend.
- [Docker](https://www.docker.com), for cross-compiling the ARM binary.
- `cmake`, needed by some native Rust dependencies.

### Building for DE10-Nano

The easiest way to build everything is with Make:

```bash
make build
```

This will build the TypeScript frontend first, then cross-compile the Rust firmware for ARM inside a Docker container.
If everything goes well, this will output the executable in `./target/armv7-unknown-linux-gnueabihf/release/one_fpga`.

You can also build the pieces separately:

```bash
make build-frontend   # Build just the TypeScript frontend
make build-1fpga      # Build just the ARM binary (requires frontend to be built first)
make build-and-sign   # Build and sign the binary with an RSA key
```

Simply copy the binary to your device and execute it.
The following commands can help:

```bash
ssh root@$MISTER_IP 'killall MiSTer one_fpga' # Make sure MiSTer (and 1FPGA) is not running
scp ./target/armv7-unknown-linux-gnueabihf/release/one_fpga root@$MISTER_IP:/media/fat/one_fpga # Copy the binary to the device
ssh root@$MISTER_IP 'sync; /media/fat/one_fpga' # Restart the firmware
```

Running `one_fpga --help` will show you the available CLI options.

### Desktop Executable

There is a Desktop version of this (that does not support Cores), which can be ran locally in debug mode with:

```bash
cargo run --bin one_fpga
```

This version should help develop some features that don't require an FPGA (like menus and configs).

### Tests

Rust tests can be run with `cargo test` as you would.
JavaScript tests can be run with `npm test`.

# Contributing

This repo is not the main fork of the MiSTer firmware.
If you want to contribute to MiSTer itself, please go to the [MiSTer repo](https://github.com/MiSTer-devel/Main_MiSTer/).

You can help a lot by testing this firmware and report bugs.

To contribute, please fork this repo, make your changes, and submit a PR.
Most PRs should be approved right away, but some may require discussion.

Make sure you follow the [Rust Code of Conduct](https://www.rust-lang.org/policies/code-of-conduct) when contributing.
We use the Rust CoC currently because it is the most complete and well thought out CoC we could find.
We might fork it locally in the future.

Thank you for understanding.
