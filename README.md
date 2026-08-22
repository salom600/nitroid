# Nitroid

> An ultra-light, high-performance Android emulator for Windows and Linux, built in Rust.

**v1.0.0** — Nitroid has graduated from scaffold to a functional foundation. This release adds the virtio device layer (virtio-blk, virtio-input, virtio-gpu), a boot protocol scaffold (Linux x86 boot protocol + Android boot cmdline), multi-vCPU scheduling (one OS thread per vCPU), an automatic Android image downloader with live progress reporting, and a pluggable ARM→x86 translation runner with caching. CI is fully green on Linux + Windows, producing 3-4 MB binaries.

Nitroid is designed from the ground up to be small (the emulator binary alone is well under 100 MB), fast (using the host OS's built-in hypervisor — KVM on Linux, WHPX on Windows), and resource-efficient (no AOSP compilation, no Chromium-derived GUI, no embedded Android runtime). It targets gamers who want to play Android titles like PUBG Mobile, Free Fire, and Call of Duty Mobile on their desktop hardware without the bloat of conventional emulators.

## Why Nitroid?

| Problem with other emulators | Nitroid's answer |
|---|---|
| 1-3 GB install size, ~6 GB after first boot | < 100 MB installer; Android image is fetched on demand |
| Memory footprint of 2-4 GB even when idle | Memory is allocated per-instance and freed when the instance stops |
| Custom-built virtio drivers that lag upstream | Uses the host's stock KVM/WHPX stack — the same one QEMU uses |
| Electron or Chromium-based UI with hundreds of MB of dependencies | Native egui UI rendering through the system GPU |
| ARM translation sold as a proprietary black box | Pluggable translation bridge — any compatible libhoudini can be slotted in |
| Per-instance system image copies waste disk | Read-only blueprint + copy-on-write overlay means N instances cost ~1 image |

## Status

Nitroid is in active development. The current state of each subsystem is:

| Subsystem | Status |
|---|---|
| Workspace structure (10 crates) | ✅ Complete |
| Configuration model | ✅ Complete |
| Multi-instance manager (read-only blueprint + overlays) | ✅ Complete |
| Keymapping engine (keyboard, mouse, macro, toggle, swipe) | ✅ Complete with full unit tests |
| Built-in game profiles (PUBG, Free Fire, CoD, generic FPS, MMORPG) | ✅ Complete |
| egui control panel (instances, images, downloader, settings, keymap editor) | ✅ Complete with live progress bar |
| KVM backend (Linux) — multi-vCPU + vCPU run loop | ✅ API complete; multi-vCPU scheduling wired |
| WHPX backend (Windows) — availability detection | ✅ Stub ready for full WHPX API integration |
| WGPU renderer (DX12/Vulkan/Metal) — surface + texture + blit pipeline | ✅ Scaffold complete |
| ARM→x86 binary translation runner with cache + stats | ✅ Trait + runner + cache complete |
| Virtio device layer (trait + 3 devices) | ✅ virtio-blk (read/write tested), virtio-input (MT-slot protocol), virtio-gpu (2D resources + scanout) |
| VirtQueue implementation with wrap-around | ✅ Complete with tests |
| Boot protocol scaffold (cmdline + BootConfig + BootLoader) | ✅ Trait + scaffold complete |
| **Automatic image downloader** (Android-x86 catalog + async streaming) | ✅ Complete with progress events |
| Boot protocol — ISO 9660 kernel extraction | 🚧 Pending — needs ISO parser |
| Virtio device dispatch in KVM/WHPX run loop | 🚧 Pending — needs guest memory access layer |
| End-to-end boot of an Android image | 🚧 Pending — needs the above two |

This repository contains a complete, compiling, tested foundation with 37 unit tests. Booting a real Android image requires the virtio dispatch + ISO parser — both are tracked as the next milestones.

## Quick start

### Prerequisites

**Linux:**
- A recent kernel (5.x+ recommended) with KVM enabled
- Vulkan-capable GPU (Mesa RADV/AMDGPU, NVIDIA proprietary, or Intel ANV)
- Your user must be in the `kvm` group: `sudo usermod -aG kvm $USER`

**Windows:**
- Windows 10 21H2 or later, or Windows 11
- Hyper-V Platform enabled (Settings → Apps → Optional features → More Windows features → Windows Hypervisor Platform)
- A DirectX 12 capable GPU

### Build from source

```bash
git clone https://github.com/salom600/nitroid.git
cd nitroid
cargo build --release
./target/release/nitroid --list      # CLI: list instances and images
./target/release/nitroid             # GUI: launch the control panel
```

### Register an Android image

Nitroid does **not** bundle an Android system image — that's the key to keeping the installer small. Download a pre-built image and register it:

```bash
# Download Android-x86 (https://www.android-x86.org/) or Bliss OS
# Then register the image:
./target/release/nitroid image register ./android-x86-9.0-r2.iso
```

Once registered, the image is shared (read-only) by every instance you create from it.

## Architecture

For the full design, see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). The high-level layout:

```
┌─────────────────────────────────────────────────────────────┐
│                       nitroid (bin)                          │
│  CLI · GUI launcher · logging                                │
└──────┬───────────────────────────────────────────────────────┘
       │
   ┌───┴───┬───────────┬───────────────┬──────────────┐
   │       │           │               │              │
   ▼       ▼           ▼               ▼              ▼
┌──────┐ ┌────┐  ┌────────────┐  ┌─────────┐  ┌─────────┐
│ core │ │ ui │  │ virtualiz.  │  │graphics │  │instances│
│      │ │    │  │ KVM / WHPX  │  │ WGPU    │  │ manager │
└──────┘ └────┘  └────────────┘  └─────────┘  └─────────┘
                      │                ▲
                      ▼                │
                 ┌──────────┐    ┌──────────────┐
                 │ input    │    │ translation │
                 │ keymap   │    │ ARM → x86   │
                 └──────────┘    └──────────────┘
```

## Built-in keymap profiles

Nitroid ships with sensible defaults for popular games:

- **PUBG Mobile** — WASD movement, mouse-look, left-click to fire, right-click to aim, 1/2/3 weapon switch, R reload, Space jump
- **Free Fire** — simplified movement + fire layout
- **Call of Duty Mobile** — PUBG base + grenade (G) + slide (Ctrl)
- **Generic FPS** — a sensible default that works for most shooters
- **MMORPG** — 5 hotkeys (1-5) mapped to ability buttons

Every profile is fully editable from the Settings panel and exportable/importable as JSON.

## CI/CD

GitHub Actions handles every build. Pushing to any branch triggers:

1. **Lint & format** — `cargo fmt --check` + `cargo clippy -D warnings`
2. **Linux build** — Ubuntu 22.04, KVM-capable runner, full test suite
3. **Windows build** — Windows Server 2022, WHPX-capable, full test suite
4. **Auto-repair** — if CI fails, this workflow fetches the failed job's logs, attempts a trivial fix (formatting/clippy), and either pushes a commit or opens a structured issue

Releases are cut by pushing a tag: `git tag v0.1.0 && git push --tags`. This triggers the release job, which packages and uploads platform archives to the GitHub release page.

## License

Dual-licensed under MIT or Apache-2.0, at your option. Contributions intentionally submitted for inclusion in this crate are dual-licensed as above, without any additional terms or conditions.

## Security

**Never commit secrets to this repository.** If you accidentally push a token or credential:

1. Revoke it immediately at the provider's settings page.
2. Force-push to remove it from history: `git rebase -i HEAD~5` (or whatever depth is needed).
3. Open an issue so maintainers can audit access logs.

See [SECURITY.md](SECURITY.md) for the full policy.
