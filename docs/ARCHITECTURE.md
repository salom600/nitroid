# Architecture

This document describes the internal design of Nitroid, the rationale behind each major decision, and the boundaries between crates.

## Design principles

1. **No reimplementation of the host's hypervisor.** We use KVM on Linux and WHPX on Windows — the same APIs QEMU uses. Building our own hypervisor would be a multi-year effort with no upside.
2. **No AOSP compilation.** Compiling Android from source takes hours and produces gigabytes of artifacts that don't belong in an "ultra-light" emulator. We accept pre-built system images (Android-x86, Bliss OS, etc.) as inputs.
3. **Read-only blueprint + per-instance overlay.** One image can back N instances. Each instance has a small (initially zero) overlay that records only the blocks it has written.
4. **Single binary, single GPU API.** WGPU gives us one code path that works on Vulkan (Linux), DX12 (Windows), and Metal (macOS, future). The UI uses egui's `glow` backend — no system WebView dependency.
5. **Pluggable translation.** ARM→x86 translation is hard. Rather than build it ourselves, we expose a `Translator` trait that any compatible backend can implement (Houdini, libhoudini64, future Rosetta-like bridges).

## Crate layout

```
crates/
├── core/             — shared types: config, errors, instance, image, paths
├── virtualization/   — KVM (Linux) + WHPX (Windows) backend trait + impls
├── graphics/          — WGPU surface, swapchain, guest texture
├── translation/       — ARM→x86 binary translation trait + cache
├── input/             — keymapping engine, profiles, translator
├── instances/         — multi-instance manager with overlay tracking
├── ui/                — egui control panel
└── bin/               — the `nitroid` binary (CLI + GUI launcher)
```

Dependencies flow strictly downward: `bin` depends on everything; `ui` depends on `instances`, `input`, `core`; no crate depends on `bin`. This keeps the graph acyclic and lets us reuse the lower layers (e.g. for headless testing on CI).

## Virtualization

### Why KVM/WHPX?

KVM and WHPX are the two user-mode-accessible hypervisors that ship with Linux and Windows respectively. Both let a userspace process create a VM, allocate guest memory, create vCPUs, and run them — without requiring a kernel module or admin privileges (assuming the user is in the `kvm` group on Linux, or has WHPX enabled on Windows).

The alternative — building a CPU simulator from scratch — would be 10-100x slower and 10,000+ lines of code for an x86_64 core alone. We don't do it.

### Why not QEMU directly?

QEMU is excellent but it's:
- Written in C — we lose Rust's safety guarantees
- ~5 million lines of code — too much to embed
- Designed as a generic emulator — we don't need 99% of the device models

Instead we use the same KVM/WHPX APIs that QEMU uses, wrapped in safe Rust.

### The `Backend` trait

```rust
pub trait Backend: Send + Sync {
    fn info(&self) -> BackendInfo;
    fn capabilities(&self) -> Result<BackendCapabilities>;
    fn create_vm(&self, cfg: &InstanceConfig) -> Result<VmHandle>;
    fn start(&self, vm: &mut VmHandle) -> Result<()>;
    fn pause(&self, vm: &mut VmHandle) -> Result<()>;
    fn resume(&self, vm: &mut VmHandle) -> Result<()>;
    fn stop(&self, vm: &mut VmHandle) -> Result<()>;
    fn inject_input(&self, vm: &mut VmHandle, event: InputEvent) -> Result<()>;
}
```

The upper layers (`ui`, `input`, `bin`) never reference `KvmBackend` or `WhpxBackend` directly — they hold `Box<dyn Backend>`. This means the same UI code works on both platforms without `#[cfg]` litter.

## Graphics

### Why WGPU?

WGPU is the Rust implementation of the WebGPU spec. It gives us:
- One API across Vulkan/DX12/Metal
- Mature, used in production by Firefox, Bevy, Ruffle, Deno
- Reasonable defaults for resource management

### What WGPU doesn't do

WGPU doesn't translate Vulkan or OpenGL ES calls — it's just a host-side rendering API. The guest's graphics commands (Vulkan/OpenGL ES, sent via virtio-gpu) need to be translated into WGPU commands by a separate translation layer. That's the responsibility of the `translation` crate's future `GpuTranslator` trait.

For the scaffold, we render a placeholder gradient so the window is non-empty during testing.

## Binary translation

### The problem

Android apps can be built for ARM (armv7, arm64) or x86 (x86, x86_64). Most Android apps are ARM-only because that's what ships on real phones. To run them on an x86_64 host, we need to translate ARM machine code into x86_64 machine code at load time.

### Why we don't build it

A correct ARMv8.x → x86_64 translator needs to handle:
- Conditional flags, flagless arithmetic, FPCR trapping
- SVE/SVE2 vector operations (which have no direct x86 equivalent)
- NEON → AVX/AVX2/AVX-512 mapping with proper NaN boxing
- ASID/TLB management, atomic ordering semantics
- Exclusive monitor / LDXR/STXR sequence correctness

Google's own translator took years. Apple's Rosetta 2 took years. We integrate, we don't rebuild.

### The `Translator` trait

```rust
pub trait Translator: Send + Sync {
    fn translate(&self, addr: u64, bytes: &[u8]) -> Result<TranslatedBlock>;
}
```

Backends:
- `NativeBackend` — no translation; the guest is already x86_64. Just returns the bytes.
- `HoudiniBridge` — wraps the libhoudini library shipped inside Android-x86 images. Loaded via `dlopen` when the image is mounted.
- `Unavailable` — used when no compatible backend is found. The instance can boot but ARM apps will fail with a friendly error.

### Translation cache

The `TranslationCache` persists translated basic blocks across VM restarts. Keys are guest program counters; values are the translated host bytes. The cache is in-memory with optional disk persistence (loaded lazily on first miss).

## Input

The input engine is **stateless across frames**. Each host event is processed independently:

```
HostEvent → Keymap lookup → OutputAction(s) → virtio-input inject
```

This makes the engine:
- Trivially testable (no per-frame accumulators)
- Predictable latency (<100 µs per event)
- Easy to replay from a recording (for benchmarking)

### Keymap features

- **Tap** — touch down on press, release on release
- **Hold** — touch down on press, hold while key is down, release on release
- **Toggle** — touch down on first press, release on second press
- **Swipe** — touch down → sequence of moves → touch up, with configurable duration
- **Macro** — sequence of taps with delays

### Latency target

End-to-end input latency (host event → guest touch handler) target is ≤4 ms on a 60 Hz guest. The engine itself adds <100 µs; the rest of the budget is consumed by virtio-input dispatch and the guest kernel's input subsystem.

## Storage

### The blueprint model

```
~/.local/share/nitroid/
├── config.toml                    # global config
├── images.json                    # image registry (small JSON)
├── instances.json                 # instance registry (small JSON)
└── instances/
    ├── abc12345-dead.overlay.qcow2   # per-instance writable overlay
    └── ...

~/.cache/nitroid/
└── (downloaded system images, large)
```

A fresh instance's overlay is a few kilobytes. A heavily-used instance might accumulate 200-500 MB after months of play. The shared image (typically 800 MB-1.5 GB) is counted once regardless of how many instances use it.

### Overlay format

We use qcow2 — the same format QEMU uses. This means:
- Sparse: only written blocks consume disk space
- Compatible with `qemu-img` for offline manipulation
- Snapshot-friendly

## CI/CD

The pipeline runs on every push and every PR:

1. **lint** — `cargo fmt --check` + `cargo clippy -D warnings`
2. **build-linux** — Ubuntu 22.04, full system deps, KVM-capable
3. **build-windows** — Windows Server 2022, WHPX-capable
4. **release** — only on tags (`v*`), packages artifacts into GitHub release

The **auto-repair** workflow runs after a failure. It:
1. Fetches the failed job's logs via the GitHub API
2. Greps for trivial patterns (formatting, missing `#[allow]`)
3. If a trivial fix is possible, applies it and pushes to the same branch
4. Otherwise, opens a structured issue with the logs and reproduction steps

This is intentionally conservative — we don't attempt to fix logic errors automatically.

## Future work

- **Boot protocol** — load kernel + initrd, set up the boot command line, wire virtio devices. This is the next major milestone.
- **Virtio device layer** — virtio-input (for the keymapping engine), virtio-gpu (for graphics), virtio-net, virtio-blk.
- **Multi-vCPU scheduling** — currently we create one vCPU; multi-vCPU requires careful handling of the KVM run loop.
- **Snapshot/resume** — save a running instance to disk and resume it later, skipping the boot sequence.
- **macOS support** — currently Linux + Windows only. macOS would use the Hypervisor.framework (`vmnet`, `Hypervisor`) and Metal via WGPU.
