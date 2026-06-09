# Cross-compile image for the menu-core host/demo binaries (armv7 musl).
#
# The device's glibc is older than the firmware image's bookworm links
# against, so host binaries are built static against musl via the community
# messense/rust-musl-cross image. That image bakes the floating `stable`
# channel, which (a) drifts from the firmware build's rustc and (b) breaks
# at runtime when `stable` rolls past the image's baked version (rustup
# tries to self-update mid-build and aborts on a stale component).
#
# We derive from it and bake the EXACT toolchain pinned in
# rust-toolchain.toml as a *versioned* toolchain, so `RUSTUP_AUTO_INSTALL=0`
# builds find it without any network update and every build — firmware and
# host — uses the identical rustc.
#
# Keep the version in sync with rust-toolchain.toml + CLAUDE.md.
FROM messense/rust-musl-cross:armv7-musleabihf

ARG RUST_VERSION=1.95.0
RUN rustup toolchain install ${RUST_VERSION} --profile default \
 && rustup default ${RUST_VERSION} \
 && rustup target add armv7-unknown-linux-musleabihf --toolchain ${RUST_VERSION} \
 && rustup component add rustfmt clippy rust-std --toolchain ${RUST_VERSION}
