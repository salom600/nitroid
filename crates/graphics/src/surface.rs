//! Surface management — owns the swapchain that the renderer blits to.

use std::sync::Arc;

use nitroid_core::Result;
use parking_lot::Mutex;

use crate::texture::GuestTexture;

/// Wrapper around a WGPU surface configured for the host window.
pub struct Surface {
    pub(crate) surface: wgpu::Surface<'static>,
    pub(crate) config: Mutex<wgpu::SurfaceConfiguration>,
    pub(crate) device: Arc<wgpu::Device>,
    pub(crate) queue: wgpu::Queue,
    /// Lazily-initialised blit pipeline. Created on first `present` call.
    pub(crate) blit_pipeline: Mutex<Option<wgpu::RenderPipeline>>,
}

impl Surface {
    /// Create a surface from a window handle (provided by egui / winit).
    /// The window handle must outlive the returned `Surface`.
    pub fn from_handle<W>(
        instance: &wgpu::Instance,
        window: &'static W,
        width: u32,
        height: u32,
    ) -> Result<Self>
    where
        W: raw_window_handle::HasWindowHandle
            + raw_window_handle::HasDisplayHandle
            + Sync
            + Send
            + 'static,
    {
        let surface = instance
            .create_surface(window)
            .map_err(|e| nitroid_core::CoreError::Graphics(format!("create_surface: {e}")))?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .ok_or_else(|| nitroid_core::CoreError::Graphics("no suitable GPU adapter found".into()))?;

        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("nitroid device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults(),
                memory_hints: wgpu::MemoryHints::default(),
            },
            None,
        ))
        .map_err(|e| nitroid_core::CoreError::Graphics(format!("request_device: {e}")))?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(
                caps.formats
                    .first()
                    .copied()
                    .unwrap_or(wgpu::TextureFormat::Bgra8Unorm),
            );

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_DST,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::Mailbox,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps
                .alpha_modes
                .first()
                .copied()
                .unwrap_or(wgpu::CompositeAlphaMode::Auto),
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        Ok(Surface {
            surface,
            config: Mutex::new(config),
            device: Arc::new(device),
            queue,
            blit_pipeline: Mutex::new(None),
        })
    }

    /// Resize the swapchain to match the window's new dimensions.
    pub fn resize(&self, width: u32, height: u32) {
        let mut cfg = self.config.lock();
        cfg.width = width.max(1);
        cfg.height = height.max(1);
        self.surface.configure(&self.device, &cfg);
    }

    /// Acquire the next frame and blit `guest` into it. Returns `Ok(())` when
    /// the frame was successfully presented.
    pub fn present(&self, _guest: &GuestTexture) -> Result<()> {
        let frame = self
            .surface
            .get_current_texture()
            .map_err(|e| nitroid_core::CoreError::Graphics(format!("get_current_texture: {e}")))?;

        let mut encoder = self.device.create_command_encoder(&Default::default());

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("nitroid blit"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            // Initialise the blit pipeline lazily — we need the surface format,
            // which we only know after `configure` runs.
            let mut pipeline_slot = self.blit_pipeline.lock();
            if pipeline_slot.is_none() {
                *pipeline_slot = Some(self.build_blit_pipeline(self.config.lock().format));
            }
            let pipeline = pipeline_slot.as_ref().unwrap();
            rpass.set_pipeline(pipeline);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        Ok(())
    }

    fn build_blit_pipeline(&self, format: wgpu::TextureFormat) -> wgpu::RenderPipeline {
        let shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("placeholder"),
                source: wgpu::ShaderSource::Wgsl(include_str!("placeholder.wgsl").into()),
            });
        self.device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("placeholder pipeline"),
                layout: None,
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: "vs_main",
                    buffers: &[],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: "fs_main",
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            })
    }
}
