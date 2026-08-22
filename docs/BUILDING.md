# Building Nitroid

## Prerequisites

### Rust toolchain

Nitroid requires Rust **1.75 or later**. Install via [rustup](https://rustup.rs/):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup default stable
```

### Linux (Ubuntu / Debian)

```bash
sudo apt-get update
sudo apt-get install -y \
  build-essential \
  pkg-config \
  libssl-dev \
  libasound2-dev \
  libudev-dev \
  libwayland-dev \
  libxkbcommon-dev \
  libx11-xcb-dev \
  libgbm-dev \
  libvulkan-dev \
  mesa-vulkan-drivers \
  vulkan-tools
```

Add your user to the `kvm` group so the emulator can access `/dev/kvm`:

```bash
sudo usermod -aG kvm $USER
# Log out and back in for the group change to take effect.
```

### Linux (Fedora / RHEL)

```bash
sudo dnf install -y \
  gcc \
  pkgconf-pkg-config \
  openssl-devel \
  alsa-lib-devel \
  systemd-devel \
  wayland-devel \
  libxkbcommon-devel \
  libxcb-devel \
  mesa-vulkan-drivers \
  vulkan-tools
```

### Linux (Arch)

```bash
sudo pacman -S --needed \
  base-devel \
  pkgconf \
  openssl \
  alsa-lib \
  systemd-libs \
  wayland \
  libxkbcommon \
  libxcb \
  vulkan-driver \
  vulkan-tools
```

### Windows

Install [Visual Studio Build Tools 2022](https://visualstudio.microsoft.com/downloads/#build-tools-for-visual-studio-2022) with the "Desktop development with C++" workload.

Enable WHPX:
1. Open **Settings → Apps → Optional features → More Windows features**
2. Check **Windows Hypervisor Platform**
3. Check **Virtual Machine Platform** (optional, but recommended)
4. Restart

A DirectX 12 capable GPU is required for the WGPU renderer.

## Build

Clone and build:

```bash
git clone https://github.com/salom600/nitroid.git
cd nitroid
cargo build --release
```

The binary will be at `target/release/nitroid` (Linux) or `target/release/nitroid.exe` (Windows).

## Run

```bash
# Show the CLI help
./target/release/nitroid --help

# List registered instances and images
./target/release/nitroid --list

# Launch the GUI control panel
./target/release/nitroid
```

## Test

Run the full test suite:

```bash
cargo test --workspace
```

Run tests for a specific crate:

```bash
cargo test -p nitroid-input
cargo test -p nitroid-instances
```

## Lint

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
```

## Release profile

The release profile is tuned for performance:

```toml
[profile.release]
opt-level = 3
lto = "fat"           # full LTO across all crates
codegen-units = 1     # best optimisation at the cost of compile time
panic = "abort"       # smaller binary, no unwinding
strip = true          # strip debug symbols
```

This produces a binary that is typically 30-50% smaller than the default release profile and runs 5-15% faster.

## Cross-compilation

### From Linux to Windows

```bash
rustup target add x86_64-pc-windows-gnu
# Install mingw-w64
sudo apt-get install -y mingw-w64
cargo build --release --target x86_64-pc-windows-gnu
```

The CI uses native Windows runners for the official Windows build (better ABI compatibility with WHPX), but the GNU toolchain is fine for development.

### From Linux to aarch64 Linux (e.g. Raspberry Pi 5)

```bash
rustup target add aarch64-unknown-linux-gnu
sudo apt-get install -y gcc-aarch64-linux-gnu
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
cargo build --release --target aarch64-unknown-linux-gnu
```

## Troubleshooting

### `error: /dev/kvm not available`

Your user is not in the `kvm` group, or your CPU doesn't support virtualization. Check:

```bash
ls -la /dev/kvm
sudo modprobe kvm
sudo modprobe kvm_intel  # or kvm_amd
egrep -c '(vmx|svm)' /proc/cpuinfo   # should be > 0
```

If `vmx` or `svm` is not present, virtualization is disabled in your BIOS — enable it there.

### `error: failed to find a suitable GPU adapter`

Your GPU driver doesn't expose Vulkan. On Linux, install `mesa-vulkan-drivers` (AMD/Intel) or the NVIDIA proprietary driver. On Windows, ensure your GPU driver is up to date.

### `WHPX is not available` (Windows)

WHPX is not enabled. Open "Turn Windows features on or off" and enable:
- Windows Hypervisor Platform
- Virtual Machine Platform (optional)

Restart your machine.

### The control panel opens but is black

The WGPU placeholder shader renders a slowly-pulsing gradient. If you see a pure black window, your GPU adapter may not support the surface format we requested — try setting `graphics = "vulkan"` (Linux) or `graphics = "dx12"` (Windows) in `~/.config/nitroid/config.toml`.

## CI build artifacts

The latest CI build is always downloadable from the [Actions tab](../../actions) of the GitHub repo. Look for the most recent successful `Build (Linux)` or `Build (Windows)` job and download its artifact.

Official releases are published on the [Releases page](../../releases) when a tag is pushed.
