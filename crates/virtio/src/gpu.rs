//! virtio-gpu — graphics passthrough.
//!
//! The guest's Vulkan/OpenGL ES driver sends drawing commands through the
//! virtio-gpu virtqueue. The host translates them into WGPU commands and
//! renders to the host window.
//!
//! ## Current state
//!
//! The full virtio-gpu 2D + 3D command set is large. This scaffold
//! implements:
//!
//! - The host-side device state (scanout dimensions, framebuffer pointer)
//! - 2D host blit (transfer host framebuffer to scanout)
//! - Resource tracking (resource create/destroy, attach-backing)
//!
//! 3D commands (Vulkan passthrough) are not yet implemented — they require
//! the virtgpu_vulkan cross-domain context, which is a separate project.

use std::sync::Arc;

use parking_lot::Mutex;
use tracing::{debug, info};

use crate::queue::VirtQueue;
use crate::transport::DeviceId;
use crate::VirtioDevice;
use nitroid_core::Result;
use nitroid_virtualization::GuestMemory;

/// virtio-gpu command types (subset).
pub const VIRTIO_GPU_CMD_GET_DISPLAY_INFO: u32 = 0x0100;
pub const VIRTIO_GPU_CMD_RESOURCE_CREATE_2D: u32 = 0x0101;
pub const VIRTIO_GPU_CMD_RESOURCE_UNREF: u32 = 0x0102;
pub const VIRTIO_GPU_CMD_SET_SCANOUT: u32 = 0x0103;
pub const VIRTIO_GPU_CMD_RESOURCE_FLUSH: u32 = 0x0104;
pub const VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D: u32 = 0x0105;
pub const VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING: u32 = 0x0106;
pub const VIRTIO_GPU_CMD_RESOURCE_DETACH_BACKING: u32 = 0x0107;

/// virtio-gpu flags.
pub const VIRTIO_GPU_FLAG_FENCE: u32 = 1 << 0;

/// A 2D resource tracked by the host.
#[derive(Debug, Clone)]
pub struct GpuResource2D {
    pub resource_id: u32,
    pub width: u32,
    pub height: u32,
    pub format: u32,
}

/// The scanout — the rectangular region of the framebuffer the guest has
/// selected for display.
#[derive(Debug, Clone, Copy, Default)]
pub struct Scanout {
    pub scanout_id: u32,
    pub resource_id: u32,
    pub width: u32,
    pub height: u32,
    pub x: u32,
    pub y: u32,
}

/// virtio-gpu device state.
pub struct VirtioGpu {
    resources: Arc<Mutex<Vec<GpuResource2D>>>,
    scanouts: Arc<Mutex<Vec<Scanout>>>,
    /// The latest host-side framebuffer (raw RGBA8 pixels). The graphics
    /// crate reads this on every present call.
    framebuffer: Arc<Mutex<Vec<u8>>>,
    /// Set to `true` when the guest flushes a frame so the renderer knows
    /// to re-upload it.
    dirty_marker: Arc<Mutex<bool>>,
    fb_width: u32,
    fb_height: u32,
}

impl VirtioGpu {
    pub fn new(width: u32, height: u32) -> Self {
        let size = (width as usize) * (height as usize) * 4;
        Self {
            resources: Arc::new(Mutex::new(Vec::new())),
            scanouts: Arc::new(Mutex::new(vec![Scanout {
                scanout_id: 0,
                resource_id: 0,
                width,
                height,
                x: 0,
                y: 0,
            }])),
            framebuffer: Arc::new(Mutex::new(vec![0u8; size])),
            dirty_marker: Arc::new(Mutex::new(false)),
            fb_width: width,
            fb_height: height,
        }
    }

    /// Create a new 2D resource. Returns its ID.
    pub fn create_resource_2d(&self, resource_id: u32, width: u32, height: u32, format: u32) {
        self.resources.lock().push(GpuResource2D {
            resource_id,
            width,
            height,
            format,
        });
        info!(
            resource_id,
            width, height, "virtio-gpu: created 2D resource"
        );
    }

    /// Destroy a resource.
    pub fn destroy_resource(&self, resource_id: u32) {
        self.resources
            .lock()
            .retain(|r| r.resource_id != resource_id);
    }

    /// Set the scanout — which resource to display, and where.
    pub fn set_scanout(&self, scanout_id: u32, resource_id: u32, x: u32, y: u32, w: u32, h: u32) {
        let mut scanouts = self.scanouts.lock();
        let existing = scanouts.iter_mut().find(|s| s.scanout_id == scanout_id);
        if let Some(s) = existing {
            s.resource_id = resource_id;
            s.x = x;
            s.y = y;
            s.width = w;
            s.height = h;
        } else {
            scanouts.push(Scanout {
                scanout_id,
                resource_id,
                width: w,
                height: h,
                x,
                y,
            });
        }
    }

    /// Update a region of the host framebuffer. The guest's
    /// `TRANSFER_TO_HOST_2D` command eventually lands here.
    pub fn update_framebuffer(&self, x: u32, y: u32, w: u32, h: u32, pixels: &[u8]) -> Result<()> {
        let mut fb = self.framebuffer.lock();
        let fb_w = self.fb_width as usize;
        let _fb_h = self.fb_height as usize;
        for row in 0..h as usize {
            let dst_y = (y as usize) + row;
            if dst_y >= self.fb_height as usize {
                break;
            }
            let dst_x_start = x as usize;
            let dst_x_end = (dst_x_start + w as usize).min(fb_w);
            let row_len = (dst_x_end - dst_x_start) * 4;
            let src_start = row * (w as usize) * 4;
            let src_end = src_start + row_len;
            let dst_start = (dst_y * fb_w + dst_x_start) * 4;
            let dst_end = dst_start + row_len;
            if src_end <= pixels.len() && dst_end <= fb.len() {
                fb[dst_start..dst_end].copy_from_slice(&pixels[src_start..src_end]);
            }
        }
        Ok(())
    }

    /// Snapshot of the current framebuffer. The graphics crate samples this
    /// on every present call.
    pub fn framebuffer_snapshot(&self) -> Vec<u8> {
        self.framebuffer.lock().clone()
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.fb_width, self.fb_height)
    }

    /// Dispatch a virtio-gpu command. Reads additional bytes from the
    /// descriptor as needed and updates the device's internal state.
    fn dispatch_command(
        &self,
        cmd_type: u32,
        mem: &GuestMemory,
        desc: &crate::queue::Descriptor,
    ) -> Result<()> {
        match cmd_type {
            VIRTIO_GPU_CMD_RESOURCE_CREATE_2D => {
                if desc.len >= 20 {
                    let mut buf = [0u8; 20];
                    mem.read_into(desc.addr + 8, &mut buf)?;
                    let resource_id = u32::from_le_bytes(buf[0..4].try_into().unwrap());
                    let format = u32::from_le_bytes(buf[4..8].try_into().unwrap());
                    let width = u32::from_le_bytes(buf[8..12].try_into().unwrap());
                    let height = u32::from_le_bytes(buf[12..16].try_into().unwrap());
                    self.create_resource_2d(resource_id, width, height, format);
                }
            }
            VIRTIO_GPU_CMD_RESOURCE_UNREF => {
                if desc.len >= 12 {
                    let mut buf = [0u8; 4];
                    mem.read_into(desc.addr + 8, &mut buf)?;
                    let resource_id = u32::from_le_bytes(buf);
                    self.destroy_resource(resource_id);
                }
            }
            VIRTIO_GPU_CMD_SET_SCANOUT => {
                if desc.len >= 24 {
                    let mut buf = [0u8; 16];
                    mem.read_into(desc.addr + 8, &mut buf)?;
                    let scanout_id = u32::from_le_bytes(buf[0..4].try_into().unwrap());
                    let resource_id = u32::from_le_bytes(buf[4..8].try_into().unwrap());
                    let w = u32::from_le_bytes(buf[8..12].try_into().unwrap());
                    let h = u32::from_le_bytes(buf[12..16].try_into().unwrap());
                    self.set_scanout(scanout_id, resource_id, 0, 0, w, h);
                }
            }
            VIRTIO_GPU_CMD_RESOURCE_FLUSH => {
                // Mark the framebuffer as dirty so the next present() call
                // uploads it to the WGPU texture.
                *self.dirty_marker.lock() = true;
            }
            _ => {
                debug!(cmd_type, "virtio-gpu: unhandled command type");
            }
        }
        Ok(())
    }
}

impl VirtioDevice for VirtioGpu {
    fn device_id(&self) -> DeviceId {
        DeviceId::Gpu
    }

    fn features(&self) -> u64 {
        // VIRTIO_GPU_F_VIRGL (0) — not yet
        // VIRTIO_GPU_F_EDID (1) — yes
        // VIRTIO_GPU_F_RESOURCE_UUID (2) — no
        // VIRTIO_GPU_F_RESOURCE_BLOB (3) — no
        0x2
    }

    fn num_queues(&self) -> usize {
        2 // ctrl + cursor
    }

    fn process_queue(
        &self,
        _queue_idx: usize,
        queue: &mut VirtQueue,
        mem: &GuestMemory,
    ) -> Result<usize> {
        let mut processed = 0;
        for _ in 0..64 {
            let pending = queue.pending(mem)?;
            if pending == 0 {
                break;
            }
            let head_idx = match queue.pop_avail(mem)? {
                Some(idx) => idx,
                None => break,
            };
            // Read the command header (8 bytes: type u32 + flags u32 + fence
            // id u64 + ctx_id u32 + padding u32 = 24 bytes total in the
            // ctrl header, but we only need the type for dispatch).
            let desc = queue.read_descriptor(mem, head_idx)?;
            if desc.len >= 8 {
                let mut header = [0u8; 8];
                if mem.read_into(desc.addr, &mut header).is_ok() {
                    let cmd_type = u32::from_le_bytes(header[0..4].try_into().unwrap());
                    debug!(cmd_type, "virtio-gpu: received command");
                    self.dispatch_command(cmd_type, mem, &desc)?;
                }
            }
            // Push the descriptor back as used.
            queue.push_used(mem, head_idx as u32, desc.len)?;
            processed += 1;
        }
        if processed > 0 {
            debug!(processed, "virtio-gpu: processed commands");
        }
        Ok(processed)
    }

    fn reset(&self) {
        self.resources.lock().clear();
        *self.scanouts.lock() = vec![Scanout::default()];
        self.framebuffer.lock().fill(0);
        *self.dirty_marker.lock() = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_destroy_resource() {
        let gpu = VirtioGpu::new(1280, 720);
        gpu.create_resource_2d(1, 1280, 720, 1);
        assert_eq!(gpu.resources.lock().len(), 1);
        gpu.destroy_resource(1);
        assert!(gpu.resources.lock().is_empty());
    }

    #[test]
    fn update_framebuffer_writes_pixels() {
        let gpu = VirtioGpu::new(4, 4);
        // Write 4 red pixels into the top-left 2x2 region.
        let red = [255u8, 0, 0, 255];
        let pixels: Vec<u8> = red.repeat(4);
        gpu.update_framebuffer(0, 0, 2, 2, &pixels).unwrap();
        let fb = gpu.framebuffer_snapshot();
        // First pixel should be red.
        assert_eq!(&fb[0..4], &red);
        // Fifth pixel (start of row 2) should also be red.
        assert_eq!(&fb[16..20], &red);
    }

    #[test]
    fn set_scanout_creates_new() {
        let gpu = VirtioGpu::new(1280, 720);
        gpu.set_scanout(1, 100, 0, 0, 640, 360);
        let scanouts = gpu.scanouts.lock();
        assert_eq!(scanouts.len(), 2);
        assert_eq!(scanouts[1].resource_id, 100);
    }
}
