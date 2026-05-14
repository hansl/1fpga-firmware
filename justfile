set dotenv-load := true

mister_ip := env_var_or_default("MISTER_IP", "192.168.1.79")

# List available recipes
default:
    @just --list

# Build frontend and ARM binary (mode: release or release-dev)
build mode="release-dev": build-frontend (build-1fpga mode)

# Build the JS frontend
build-frontend:
    npm run build

# (Re-)build the Docker builder image when the Dockerfile changes
docker-image:
    docker build -f ./docker/armv7/de10nano.Dockerfile . -t 1fpga:armv7

# Build the cross-compile image only if it isn't already present.
_ensure-docker-image:
    @docker image inspect 1fpga:armv7 > /dev/null 2>&1 || just docker-image

# Build the ARM binary via Docker
build-1fpga mode="release-dev": _ensure-docker-image
    docker run -it -e "TERM=xterm-256color" -v "{{justfile_directory()}}":/app 1fpga:armv7 --bin one_fpga_bin --profile {{mode}}
    cp target/armv7-unknown-linux-gnueabihf/{{mode}}/one_fpga_bin target/armv7-unknown-linux-gnueabihf/{{mode}}/one_fpga

# Build and sign the binary
build-and-sign mode="release" public_key="": (build mode)
    #!/usr/bin/env bash
    set -euo pipefail
    key="{{public_key}}"
    if [ -z "$key" ]; then
        read -p "Enter path to public key: " key
    fi
    openssl pkeyutl -sign \
        -inkey "$key" \
        -out target/armv7-unknown-linux-gnueabihf/{{mode}}/one_fpga.sig \
        -rawin -in target/armv7-unknown-linux-gnueabihf/{{mode}}/one_fpga

# Deploy the frontend to the device
deploy-frontend: build-frontend
    rsync -raH --delete js/frontend/dist/ root@{{mister_ip}}:/root/frontend

# Create a new DB migration
new-migration name:
    #!/usr/bin/env bash
    set -euo pipefail
    ts=$(date +%Y-%m-%d-%H%M%S)
    dir="js/frontend/migrations/1fpga/${ts}_{{name}}"
    mkdir "$dir"
    echo "-- Add your migration here. Comments will be removed." > "$dir/up.sql"

# Update Patreon credits
patreon:
    LAST_RELEASE=$(git tag | sort -r | head -n1) npm run patreon

# Pull pre-built Quartus 17.0.2 image (theypsilon/quartus-lite-c5)
menu-core-image:
    docker build --platform linux/amd64 -t one-fpga-quartus:17.0.2 docker/quartus

# Build the Quartus image only if it isn't already present.
_ensure-menu-core-image:
    @docker image inspect one-fpga-quartus:17.0.2 > /dev/null 2>&1 || just menu-core-image

# Compile the menu-core FPGA bitstream (requires cores/menu-core-fpga submodule)
build-menu-core: _ensure-menu-core-image
    @test -f cores/menu-core-fpga/menu_core.qpf || \
        (echo "ERROR: cores/menu-core-fpga submodule not initialized. Run:" && \
         echo "  git submodule update --init --recursive" && \
         exit 1)
    docker run --rm -t \
        --platform linux/amd64 \
        -u "$(id -u):$(id -g)" \
        -v "{{justfile_directory()}}/cores/menu-core-fpga":/work \
        one-fpga-quartus:17.0.2 \
        --flow compile menu_core.qpf

# Open an interactive shell in the Quartus container
quartus-shell:
    docker run --rm -it \
        --platform linux/amd64 \
        -u "$(id -u):$(id -g)" \
        -v "{{justfile_directory()}}/cores/menu-core-fpga":/work \
        --entrypoint /bin/bash \
        one-fpga-quartus:17.0.2

# Deploy the built menu-core .rbf to the device
deploy-menu-core: build-menu-core
    scp cores/menu-core-fpga/output_files/menu_core.rbf root@{{mister_ip}}:/media/fat/menu.rbf

# Cross-compile the menu-core host probe binary for armv7 (musl, static)
# Uses a separate community image because the device's glibc is older
# than what the main firmware's bookworm-based image links against.
build-menu-core-host mode="release-dev":
    docker run --rm -t \
        -e RUSTUP_AUTO_INSTALL=0 \
        -v "{{justfile_directory()}}":/home/rust/src \
        messense/rust-musl-cross:armv7-musleabihf \
        cargo build --target armv7-unknown-linux-musleabihf --bin one_fpga_menu_core --profile {{mode}} --no-default-features --features=platform_de10

# Deploy the host probe binary to the device
deploy-menu-core-host mode="release-dev": (build-menu-core-host mode) _kill-fpga-users
    scp target/armv7-unknown-linux-musleabihf/{{mode}}/one_fpga_menu_core root@{{mister_ip}}:/media/fat/one_fpga_menu_core

# Cross-compile the menu-core demo binary (bouncing-rect animation)
build-menu-demo mode="release-dev":
    docker run --rm -t \
        -v "{{justfile_directory()}}":/home/rust/src \
        messense/rust-musl-cross:armv7-musleabihf \
        cargo build --target armv7-unknown-linux-musleabihf --bin menu_demo --profile {{mode}} --no-default-features --features=platform_de10

# Deploy the menu-core demo binary to the device
deploy-menu-demo mode="release-dev": (build-menu-demo mode) _kill-fpga-users
    scp target/armv7-unknown-linux-musleabihf/{{mode}}/menu_demo root@{{mister_ip}}:/media/fat/menu_demo

# Run the menu-core demo on the device (animation loop; Ctrl+C to stop)
demo-menu-core: _kill-fpga-users
    ssh -t root@{{mister_ip}} '/media/fat/menu_demo'

# Cross-compile the menu-ui launcher (React-on-Boa UI framework)
build-menu-ui mode="release-dev":
    docker run --rm -t \
        -v "{{justfile_directory()}}":/home/rust/src \
        messense/rust-musl-cross:armv7-musleabihf \
        cargo build --target armv7-unknown-linux-musleabihf --bin menu_ui --profile {{mode}} --no-default-features --features=platform_de10

# Kill anything that might be holding the FPGA registers (MiSTer
# auto-starts after a reboot, the previous menu_ui run may still be
# attached, etc.). Best-effort.
_kill-fpga-users:
    -ssh root@{{mister_ip}} 'killall MiSTer 2>/dev/null; killall menu_ui 2>/dev/null; killall one_fpga_menu_core 2>/dev/null; killall menu_demo 2>/dev/null; true'

# Deploy the menu-ui binary to the device
deploy-menu-ui mode="release-dev": (build-menu-ui mode) _kill-fpga-users
    scp target/armv7-unknown-linux-musleabihf/{{mode}}/menu_ui root@{{mister_ip}}:/media/fat/menu_ui

# Deploy the menu-ui JS bundle to the device (used with --bundle for dev iteration)
deploy-menu-ui-bundle:
    # Root-level build runs `@1fpga/schemas` then `@1fpga/frontend` in the
    # right order; the latter imports the former, so building only frontend
    # would fail with "Cannot find module '@1fpga/schemas'".
    npm run build
    scp js/frontend/dist/menu_ui.js root@{{mister_ip}}:/media/fat/menu_ui_app.js

# Deploy a small test PNG to the device for menu-ui's <img> demo
deploy-menu-ui-test-png:
    scp docs/assets/osd/line_array.png root@{{mister_ip}}:/media/fat/menu_ui_test.png

# Run the menu-ui launcher on the device, loading the deployed JS bundle
run-menu-ui: _kill-fpga-users
    ssh -t root@{{mister_ip}} '/media/fat/menu_ui --bundle /media/fat/menu_ui_app.js'

# Build everything menu-ui needs and deploy + run in one shot
demo-menu-ui: deploy-menu-ui deploy-menu-ui-bundle run-menu-ui

# Run the probe on the device (assumes the menu-core .rbf is loaded and the binary is deployed)
probe-menu-core: _kill-fpga-users
    ssh root@{{mister_ip}} '/media/fat/one_fpga_menu_core probe'

# Run the M2b ring round-trip test on the device
ring-test-menu-core: _kill-fpga-users
    ssh root@{{mister_ip}} '/media/fat/one_fpga_menu_core ring-test'

# Run the M2c1 FILL_RECT visual test on the device (look at HDMI)
draw-test-menu-core: _kill-fpga-users
    ssh root@{{mister_ip}} '/media/fat/one_fpga_menu_core draw-test'

# Run the M2c3.1 COPY_RECT visual test on the device (look at HDMI)
texture-test-menu-core: _kill-fpga-users
    ssh root@{{mister_ip}} '/media/fat/one_fpga_menu_core texture-test'

# Run the M2c3.2 A8 + tint visual test on the device (look at HDMI)
a8-test-menu-core: _kill-fpga-users
    ssh root@{{mister_ip}} '/media/fat/one_fpga_menu_core a8-test'

# Run the M2c3.3 SrcAlpha blend visual test on the device (look at HDMI)
blend-test-menu-core: _kill-fpga-users
    ssh root@{{mister_ip}} '/media/fat/one_fpga_menu_core blend-test'

# Run the TTF text-rendering visual test on the device (look at HDMI)
text-test-menu-core: _kill-fpga-users
    ssh root@{{mister_ip}} '/media/fat/one_fpga_menu_core text-test'

# Run the TTF text animation perf test on the device (look at HDMI; reports FPS)
text-anim-menu-core: _kill-fpga-users
    ssh root@{{mister_ip}} '/media/fat/one_fpga_menu_core text-anim'

# Run the M2c2 SET_CLIP / CLEAR_CLIP visual test on the device (look at HDMI)
clip-test-menu-core: _kill-fpga-users
    ssh root@{{mister_ip}} '/media/fat/one_fpga_menu_core clip-test'

# Run the N4.5 SET_RENDER_TARGET visual test on the device (look at HDMI)
rtt-test-menu-core: _kill-fpga-users
    ssh root@{{mister_ip}} '/media/fat/one_fpga_menu_core rtt-test'
