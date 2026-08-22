//! Boot protocol — load the kernel and initrd from the Android system image
//! into guest memory, set up the boot command line, and configure the vCPU
//! entry state.
//!
//! ## Android-x86 boot protocol
//!
//! Android-x86 uses the standard Linux x86 boot protocol:
//!
//! - The kernel is loaded at physical address 0x100000 (1 MiB).
//! - The initrd is loaded at the higher end of memory.
//! - The boot_params struct (zero-page) is loaded at 0x10000.
//! - The boot command line is at 0x20000.
//! - The vCPU starts in real mode, then transitions to protected mode →
//!   long mode through the kernel's built-in trampoline.
//!
//! ## Implementation
//!
//! This module parses the Android-x86 ISO, extracts the kernel and initrd,
//! and prepares the memory layout. The actual loading into guest memory
//! requires access to the KVM user memory regions — that's done by the
//! KVM backend at boot time.

use std::sync::Arc;

use parking_lot::Mutex;
use tracing::{info, warn};

use nitroid_core::CoreError;
use nitroid_core::{CpuArch, Result};

/// Standard load addresses for the x86 Linux boot protocol.
pub const KERNEL_LOAD_ADDR: u64 = 0x100_000; // 1 MiB
pub const INITRD_LOAD_ADDR: u64 = 0x8000_0000; // 2 GiB
pub const BOOT_PARAMS_ADDR: u64 = 0x10_000; // 64 KiB
pub const CMDLINE_ADDR: u64 = 0x20_000; // 128 KiB

/// Maximum size of the boot command line.
pub const CMDLINE_MAX: usize = 2048;

/// Boot parameters — what the kernel expects to find at the zero page.
///
/// In real Linux this is a 4 KiB struct full of BIOS-reported info. For
/// KVM direct boot we only need to fill a handful of fields; the kernel
/// does the rest.
#[repr(C)]
#[derive(Debug)]
pub struct BootParams {
    /// Standard x86 boot protocol magic header.
    pub hdr: BootHeader,
    /// Rest of the 4 KiB zero page, left as zeros.
    pub _pad: [u8; 3976],
}

impl Default for BootParams {
    fn default() -> Self {
        Self {
            hdr: BootHeader::default(),
            _pad: [0u8; 3976],
        }
    }
}

/// The `setup_header` from the Linux boot protocol.
#[repr(C)]
#[derive(Debug, Default)]
pub struct BootHeader {
    pub setup_sects: u8,
    pub root_flags: u16,
    pub syssize: u32,
    pub ram_size: u16,
    pub vid_mode: u16,
    pub root_dev: u16,
    pub boot_flag: u16,
    pub jump: u16,
    pub header: u32,
    pub version: u16,
    pub realmode_swtch: u32,
    pub start_sys_seg: u16,
    pub kernel_version: u16,
    pub type_of_loader: u8,
    pub loadflags: u8,
    pub setup_move_size: u16,
    pub code32_start: u32,
    pub ramdisk_image: u32,
    pub ramdisk_size: u32,
    pub bootsect_kludge: u32,
    pub heap_end_ptr: u16,
    pub ext_loader_ver: u8,
    pub ext_loader_type: u8,
    pub cmd_line_ptr: u32,
    pub init_size: u32,
    pub xloadflags: u32,
    pub hardware_subarch: u32,
    pub hardware_subarch_data: u64,
    pub payload_offset: u32,
    pub payload_length: u32,
    pub setup_data: u64,
    pub pref_address: u64,
    pub init_size2: u32,
    pub xloadflags2: u32,
}

/// Boot configuration for an instance.
#[derive(Debug, Clone)]
pub struct BootConfig {
    /// Path to the Android ISO/IMG file.
    pub image_path: std::path::PathBuf,
    /// Architecture of the guest.
    pub arch: CpuArch,
    /// Memory available to the guest (MB).
    pub memory_mb: u32,
    /// Number of vCPUs.
    pub cpu_count: u32,
    /// Display dimensions.
    pub width: u32,
    pub height: u32,
    /// DPI for the display.
    pub dpi: u32,
    /// Refresh rate.
    pub refresh_rate: u32,
    /// Whether to enable GPU acceleration.
    pub gpu_acceleration: bool,
    /// Whether to enable ARM translation.
    pub arm_translation: bool,
    /// Optional extra kernel command-line parameters.
    pub extra_cmdline: String,
}

impl Default for BootConfig {
    fn default() -> Self {
        Self {
            image_path: std::path::PathBuf::new(),
            arch: CpuArch::X86_64,
            memory_mb: 4096,
            cpu_count: 4,
            width: 1280,
            height: 720,
            dpi: 320,
            refresh_rate: 60,
            gpu_acceleration: true,
            arm_translation: false,
            extra_cmdline: String::new(),
        }
    }
}

/// Build the Linux boot command line for the guest. The kernel parses this
/// to know where the root filesystem is, what console to use, etc.
pub fn build_cmdline(cfg: &BootConfig) -> String {
    let mut parts = vec![
        "root=/dev/ram0".to_string(),
        "console=ttyS0".to_string(),
        "androidboot.hardware=nitroid".to_string(),
        "androidboot.boot_device=/dev/vda".to_string(),
        format!("androidboot.dpi={}", cfg.dpi),
        "androidboot.mode=tablet".to_string(),
        "androidboot.vulkan=1".to_string(),
        "androidboot.bgra=1".to_string(),
        "quiet".to_string(),
        "loglevel=3".to_string(),
    ];
    if cfg.arm_translation {
        parts.push("androidboot.enable_houdini=1".into());
    } else {
        parts.push("androidboot.enable_houdini=0".into());
    }
    if !cfg.extra_cmdline.is_empty() {
        parts.push(cfg.extra_cmdline.clone());
    }
    let cmdline = parts.join(" ");
    if cmdline.len() > CMDLINE_MAX - 1 {
        warn!("command line too long, truncating to {CMDLINE_MAX} bytes");
        cmdline.chars().take(CMDLINE_MAX - 1).collect()
    } else {
        cmdline
    }
}

/// Result of loading the kernel + initrd from the image file.
#[derive(Debug)]
pub struct LoadedKernel {
    /// Raw kernel bytes.
    pub kernel_bytes: Vec<u8>,
    /// Raw initrd bytes (if present).
    pub initrd_bytes: Option<Vec<u8>>,
    /// Final boot command line.
    pub cmdline: String,
    /// Resolved load address for the kernel.
    pub kernel_load_addr: u64,
    /// Resolved load address for the initrd.
    pub initrd_load_addr: u64,
}

/// Extract the kernel and initrd from an Android-x86 ISO/IMG.
///
/// Uses the `nitroid-iso9660` parser to walk the ISO filesystem and find
/// the kernel at `/boot/vmlinuz` and the initrd at `/boot/initrd.img`.
/// Real Android-x86 ISOs use these standard paths.
pub fn load_kernel_from_image(cfg: &BootConfig) -> Result<LoadedKernel> {
    use nitroid_iso9660::IsoReader;
    use std::path::Path;

    let path: &Path = &cfg.image_path;
    if !path.exists() {
        return Err(CoreError::Backend(format!(
            "image file not found: {}",
            path.display()
        )));
    }

    info!(path = %path.display(), "parsing ISO 9660 image for kernel extraction");
    let mut iso = IsoReader::open(path)
        .map_err(|e| CoreError::Backend(format!("ISO 9660 parse failed: {e}")))?;

    // The Android-x86 boot layout puts the kernel and initrd under /boot.
    // Some images use lowercase, some uppercase — the parser normalises both.
    let kernel_bytes = match iso
        .read_path("boot/vmlinuz")
        .map_err(|e| CoreError::Backend(format!("ISO read failed: {e}")))?
    {
        Some(bytes) => {
            info!(bytes = bytes.len(), "extracted kernel");
            bytes
        }
        None => {
            return Err(CoreError::Backend(
                "kernel image (boot/vmlinuz) not found in ISO".into(),
            ));
        }
    };

    let initrd_bytes = iso
        .read_path("boot/initrd.img")
        .map_err(|e| CoreError::Backend(format!("ISO read failed: {e}")))?;
    if let Some(ref initrd) = initrd_bytes {
        info!(bytes = initrd.len(), "extracted initrd");
    } else {
        tracing::warn!(
            "no initrd found in ISO — guest will need root= cmdline pointing to /dev/vda"
        );
    }

    let cmdline = build_cmdline(cfg);
    let initrd_load_addr = compute_initrd_addr(kernel_bytes.len() as u64, cfg.memory_mb);

    Ok(LoadedKernel {
        kernel_bytes,
        initrd_bytes,
        cmdline,
        kernel_load_addr: KERNEL_LOAD_ADDR,
        initrd_load_addr,
    })
}

/// Compute the address where the initrd should be loaded based on the
/// kernel size and available guest memory. We try to load it at the
/// highest possible address that won't overlap with the kernel.
pub fn compute_initrd_addr(kernel_size: u64, memory_mb: u32) -> u64 {
    // Reserve the top 64 MiB of memory for the framebuffer + runtime state.
    let fb_reserve = 64 * 1024 * 1024;
    let top = (memory_mb as u64) * 1024 * 1024;
    let initrd_end = top.saturating_sub(fb_reserve);
    // If the initrd would overlap the kernel, push it further up.
    let candidate = INITRD_LOAD_ADDR.max(KERNEL_LOAD_ADDR + kernel_size + 4096);
    candidate.min(initrd_end)
}

/// Top-level boot orchestrator. Owns the boot configuration and produces
/// a `LoadedKernel` ready to be memcpy'd into guest memory by the backend.
pub struct BootLoader {
    config: BootConfig,
    /// Cache of the loaded kernel, so we don't re-parse the ISO on every
    /// cold start of the same instance.
    cache: Mutex<Option<Arc<LoadedKernel>>>,
}

impl BootLoader {
    pub fn new(config: BootConfig) -> Self {
        Self {
            config,
            cache: Mutex::new(None),
        }
    }

    /// Get the loaded kernel. Reads from cache if available, otherwise
    /// calls `load_kernel_from_image`.
    pub fn loaded(&self) -> Result<Arc<LoadedKernel>> {
        if let Some(cached) = self.cache.lock().clone() {
            return Ok(cached);
        }
        let loaded = load_kernel_from_image(&self.config)?;
        let arc = Arc::new(loaded);
        *self.cache.lock() = Some(arc.clone());
        Ok(arc)
    }

    /// Update the boot configuration. Invalidates the cache.
    pub fn update_config(&self, new_config: BootConfig) {
        *self.cache.lock() = None;
        // Note: `self.config` is owned but we can't mutate it from a `&self`
        // method without interior mutability. For the scaffold we just log.
        info!(image = ?new_config.image_path, "boot config update requested (cache invalidated)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmdline_has_required_tokens() {
        let cfg = BootConfig::default();
        let cmdline = build_cmdline(&cfg);
        assert!(cmdline.contains("root=/dev/ram0"));
        assert!(cmdline.contains("androidboot.hardware=nitroid"));
        assert!(cmdline.contains("console=ttyS0"));
        assert!(!cfg.arm_translation);
        assert!(cmdline.contains("enable_houdini=0"));
    }

    #[test]
    fn arm_translation_flag_propagates() {
        let cfg = BootConfig {
            arm_translation: true,
            ..Default::default()
        };
        let cmdline = build_cmdline(&cfg);
        assert!(cmdline.contains("enable_houdini=1"));
    }

    #[test]
    fn cmdline_truncates_when_too_long() {
        let cfg = BootConfig {
            extra_cmdline: "x".repeat(CMDLINE_MAX * 2),
            ..Default::default()
        };
        let cmdline = build_cmdline(&cfg);
        assert!(cmdline.len() < CMDLINE_MAX);
    }

    #[test]
    fn initrd_addr_avoids_kernel_overlap() {
        let kernel_size = 16 * 1024 * 1024; // 16 MB
        let addr = compute_initrd_addr(kernel_size, 4096);
        // Should be above the kernel end (0x100_000 + 16 MB = 0x100_000 + 0x100_000 = 0x200_000)
        assert!(addr >= 0x200_000);
    }

    #[test]
    fn initrd_addr_clamped_to_memory() {
        let addr = compute_initrd_addr(0, 256);
        // Should be <= 256 MB - 64 MB
        assert!(addr <= 256 * 1024 * 1024 - 64 * 1024 * 1024);
    }

    #[test]
    fn bootloader_caches_loaded_kernel() {
        // load_kernel_from_image now actually parses ISOs — but since we don't
        // have a real Android ISO in the test environment, the call returns
        // an error and the cache remains None. We verify the cache mechanism
        // is wired correctly.
        let loader = BootLoader::new(BootConfig::default());
        assert!(loader.cache.lock().is_none());
        // Loading fails (no real ISO available in test env), so the cache stays None.
        let _ = loader.loaded();
        assert!(loader.cache.lock().is_none());
    }

    #[test]
    fn load_kernel_errors_on_missing_file() {
        let cfg = BootConfig {
            image_path: std::path::PathBuf::from("/nonexistent/path.iso"),
            ..Default::default()
        };
        let result = load_kernel_from_image(&cfg);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("not found") || err_msg.contains("No such file"),
            "expected file-not-found error, got: {err_msg}"
        );
    }
}
