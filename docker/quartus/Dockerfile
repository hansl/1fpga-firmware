# ---------------------------------------------------------------------------
# Quartus Prime Lite 17.0.2 headless build environment for MiSTer cores.
#
# Uses theypsilon/quartus-lite-c5:17.0.2 — a community-maintained image
# pre-loaded with Quartus 17.0.2 and the Cyclone V device pack. We
# attempted to install Quartus ourselves from Intel's tarball, but
# Intel's 17.0.2 installer has a reproducible deadlock in unattended
# mode (confirmed on macOS+QEMU and on native amd64 Linux; the update
# installer's select-thread waits indefinitely on I/O that never
# arrives). The community image sidesteps this entirely.
#
# Upstream:
#   https://github.com/theypsilon/quartus-lite-c5-dockerized
#   https://hub.docker.com/r/theypsilon/quartus-lite-c5
#
# Build:
#   docker build -t one-fpga-quartus:17.0.2 docker/quartus
#
# Or via the top-level justfile:
#   just menu-core-image
#
# Platform: linux/amd64 is forced by the justfile recipe because Quartus
# is x86-only. On Apple Silicon, Docker Desktop's QEMU emulation is
# known to hang the Quartus compile tools; build on a real amd64 host
# (native Linux, a Steam Deck, or a cloud VM).
# ---------------------------------------------------------------------------

FROM theypsilon/quartus-lite-c5:17.0.2

# Default working directory inside the container; `just build-menu-core`
# mounts cores/menu-core/ here.
WORKDIR /work

# The upstream image already has quartus_sh in PATH.
ENTRYPOINT ["quartus_sh"]
CMD ["--version"]
