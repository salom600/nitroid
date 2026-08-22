//! Graphics rendering via WGPU.
//!
//! This crate owns the host-side graphics context — the WGPU device, the
//! surface that the Android guest's virtio-gpu commands are blitted onto, and
//! the swapchain management. The guest's Vulkan/OpenGL ES commands are
//! translated by a separate translation pass (the `nitroid-translation`
//! crate); this crate just renders the resulting framebuffer to the window.
//!
//! Why WGPU?
//!
//! - Single API surface for DX12 (Windows), Vulkan (Linux), Metal (macOS).
//! - Mature, well-tested, used by Firefox, Bevy, Ruffle.
//! - Lets us ship one binary that works on every host without per-vendor
//!   code paths.

pub mod renderer;
pub mod surface;
pub mod texture;

pub use renderer::Renderer;
pub use surface::Surface;
pub use texture::GuestTexture;

use nitroid_core::{GraphicsBackend, Result};

/// Map a [`GraphicsBackend`] choice onto WGPU's `Backends` bitmask.
pub fn backends_for(choice: GraphicsBackend) -> wgpu::Backends {
    use wgpu::Backends as B;
    match choice {
        GraphicsBackend::Auto => B::all(),
        GraphicsBackend::Vulkan => B::VULKAN,
        GraphicsBackend::Dx12 => B::DX12,
        GraphicsBackend::Metal => B::METAL,
        GraphicsBackend::OpenGl => B::GL,
    }
}

/// Initialise a WGPU instance for the requested backend set.
pub fn create_instance(backend: GraphicsBackend) -> Result<wgpu::Instance> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: backends_for(backend),
        flags: wgpu::InstanceFlags::default(),
        dx12_shader_compiler: wgpu::Dx12Compiler::default(),
        gles_minor_version: wgpu::Gles3MinorVersion::default(),
    });
    Ok(instance)
}
