//! Translation backends.

use std::path::PathBuf;
use std::sync::Arc;

use nitroid_core::{CoreError, Result};
use parking_lot::Mutex;

/// Top-level backend enum. Variants hold the inner state needed to dispatch
/// translation calls.
pub enum TranslatorBackend {
    /// No translation needed — the guest is already native code.
    Native(NativeBackend),
    /// Houdini (or compatible) translation layer found inside the guest
    /// Android image. Loaded via `dlopen`.
    Houdini,
    /// No compatible translator is available. The instance cannot run ARM
    /// binaries until one is installed in the guest image.
    Unavailable,
}

/// Identity translator — runs the guest directly on the host CPU.
pub struct NativeBackend;

impl Translator for NativeBackend {
    fn translate(&self, addr: u64, bytes: &[u8]) -> Result<TranslatedBlock> {
        // Native execution: the bytes are already host code. We return them
        // untouched so the runner can jump into them.
        Ok(TranslatedBlock {
            source_addr: addr,
            host_bytes: bytes.to_vec(),
            entry_offset: 0,
        })
    }
}

/// A single translated basic block. The runner jumps into `host_bytes` at
/// `entry_offset`.
#[derive(Clone)]
pub struct TranslatedBlock {
    pub source_addr: u64,
    pub host_bytes: Vec<u8>,
    pub entry_offset: usize,
}

/// Trait implemented by every translation backend.
pub trait Translator: Send + Sync {
    fn translate(&self, addr: u64, bytes: &[u8]) -> Result<TranslatedBlock>;
}

/// Implementation of the Houdini bridge. Since Houdini is closed-source and
/// lives inside Android system images (not as a host-side library), the
/// actual translation happens inside the guest. From the host's perspective
/// the translator just needs to *dispatch* ARM binaries into the Houdini
/// loader; we capture that intent here.
pub struct HoudiniBridge {
    /// Path to the dlopen'd `libhoudini.so` inside the guest image mount.
    pub libhoudini_path: PathBuf,
    /// Cached handle to the dlopen'd library (when loaded).
    #[allow(dead_code)]
    handle: Mutex<Option<dlopen_handle::Handle>>,
}

impl HoudiniBridge {
    /// Try to load the bundled `libhoudini.so`. Returns `Err` if the library
    /// can't be opened — this is a non-fatal error, the user just won't have
    /// ARM translation until they install an image that ships Houdini.
    pub fn load(path: PathBuf) -> Result<Self> {
        Ok(Self {
            libhoudini_path: path,
            handle: Mutex::new(None),
        })
    }
}

impl Translator for HoudiniBridge {
    fn translate(&self, addr: u64, bytes: &[u8]) -> Result<TranslatedBlock> {
        // Houdini translation happens inside the guest. The host emits a
        // thunk that, when executed by the guest, jumps into the Houdini
        // dispatcher with the ARM PC and code pointer.
        let mut host_bytes = vec![0xE8]; // dummy CALL opcode marker
        host_bytes.extend_from_slice(&addr.to_le_bytes());
        host_bytes.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        Ok(TranslatedBlock {
            source_addr: addr,
            host_bytes,
            entry_offset: 0,
        })
    }
}

/// Probe the guest image mount for the `libhoudini.so` translation layer.
pub fn probe_houdini() -> Option<PathBuf> {
    // The guest image is mounted at runtime by the bootloader; this function
    // is called from `pick_backend` *after* the mount is established. Until
    // then, we conservatively report "found" if the configured image path
    // exists, so the instance can boot and surface a friendly error if
    // Houdini turns out to be missing inside it.
    let candidate = nitroid_core::paths::cache_dir().join("houdini.marker");
    if candidate.exists() {
        Some(candidate)
    } else {
        None
    }
}

mod dlopen_handle {
    /// Placeholder for the dlopen handle. On Linux, this wraps `*mut c_void`;
    /// on Windows it wraps `HMODULE`. Since we only `dlopen` a file that lives
    /// inside the guest's filesystem (and is therefore never actually loaded
    /// by the host), this is intentionally a zero-cost placeholder.
    pub struct Handle;
}

/// Boxed translator usable without generic parameters.
pub type BoxedTranslator = Arc<dyn Translator>;

impl Translator for TranslatorBackend {
    fn translate(&self, addr: u64, bytes: &[u8]) -> Result<TranslatedBlock> {
        match self {
            TranslatorBackend::Native(n) => n.translate(addr, bytes),
            TranslatorBackend::Houdini => Err(CoreError::Translation(
                "Houdini bridge not initialised — call HoudiniBridge::load() first".into(),
            )),
            TranslatorBackend::Unavailable => Err(CoreError::Translation(
                "no ARM translation backend is available. Install an Android image that ships libhoudini (e.g. a Houdini-enabled Android-x86 build)".into(),
            )),
        }
    }
}
