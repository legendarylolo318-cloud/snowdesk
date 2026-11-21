//! wgpu renderer: aurora → settled-snow texture → instanced sprites.

use crate::sim::Instance;
use crate::sim::grid::Grid;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub const MAX_INSTANCES: usize = 24_000;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    res: [f32; 2],
    grid: [f32; 2],
    cell: f32,
    time: f32,
    aurora: f32,
    sparkle: f32,
}

/// Device, queue and instance, shared by every monitor.
pub struct Gpu {
    pub instance: wgpu::Instance,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter: wgpu::Adapter,
    /// Set by wgpu when the device is lost (driver reset, GPU unplugged) or
    /// an uncaptured error makes it unusable. The overlay is then rebuilt.
    pub lost: Arc<AtomicBool>,
}

impl Gpu {
    /// Create the instance, then the device against the first surface, which
    /// `make_surface` creates from the new instance.
    pub fn new(
        make_surface: impl Fn(&wgpu::Instance) -> anyhow::Result<wgpu::Surface<'static>>,
    ) -> anyhow::Result<(Self, wgpu::Surface<'static>)> {
        // Try the native API alone first: initialising wgpu's GL backend
        // loads Mesa's GL driver and LLVM (~35 MB RSS) for nothing.
        #[cfg(windows)]
        let preferred = wgpu::Backends::DX12;
        #[cfg(not(windows))]
        let preferred = wgpu::Backends::VULKAN;
        let mut last_err = None;
        for backends in [preferred, wgpu::Backends::all()] {
            let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
            desc.backends = backends;
            // Transparent windows on DX12 need a DirectComposition swapchain.
            desc.backend_options.dx12.presentation_system = wgpu::Dx12SwapchainKind::DxgiFromVisual;
            let instance = wgpu::Instance::new(desc);
            let surface = make_surface(&instance)?;
            match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })) {
                Ok(adapter) => return Self::with_adapter(instance, adapter, surface),
                Err(e) => last_err = Some(e),
            }
        }
        Err(anyhow::anyhow!(
            "no usable GPU adapter: {}",
            last_err.map(|e| e.to_string()).unwrap_or_default()
        ))
    }

    fn with_adapter(
        instance: wgpu::Instance,
        adapter: wgpu::Adapter,
        surface: wgpu::Surface<'static>,
    ) -> anyhow::Result<(Self, wgpu::Surface<'static>)> {
        let info = adapter.get_info();
        tracing::info!(
            "GPU: {} ({:?}, {:?})",
            info.name,
            info.device_type,
            info.backend
        );
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("snowdesk"),
                // The overlay needs a few MB of GPU memory. The default
                // "Performance" hint pre-allocates 128-256 MB blocks.
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                ..Default::default()
            }))?;
        let lost = Arc::new(AtomicBool::new(false));
        {
            let lost = lost.clone();
            device.set_device_lost_callback(move |reason, msg| {
                if reason != wgpu::DeviceLostReason::Destroyed {
                    tracing::error!("GPU device lost ({reason:?}): {msg}");
                    lost.store(true, Ordering::Relaxed);
                }
            });
        }
        {
            // Default behaviour is to panic; log and rebuild instead.
            let lost = lost.clone();
            device.on_uncaptured_error(Arc::new(move |e| {
                tracing::error!("GPU error: {e}");
                lost.store(true, Ordering::Relaxed);
            }));
        }
        Ok((
            Self {
                instance,
                device,
                queue,
                adapter,
                lost,
            },
            surface,
        ))
    }

    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::Relaxed)
    }
}

pub struct MonitorRenderer {
    gpu: Arc<Gpu>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    uniforms: wgpu::Buffer,
    grid_tex: wgpu::Texture,
    grid_dims: (usize, usize),
    frost_tex: wgpu::Texture,
    frost_dims: (u32, u32),
    frost_pipe: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    bind: wgpu::BindGroup,
    layout: wgpu::BindGroupLayout,
    grid_pipe: wgpu::RenderPipeline,
    aurora_pipe: wgpu::RenderPipeline,
    sprite_pipe: wgpu::RenderPipeline,
    instances: wgpu::Buffer,
    /// Reused list of dirty grid rects (no per-frame allocation).
    dirty: Vec<(usize, usize, usize, usize)>,
    /// Swapchain shrunk to a tiny size while the monitor is blanked.
    asleep: bool,
}

/// Swapchain side while blanked: divisible by every integer buffer scale up
/// to 4, so a Wayland compositor accepts it.
const SLEEP_SIZE: u32 = 12;

impl MonitorRenderer {
    pub fn new(
        gpu: Arc<Gpu>,
        surface: wgpu::Surface<'static>,
        w: u32,
        h: u32,
        grid: &Grid,
    ) -> anyhow::Result<Self> {
        let dev = &gpu.device;
        let caps = surface.get_capabilities(&gpu.adapter);
        use wgpu::TextureFormat as F;
        let format = [F::Bgra8Unorm, F::Rgba8Unorm]
            .into_iter()
            .find(|f| caps.formats.contains(f))
            .or_else(|| caps.formats.iter().copied().find(|f| !f.is_srgb()))
            .unwrap_or(caps.formats[0]);
        use wgpu::CompositeAlphaMode as A;
        let alpha = [A::PreMultiplied, A::Inherit, A::PostMultiplied]
            .into_iter()
            .find(|a| caps.alpha_modes.contains(a))
            .unwrap_or(caps.alpha_modes[0]);
        tracing::info!("surface format {format:?}, alpha {alpha:?}");
        if matches!(alpha, A::Opaque) {
            // Never show an opaque fullscreen window over the desktop.
            anyhow::bail!(
                "the GPU surface does not support transparency (alpha modes: {:?})",
                caps.alpha_modes
            );
        }
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: w.max(1),
            height: h.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 1,
            alpha_mode: alpha,
            color_space: Default::default(),
            view_formats: vec![],
        };
        surface.configure(dev, &config);

        let shader = dev.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("snow"),
            source: wgpu::ShaderSource::Wgsl(include_str!("render.wgsl").into()),
        });
        let layout = dev.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pl = dev.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let blend = Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING);
        let target = [Some(wgpu::ColorTargetState {
            format,
            blend,
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let full = |label: &str, fs: &str| {
            dev.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_full"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &target,
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let grid_pipe = full("grid", "fs_grid");
        let aurora_pipe = full("aurora", "fs_aurora");
        let frost_pipe = full("frost", "fs_frost");
        let attrs = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32, 3 => Uint32, 4 => Float32x4];
        let sprite_pipe = dev.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sprites"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_sprite"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Instance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &attrs,
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_sprite"),
                compilation_options: Default::default(),
                targets: &target,
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        let uniforms = dev.create_buffer(&wgpu::BufferDescriptor {
            label: Some("u"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let instances = dev.create_buffer(&wgpu::BufferDescriptor {
            label: Some("inst"),
            size: (MAX_INSTANCES * std::mem::size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = dev.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let frost_tex = Self::make_frost(&gpu, 1, 1);
        let grid_tex = Self::make_grid(&gpu, grid);
        let bind = Self::make_bind(&gpu, &layout, &uniforms, &grid_tex, &frost_tex, &sampler);
        Ok(Self {
            gpu,
            surface,
            config,
            uniforms,
            grid_tex,
            grid_dims: (grid.w, grid.h),
            bind,
            frost_tex,
            frost_dims: (1, 1),
            frost_pipe,
            sampler,
            layout,
            grid_pipe,
            aurora_pipe,
            sprite_pipe,
            instances,
            dirty: Vec::new(),
            asleep: false,
        })
    }

    fn make_grid(gpu: &Gpu, grid: &Grid) -> wgpu::Texture {
        gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("grid"),
            size: wgpu::Extent3d {
                width: grid.w as u32,
                height: grid.h as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    fn make_frost(gpu: &Gpu, w: u32, h: u32) -> wgpu::Texture {
        gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("frost"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    fn make_bind(
        gpu: &Gpu,
        layout: &wgpu::BindGroupLayout,
        uniforms: &wgpu::Buffer,
        grid: &wgpu::Texture,
        frost: &wgpu::Texture,
        sampler: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        let gv = grid.create_view(&Default::default());
        let fv = frost.create_view(&Default::default());
        gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&gv),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&fv),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
    }

    fn rebuild_bind(&mut self) {
        self.bind = Self::make_bind(
            &self.gpu,
            &self.layout,
            &self.uniforms,
            &self.grid_tex,
            &self.frost_tex,
            &self.sampler,
        );
    }

    /// Upload the frost map if it changed (it is small and changes slowly).
    fn upload_frost(&mut self, f: &mut crate::sim::frost::Frost) {
        if !f.dirty {
            return;
        }
        f.dirty = false;
        if self.frost_dims != (f.w, f.h) {
            self.frost_tex = Self::make_frost(&self.gpu, f.w, f.h);
            self.frost_dims = (f.w, f.h);
            self.rebuild_bind();
        }
        self.gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.frost_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &f.data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(f.w),
                rows_per_image: Some(f.h),
            },
            wgpu::Extent3d {
                width: f.w,
                height: f.h,
                depth_or_array_layers: 1,
            },
        );
    }

    pub fn resize(&mut self, w: u32, h: u32, grid: &mut Grid) {
        self.config.width = w.max(1);
        self.config.height = h.max(1);
        if !self.asleep {
            self.surface.configure(&self.gpu.device, &self.config);
        }
        if self.grid_dims != (grid.w, grid.h) {
            self.grid_tex = Self::make_grid(&self.gpu, grid);
            self.grid_dims = (grid.w, grid.h);
            self.rebuild_bind();
        }
        grid.mark_all_dirty();
    }

    /// Render one frame. `aurora` and `sparkle` are feature strengths.
    /// Returns false if the surface needs attention (frame skipped).
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        grid: &mut Grid,
        frost: &mut crate::sim::frost::Frost,
        cell: f32,
        instances: &[Instance],
        time: f32,
        aurora: f32,
        sparkle: bool,
    ) -> bool {
        if self.asleep {
            self.asleep = false;
            self.surface.configure(&self.gpu.device, &self.config);
        }
        self.upload_frost(frost);
        let frosty = frost.any;
        let q = &self.gpu.queue;
        // Upload only the dirty chunks.
        let gw = grid.w as u32;
        let tex = &self.grid_tex;
        let mut rects = std::mem::take(&mut self.dirty);
        rects.clear();
        grid.drain_dirty(|x, y, w, h| rects.push((x, y, w, h)));
        for &(x, y, w, h) in &rects {
            q.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: x as u32,
                        y: y as u32,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &grid.cells[y * grid.w + x..],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(gw),
                    rows_per_image: Some(h as u32),
                },
                wgpu::Extent3d {
                    width: w as u32,
                    height: h as u32,
                    depth_or_array_layers: 1,
                },
            );
        }
        self.dirty = rects;
        let u = Uniforms {
            res: [self.config.width as f32, self.config.height as f32],
            grid: [grid.w as f32, grid.h as f32],
            cell,
            time,
            aurora,
            sparkle: if sparkle { 1.0 } else { 0.0 },
        };
        q.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&u));
        let n = instances.len().min(MAX_INSTANCES);
        if n > 0 {
            q.write_buffer(&self.instances, 0, bytemuck::cast_slice(&instances[..n]));
        }

        use wgpu::CurrentSurfaceTexture as C;
        let frame = match self.surface.get_current_texture() {
            C::Success(f) | C::Suboptimal(f) => f,
            C::Outdated | C::Lost => {
                self.surface.configure(&self.gpu.device, &self.config);
                return false;
            }
            _ => return false,
        };
        let view = frame.texture.create_view(&Default::default());
        let mut enc = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.bind, &[]);
            if aurora > 0.0 {
                pass.set_pipeline(&self.aurora_pipe);
                pass.draw(0..3, 0..1);
            }
            if frosty {
                pass.set_pipeline(&self.frost_pipe);
                pass.draw(0..3, 0..1);
            }
            // Settled snow: only shade rows that can contain snow.
            if let Some(row) = grid.first_snow_row() {
                let y0 = ((row as f32 * cell) as u32).min(self.config.height - 1);
                pass.set_scissor_rect(0, y0, self.config.width, self.config.height - y0);
                pass.set_pipeline(&self.grid_pipe);
                pass.draw(0..3, 0..1);
                pass.set_scissor_rect(0, 0, self.config.width, self.config.height);
            }
            if n > 0 {
                pass.set_pipeline(&self.sprite_pipe);
                pass.set_vertex_buffer(0, self.instances.slice(..));
                pass.draw(0..6, 0..n as u32);
            }
        }
        q.submit(Some(enc.finish()));
        self.gpu.queue.present(frame);
        true
    }
}

impl MonitorRenderer {
    /// Present a fully transparent frame (used when pausing so no stale snow
    /// stays on screen).
    pub fn blank(&mut self) {
        self.present_clear();
    }

    /// Blank and give the swapchain memory back: the surface is reconfigured
    /// to a tiny transparent buffer until the next `render`. Only for
    /// surfaces whose size is free (Wayland); X11 and Windows swapchains
    /// must match the window size.
    pub fn blank_and_sleep(&mut self) {
        if !self.asleep {
            self.asleep = true;
            let mut tiny = self.config.clone();
            tiny.width = SLEEP_SIZE;
            tiny.height = SLEEP_SIZE;
            self.surface.configure(&self.gpu.device, &tiny);
        }
        self.present_clear();
    }

    fn present_clear(&mut self) {
        use wgpu::CurrentSurfaceTexture as C;
        let frame = match self.surface.get_current_texture() {
            C::Success(f) | C::Suboptimal(f) => f,
            _ => return,
        };
        let view = frame.texture.create_view(&Default::default());
        let mut enc = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        drop(enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        }));
        self.gpu.queue.submit(Some(enc.finish()));
        self.gpu.queue.present(frame);
    }
}
