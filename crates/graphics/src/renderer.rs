//! Top-level renderer — orchestrates the surface and guest texture.

use nitroid_core::{GraphicsBackend, Result};
use parking_lot::RwLock;

use crate::surface::Surface;
use crate::texture::GuestTexture;

/// Top-level renderer. Owns the WGPU surface, the guest texture, and the
/// blit pipeline that composites the guest framebuffer to the host window.
pub struct Renderer {
    pub instance: wgpu::Instance,
    pub surface: Option<Surface>,
    pub guest: RwLock<Option<GuestTexture>>,
    pub backend: GraphicsBackend,
}

impl Renderer {
    /// Create a renderer without a surface (headless mode — used by tests
    /// and the CI smoke check).
    pub fn headless(backend: GraphicsBackend) -> Result<Self> {
        let instance = crate::create_instance(backend)?;
        Ok(Self {
            instance,
            surface: None,
            guest: RwLock::new(None),
            backend,
        })
    }

    /// Allocate (or reallocate) the guest texture to match the requested
    /// resolution. Must be called after `attach_surface` so we have a device
    /// to allocate against.
    pub fn ensure_guest(&self, device: &wgpu::Device, w: u32, h: u32) -> Result<()> {
        let mut guest = self.guest.write();
        let needs_recreate = match guest.as_ref() {
            None => true,
            Some(g) => g.width != w || g.height != h,
        };
        if needs_recreate {
            *guest = Some(GuestTexture::new(
                device,
                w,
                h,
                wgpu::TextureFormat::Rgba8Unorm,
            )?);
        }
        Ok(())
    }
}
