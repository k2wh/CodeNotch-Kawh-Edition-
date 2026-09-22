# Builds CodeNotch's Linux package on the base Pop!_OS 22.04 and Ubuntu 22.04
# share, so it links against the libraries those systems have.
#
#   docker build -t codenotch-linux packaging/linux
#   see build.sh and try.sh for what runs inside it
FROM ubuntu:22.04

ENV DEBIAN_FRONTEND=noninteractive

# Tauri's build requirements on Linux.
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential ca-certificates curl file pkg-config rsync xz-utils \
      libssl-dev libgtk-3-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev \
      librsvg2-dev libxdo-dev \
    && rm -rf /var/lib/apt/lists/*

# Node 22, which Vite 7 needs.
RUN curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && rm -rf /var/lib/apt/lists/*

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --profile minimal --component clippy --default-toolchain stable
ENV PATH=/root/.cargo/bin:$PATH

# For trying a build out (try.sh, probe.sh): a virtual screen, a window
# manager that speaks EWMH, a compositor so transparency is real, windows to
# find and raise, and tools to look.
RUN apt-get update && apt-get install -y --no-install-recommends \
      xvfb openbox picom x11-utils x11-apps xterm xdotool imagemagick dbus dbus-x11 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
