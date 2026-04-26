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

# Build the ARM binary via Docker
build-1fpga mode="release-dev":
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

# Compile the menu-core FPGA bitstream (requires cores/menu-core-fpga/ checkout)
build-menu-core:
    @test -f cores/menu-core-fpga/menu_core.qpf || \
        (echo "ERROR: cores/menu-core-fpga/ not found. Clone the FPGA repo first:" && \
         echo "  git clone git@github.com:one-retro/1fpga-menu-core.git cores/menu-core-fpga" && \
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
