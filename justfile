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
