# Linux build

CodeNotch builds for Linux (X11) in Docker, on Ubuntu 22.04, the base
Pop!_OS 22.04 shares. From the repository root, with Docker running:

```sh
docker build -t codenotch-linux packaging/linux
```

Every script below reads the checkout from `/host` (read-only) and works on a
copy of it. The named volumes keep Cargo's and npm's caches between runs.

| Script     | What it does                                                        | Extra mounts          |
| ---------- | ------------------------------------------------------------------- | --------------------- |
| `build.sh` | Builds the `.deb` and copies it to `/out`.                          | `/out`, caches        |
| `try.sh`   | Installs that `.deb` on a virtual screen and checks the notch: window manager hints, where it sits, which clicks it takes, screenshots. | `/out`                |
| `probe.sh` | Runs the X11 layer's live test against two xterms.                  | caches                |
| `holds-its-place.sh` | Shoves the notch's window aside the way a window manager does, and checks that it goes back. | `/out` |
| `check.sh` | Clippy and the unit tests, on Linux.                                | caches                |

```sh
docker run --rm -v "$PWD:/host:ro" -v "$PWD/src-tauri/target/linux:/out" \
  -v cn-target:/build/src-tauri/target -v cn-node:/build/node_modules \
  -v cn-cargo:/root/.cargo/registry \
  codenotch-linux bash /host/packaging/linux/build.sh
```

From a Windows checkout, whose line endings are CRLF, feed the script through
`tr -d '\r'` instead: `bash -c "tr -d '\r' < /host/packaging/linux/build.sh | bash"`.

The virtual screen has no graphics card, so it shows how the notch behaves,
not how it performs. WebKitGTK is left on its DMA-BUF renderer, passing frames
through shared memory (`WEBKIT_DMABUF_RENDERER_FORCE_SHM`, set in `lib.rs`):
the older renderer, the usual workaround for blank windows on NVIDIA, never
clears a transparent window, and a closed card stays on screen.
