//! wgpu drawing: one R8 texture per decoded tile, one instanced quad per tile.

use decoder::Gray8Buf;

const SHADER: &str = r#"
struct Globals { screen: vec2f, pad: vec2f };
@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var samp: sampler;
@group(1) @binding(0) var tex: texture_2d<f32>;

struct VsOut { @builtin(position) pos: vec4f, @location(0) uv: vec2f };

// `rect` is (x0, y0, x1, y1) in screen pixels, one instance per tile; 4 vertices as a strip.
@vertex
fn vs(@builtin(vertex_index) vi: u32, @location(0) rect: vec4f) -> VsOut {
    let c = vec2f(f32(vi & 1u), f32(vi >> 1u));
    let p = mix(rect.xy, rect.zw, c);
    let ndc = vec2f(p.x / g.screen.x * 2.0 - 1.0, 1.0 - p.y / g.screen.y * 2.0);
    return VsOut(vec4f(ndc, 0.0, 1.0), c);
}

@fragment
fn fs(i: VsOut) -> @location(0) vec4f {
    let v = textureSample(tex, samp, i.uv).r;
    return vec4f(v, v, v, 1.0);
}
"#;

const BACKGROUND: wgpu::Color = wgpu::Color {
    r: 0.07,
    g: 0.07,
    b: 0.08,
    a: 1.0,
};

pub struct GpuTile {
    /// Kept so the texture's lifetime is explicit; the bind group also references it.
    _texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    pub w: u32,
    pub h: u32,
}

impl GpuTile {
    pub fn bytes(&self) -> u64 {
        u64::from(self.w) * u64::from(self.h)
    }
}

pub struct Quad<'a> {
    /// Screen-pixel rectangle `(x0, y0, x1, y1)`.
    pub rect: [f32; 4],
    pub tile: &'a GpuTile,
}

pub struct Renderer {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    globals: wgpu::Buffer,
    globals_linear: wgpu::BindGroup,
    globals_nearest: wgpu::BindGroup,
    tile_layout: wgpu::BindGroupLayout,
    instances: wgpu::Buffer,
    capacity: usize,
}

impl Renderer {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("tiles"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("tiles"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 16,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x4],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = |filter| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                mag_filter: filter,
                min_filter: filter,
                ..Default::default()
            })
        };
        let (linear, nearest) = (
            sampler(wgpu::FilterMode::Linear),
            sampler(wgpu::FilterMode::Nearest),
        );
        let layout0 = pipeline.get_bind_group_layout(0);
        let globals_group = |s: &wgpu::Sampler| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("globals"),
                layout: &layout0,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: globals.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(s),
                    },
                ],
            })
        };
        let (globals_linear, globals_nearest) = (globals_group(&linear), globals_group(&nearest));
        let tile_layout = pipeline.get_bind_group_layout(1);
        let capacity = 4096;
        let instances = Self::instance_buffer(&device, capacity);
        Self {
            device,
            queue,
            pipeline,
            globals,
            globals_linear,
            globals_nearest,
            tile_layout,
            instances,
            capacity,
        }
    }

    fn instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instances"),
            size: (capacity * 16) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    pub fn upload(&self, buf: &Gray8Buf) -> GpuTile {
        let size = wgpu::Extent3d {
            width: buf.w,
            height: buf.h,
            depth_or_array_layers: 1,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("tile"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            texture.as_image_copy(),
            &buf.data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(buf.w),
                rows_per_image: Some(buf.h),
            },
            size,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tile"),
            layout: &self.tile_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
        });
        GpuTile {
            _texture: texture,
            bind_group,
            w: buf.w,
            h: buf.h,
        }
    }

    /// Draw `quads` in order (later ones on top) into `target`.
    pub fn render(
        &mut self,
        target: &wgpu::TextureView,
        size: (u32, u32),
        nearest: bool,
        quads: &[Quad],
    ) {
        if quads.len() > self.capacity {
            self.capacity = quads.len().next_power_of_two();
            self.instances = Self::instance_buffer(&self.device, self.capacity);
        }
        let rects: Vec<[f32; 4]> = quads.iter().map(|q| q.rect).collect();
        self.queue
            .write_buffer(&self.instances, 0, bytemuck::cast_slice(&rects));
        self.queue.write_buffer(
            &self.globals,
            0,
            bytemuck::cast_slice(&[size.0 as f32, size.1 as f32, 0.0, 0.0]),
        );

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("tiles"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(BACKGROUND),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(
                0,
                if nearest {
                    &self.globals_nearest
                } else {
                    &self.globals_linear
                },
                &[],
            );
            pass.set_vertex_buffer(0, self.instances.slice(..));
            for (i, q) in quads.iter().enumerate() {
                pass.set_bind_group(1, &q.tile.bind_group, &[]);
                pass.draw(0..4, i as u32..i as u32 + 1);
            }
        }
        self.queue.submit([encoder.finish()]);
    }

    /// Render offscreen and return tightly packed RGBA8 pixels (for `--snapshot`).
    pub fn render_to_rgba(&mut self, size: (u32, u32), nearest: bool, quads: &[Quad]) -> Vec<u8> {
        let (w, h) = size;
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("snapshot"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SNAPSHOT_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.render(&view, size, nearest, quads);

        let padded = (w * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: u64::from(padded) * u64::from(h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("readback"),
            });
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        rx.recv().expect("map callback").expect("map readback");
        let mapped = buffer.slice(..).get_mapped_range().expect("mapped range");
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for row in mapped.chunks(padded as usize).take(h as usize) {
            out.extend_from_slice(&row[..(w * 4) as usize]);
        }
        out
    }
}

/// Offscreen format. Non-sRGB so grayscale values pass through unchanged (as on the window).
pub const SNAPSHOT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
