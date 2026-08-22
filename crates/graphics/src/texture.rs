//! Texture management — wraps the host-side WGPU texture that mirrors the
//! guest's framebuffer.

use nitroid_core::Result;
use parking_lot::Mutex;

/// A host-side texture that mirrors the guest Android framebuffer. The guest
/// writes pixels into the underlying buffer (via virtio-gpu commands); the
/// host renderer samples from it to present.
pub struct GuestTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
    pub format: wgpu::TextureFormat,
    pub dirty: Mutex<bool>,
}

impl GuestTexture {
    /// Create a new guest texture of the requested dimensions.
    pub fn new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> Result<Self> {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("guest framebuffer"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(Self {
            texture,
            view,
            width,
            height,
            format,
            dirty: Mutex::new(false),
        })
    }

    /// Upload raw RGBA8 pixels from the guest. Caller is responsible for
    /// ensuring `bytes.len() == width * height * 4`.
    pub fn upload(&self, queue: &wgpu::Queue, bytes: &[u8]) -> Result<()> {
        let expected = (self.width * self.height * 4) as usize;
        if bytes.len() < expected {
            return Err(nitroid_core::CoreError::Graphics(format!(
                "upload size mismatch: got {} bytes, expected {expected}",
                bytes.len()
            )));
        }
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(self.width * 4),
                rows_per_image: Some(self.height),
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        *self.dirty.lock() = true;
        Ok(())
    }
}
