# ---------------------------------------------------------------------------
# Quartus Prime Lite 17.0.2 headless build environment for MiSTer cores.
#
# Base: Ubuntu 20.04 (glibc 2.31). Quartus 17.0.2 was built for the
# Ubuntu 16.04/18.04 era but glibc is forward-compatible, and 20.04
# ships a ca-certificates bundle recent enough to validate Intel's
# current CDN TLS chain. 18.04 would be closer to Quartus's origin but
# its cert bundle cannot verify downloads.intel.com's current chain.
#
# Install: setup.sh supports `--mode unattended` (confirmed by running
# `setup.sh --help` under QEMU on 2026-04-24 — no prompts, no expect,
# no tty). Much more reliable than driving the interactive installer.
#
# Build:
#   docker build -t one-fpga-quartus:17.0.2 docker/quartus
#
# Or via the top-level justfile:
#   just menu-core-image
#
# Platform: linux/amd64 is forced by the justfile recipe because Quartus
# is x86-only. Apple Silicon users: Rosetta 2 triggers
# `rosetta error: bss_size overflow` on setup.sh, so Docker Desktop must
# be set to use QEMU (uncheck "Use Rosetta for x86_64/amd64 emulation
# on Apple Silicon"). Expect a ~1.5–2 hour build under QEMU vs ~20 min
# on native amd64.
#
# Sizes:
#   - Tarball download:       ~8.8 GB  (cached as a build layer)
#   - After install:          ~7   GB
#   - After pruning:          ~4   GB
#   - Final runtime image:    ~4   GB
# ---------------------------------------------------------------------------

FROM ubuntu:20.04 AS install

ARG DEBIAN_FRONTEND=noninteractive
ARG QUARTUS_VERSION=17.0
ARG QUARTUS_TAR_URL=https://downloads.intel.com/akdlm/software/acdsinst/17.0std.2/602/ib_tar/Quartus-lite-17.0.2.602-linux.tar

# 32-bit runtime libraries required by the 17.0 installer and by several
# Quartus binaries which are still built for i386.
RUN dpkg --add-architecture i386 \
 && apt-get update \
 && apt-get install --no-install-recommends -y \
      ca-certificates curl expect locales \
      libc6:i386 libncurses5:i386 libstdc++6:i386 \
      libgl1-mesa-glx libsm6 libxext6 libxft2 libxrender1 libxtst6 \
 && locale-gen en_US.UTF-8 \
 && rm -rf /var/lib/apt/lists/*

ENV LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8

WORKDIR /tmp/quartus-tar
# Intel's CDN (Akamai) occasionally resets connections mid-transfer on
# large files like this 8.8 GB tarball. curl 7.68.0 on Ubuntu 20.04
# predates `--retry-all-errors` (added in 7.71.0), so we drive the
# retries from the shell. Each attempt resumes from the partial file
# via `-C -`, so a single build can absorb multiple resets without
# re-downloading the whole thing.
RUN set -eu; \
    for attempt in 1 2 3 4 5 6 7 8 9 10; do \
      echo "=== Download attempt $attempt ==="; \
      if curl -fSL -C - --connect-timeout 30 -o installer.tar "$QUARTUS_TAR_URL"; then \
        echo "=== Download succeeded on attempt $attempt ==="; \
        break; \
      fi; \
      if [ "$attempt" = "10" ]; then \
        echo "=== All 10 download attempts exhausted ==="; \
        exit 1; \
      fi; \
      echo "=== Attempt $attempt failed; sleeping 15s before retrying ==="; \
      sleep 15; \
    done; \
    tar xf installer.tar; \
    rm installer.tar

# Unattended install. The disable-components list selects Cyclone V as
# the only device family (skips Arria II, Cyclone III/IV, Cyclone 10 LP,
# MAX II/V, MAX 10) and skips ModelSim entirely. Keeps: quartus,
# quartus_update (gets us to 17.0.2), devinfo, cyclonev.
RUN /tmp/quartus-tar/setup.sh \
      --mode unattended \
      --unattendedmodeui none \
      --installdir /opt/intelFPGA_lite/${QUARTUS_VERSION} \
      --disable-components quartus_help,arria_lite,cyclone,cyclone10lp,max,max10,modelsim_ase,modelsim_ae \
 && rm -rf /tmp/quartus-tar

# Shrink the image: drop documentation, examples, and unused device
# families (Cyclone V is the only one the DE10-Nano target needs).
RUN find /opt/intelFPGA_lite -type d -name 'help' -prune -exec rm -rf {} + \
 && find /opt/intelFPGA_lite -type d -name 'examples' -prune -exec rm -rf {} + \
 && find /opt/intelFPGA_lite -type d -name 'doc' -prune -exec rm -rf {} + \
 && find /opt/intelFPGA_lite/${QUARTUS_VERSION}/quartus/common/devinfo \
        -mindepth 1 -maxdepth 1 -type d ! -name cyclonev \
        -exec rm -rf {} +

# ---------------------------------------------------------------------------
# Runtime stage — minimal shared libraries Quartus actually loads.
# ---------------------------------------------------------------------------
FROM ubuntu:20.04

ARG DEBIAN_FRONTEND=noninteractive
ARG QUARTUS_VERSION=17.0

RUN dpkg --add-architecture i386 \
 && apt-get update \
 && apt-get install --no-install-recommends -y \
      ca-certificates locales \
      libc6:i386 libncurses5:i386 libstdc++6:i386 \
      libgl1-mesa-glx libsm6 libxext6 libxft2 libxrender1 libxtst6 \
 && locale-gen en_US.UTF-8 \
 && rm -rf /var/lib/apt/lists/*

ENV LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8

COPY --from=install /opt/intelFPGA_lite /opt/intelFPGA_lite

ENV PATH=/opt/intelFPGA_lite/17.0/quartus/bin:$PATH
ENV QSYS_ROOTDIR=/opt/intelFPGA_lite/17.0/quartus/sopc_builder/bin

WORKDIR /work
ENTRYPOINT ["/opt/intelFPGA_lite/17.0/quartus/bin/quartus_sh"]
CMD ["--version"]
