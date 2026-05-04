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
    scp cores/menu-core-fpga/output_files/menu_core.rbf root@{{mister_ip}}:/media/fat/menu_core.rbf

# Cross-compile the menu-core host probe binary for armv7 (musl, static)
# Uses a separate community image because the device's glibc is older
# than what the main firmware's bookworm-based image links against.
build-menu-core-host mode="release-dev":
    docker run --rm -t \
        -v "{{justfile_directory()}}":/home/rust/src \
        messense/rust-musl-cross:armv7-musleabihf \
        cargo build --target armv7-unknown-linux-musleabihf --bin one_fpga_menu_core --profile {{mode}} --no-default-features --features=platform_de10

# Deploy the host probe binary to the device
deploy-menu-core-host mode="release-dev": (build-menu-core-host mode)
    scp target/armv7-unknown-linux-musleabihf/{{mode}}/one_fpga_menu_core root@{{mister_ip}}:/media/fat/one_fpga_menu_core

# Run the probe on the device (assumes the menu-core .rbf is loaded and the binary is deployed)
probe-menu-core:
    ssh root@{{mister_ip}} '/media/fat/one_fpga_menu_core probe'

# Run the M2b ring round-trip test on the device
ring-test-menu-core:
    ssh root@{{mister_ip}} '/media/fat/one_fpga_menu_core ring-test'

# Run the M2c1 FILL_RECT visual test on the device (look at HDMI)
draw-test-menu-core:
    ssh root@{{mister_ip}} '/media/fat/one_fpga_menu_core draw-test'

# Run the M2c3.1 COPY_RECT visual test on the device (look at HDMI)
texture-test-menu-core:
    ssh root@{{mister_ip}} '/media/fat/one_fpga_menu_core texture-test'

# Run the M2c3.2 A8 + tint visual test on the device (look at HDMI)
a8-test-menu-core:
    ssh root@{{mister_ip}} '/media/fat/one_fpga_menu_core a8-test'

# Run the M2c3.3 SrcAlpha blend visual test on the device (look at HDMI)
blend-test-menu-core:
    ssh root@{{mister_ip}} '/media/fat/one_fpga_menu_core blend-test'

# Run the TTF text-rendering visual test on the device (look at HDMI)
text-test-menu-core:
    ssh root@{{mister_ip}} '/media/fat/one_fpga_menu_core text-test'
