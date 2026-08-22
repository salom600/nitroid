//! Backend trait — the abstraction surface every hypervisor must implement.

use nitroid_core::InstanceConfig;
use nitroid_core::Result;

/// Static information about the backend (name, version, supported features).
#[derive(Debug, Clone)]
pub struct BackendInfo {
    /// Human-readable name (e.g. "KVM (Linux 6.x)").
    pub name: String,
    /// Backend version string (e.g. the kernel version, or WHPX API version).
    pub version: String,
    /// Whether nested virtualisation is enabled.
    pub nested: bool,
}

/// Per-backend capability flags.
#[derive(Debug, Clone, Default)]
pub struct BackendCapabilities {
    /// Maximum number of vCPUs per VM.
    pub max_vcpus: u32,
    /// Maximum memory (in megabytes) the backend can map.
    pub max_memory_mb: u64,
    /// Whether the backend supports ARM guest translation.
    pub supports_arm_translation: bool,
    /// Whether the backend exposes a virtio-gpu context for WGPU rendering.
    pub supports_virtio_gpu: bool,
}

/// Opaque handle to a created (but not yet started) VM. The implementation
/// owns the file descriptors / partition handles; the upper layers only hold
/// a trait object so they remain platform-agnostic.
pub struct VmHandle {
    inner: Box<dyn std::any::Any + Send + Sync>,
}

impl VmHandle {
    pub fn new(inner: impl std::any::Any + Send + Sync) -> Self {
        Self {
            inner: Box::new(inner),
        }
    }
    pub fn as_any(&self) -> &dyn std::any::Any {
        self.inner.as_ref()
    }
}

/// The hypervisor backend abstraction. Every method is synchronous — the
/// upper layers wrap blocking calls in `tokio::task::spawn_blocking` when
/// they need to run inside an async runtime.
pub trait Backend: Send + Sync {
    /// Static information about this backend.
    fn info(&self) -> BackendInfo;

    /// Capabilities reported by the host hypervisor.
    fn capabilities(&self) -> Result<BackendCapabilities>;

    /// Create a new VM bound to `cfg`. The VM is created in a `Created`
    /// state — call [`Backend::start`] to begin execution.
    fn create_vm(&self, cfg: &InstanceConfig) -> Result<VmHandle>;

    /// Begin VM execution. Blocks until the VM has signalled it's running.
    fn start(&self, vm: &mut VmHandle) -> Result<()>;

    /// Pause the VM. The guest is frozen but its memory is preserved.
    fn pause(&self, vm: &mut VmHandle) -> Result<()>;

    /// Resume a previously paused VM.
    fn resume(&self, vm: &mut VmHandle) -> Result<()>;

    /// Forcefully stop the VM. Resources are released.
    fn stop(&self, vm: &mut VmHandle) -> Result<()>;

    /// Inject a synthetic input event into the guest (used by the keymapping
    /// engine to send touch / key events through virtio-input).
    fn inject_input(&self, vm: &mut VmHandle, event: InputEvent) -> Result<()>;
}

/// Cross-platform input event used to communicate with virtio-input. Mirrors
/// the subset of evdev events Android expects.
#[derive(Debug, Clone, Copy)]
pub enum InputEvent {
    /// Touch screen down/up/move. Coordinates are in guest display space.
    Touch {
        slot: u32,
        x: u32,
        y: u32,
        pressure: u16,
        active: bool,
    },
    /// Keyboard event (Linux evdev keycode + press state).
    Key { code: u16, pressed: bool },
    /// Pointer movement event (relative, in pixels).
    RelativeMove { dx: i32, dy: i32 },
    /// Mouse button event.
    MouseButton { button: MouseButton, pressed: bool },
    /// Mouse wheel event.
    Wheel { delta: i32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Side,
    Extra,
}
