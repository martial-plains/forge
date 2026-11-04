// SPDX-License-Identifier: MIT OR Apache-2.0
//! Forge's canvas on the GPU, with wgpu.
//!
//! [`forge_canvas`] describes a drawing as a [`Scene`]; this draws it with
//! the machine's graphics API (Vulkan, Metal, Direct3D 12 or OpenGL) so one
//! renderer serves every frontend: the GNOME and Windows apps show
//! its pictures, and so can the macOS app and the terminal. Shapes are
//! tessellated (lyon) and drawn with 4× multisampling, text comes from a
//! glyph atlas, pictures are textures.
//!
//! * [`Renderer::headless`] picks an adapter on its own and renders to
//!   pixels ([`Renderer::render_rgba`], [`Renderer::render_png`]).
//! * [`Renderer::new`] uses a device an app already has, and
//!   [`Renderer::draw`] draws into any texture view of it.
//!
//! The pixels match the CPU rasteriser's ([`forge_canvas::raster`]) to
//! within anti-aliasing; the differences are listed in [`batch`]. When
//! there is no adapter (a server with no GPU and no software Vulkan)
//! [`Renderer::headless`] says so and callers fall back to the CPU.

pub mod atlas;
pub mod batch;

use std::collections::HashMap;
use std::sync::Arc;

use batch::{Batch, Draw, Pictures, Source, Stop, Vertex};
use forge_canvas::Scene;
use wgpu::util::DeviceExt;

const SHADER: &str = include_str!("shader.wgsl");
const SAMPLES: u32 = 4;
/// The format pictures are drawn in. Not sRGB: blending happens on the
/// colour numbers as written, like the CPU rasteriser does.
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// RGBA pixels, straight (not premultiplied) alpha, top row first.
#[derive(Debug, Clone, PartialEq)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Pixels {
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = ((y * self.width + x) * 4) as usize;
        Some([self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]])
    }

    pub fn png(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        let img = image::RgbaImage::from_raw(self.width, self.height, self.rgba.clone()).ok_or("pixel buffer has the wrong size")?;
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).map_err(|e| e.to_string())?;
        Ok(out)
    }
}

struct Decoded {
    width: u32,
    height: u32,
    /// Premultiplied.
    rgba: Vec<u8>,
}

#[derive(Default)]
struct PictureCache {
    decoded: HashMap<String, Option<Arc<Decoded>>>,
}

impl PictureCache {
    fn get(&mut self, path: &str) -> Option<Arc<Decoded>> {
        self.decoded
            .entry(path.to_string())
            .or_insert_with(|| {
                let img = image::open(path).ok()?.to_rgba8();
                let (width, height) = img.dimensions();
                if width == 0 || height == 0 || width > 8192 || height > 8192 {
                    return None;
                }
                let mut rgba = img.into_raw();
                for p in rgba.chunks_exact_mut(4) {
                    let a = u16::from(p[3]);
                    for c in &mut p[..3] {
                        *c = ((u16::from(*c) * a + 127) / 255) as u8;
                    }
                }
                Some(Arc::new(Decoded { width, height, rgba }))
            })
            .clone()
    }
}

impl Pictures for PictureCache {
    fn size(&mut self, path: &str) -> Option<(u32, u32)> {
        self.get(path).map(|d| (d.width, d.height))
    }
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    samples: u32,
    frame_layout: wgpu::BindGroupLayout,
    frame_buffer: wgpu::Buffer,
    frame_group: wgpu::BindGroup,
    atlas: atlas::Atlas,
    atlas_texture: wgpu::Texture,
    atlas_group: wgpu::BindGroup,
    picture_layout: wgpu::BindGroupLayout,
    blank_group: wgpu::BindGroup,
    sampler: wgpu::Sampler,
    pictures: PictureCache,
    picture_groups: HashMap<String, wgpu::BindGroup>,
    uploaded_generation: u32,
    /// The adapter's name, for logs and `canvas-gpu-info`.
    pub adapter: String,
}

impl Renderer {
    /// Open the best adapter there is and render to pixels.
    pub fn headless() -> Result<Renderer, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            apply_limit_buckets: false,
        }))
        .map_err(|e| format!("no graphics adapter: {e}"))?;
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor { label: Some("forge canvas"), ..Default::default() }))
            .map_err(|e| format!("can't open the graphics device: {e}"))?;
        let mut r = Renderer::new(device, queue, FORMAT)?;
        r.adapter = format!("{} ({:?})", info.name, info.backend);
        Ok(r)
    }

    /// Use a device the app already has. `format` is what [`Renderer::draw`]
    /// will draw into; it must not be an sRGB format.
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, format: wgpu::TextureFormat) -> Result<Renderer, String> {
        if format.is_srgb() {
            return Err("draw into a non-sRGB format (Rgba8Unorm or Bgra8Unorm)".into());
        }
        let samples = if format.guaranteed_format_features(device.features()).flags.sample_count_supported(SAMPLES) { SAMPLES } else { 1 };
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("forge canvas"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let texture_entries = [
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                count: None,
            },
            wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
        ];
        let atlas_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("atlas"), entries: &texture_entries });
        let picture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("picture"), entries: &texture_entries });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("forge canvas"),
            bind_group_layouts: &[Some(&frame_layout), Some(&atlas_layout), Some(&picture_layout)],
            immediate_size: 0,
        });
        let attrs = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32];
        let premultiplied = wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("forge canvas"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<Vertex>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attrs })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: Some(premultiplied), write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState { count: samples, mask: !0, alpha_to_coverage_enabled: false },
            multiview_mask: None,
            cache: None,
        });
        let frame_buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("frame"), size: 16, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let frame_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame"),
            layout: &frame_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: frame_buffer.as_entire_binding() }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let atlas_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyphs"),
            size: wgpu::Extent3d { width: atlas::SIZE, height: atlas::SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let atlas_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("atlas"),
            layout: &atlas_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&atlas_texture.create_view(&Default::default())) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });
        let blank = device.create_texture_with_data(
            &queue,
            &wgpu::TextureDescriptor {
                label: Some("blank"),
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &[0, 0, 0, 0],
        );
        let blank_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blank"),
            layout: &picture_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&blank.create_view(&Default::default())) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });
        Ok(Renderer {
            device,
            queue,
            pipeline,
            samples,
            frame_layout,
            frame_buffer,
            frame_group,
            atlas: atlas::Atlas::default(),
            atlas_texture,
            atlas_group,
            picture_layout,
            blank_group,
            sampler,
            pictures: PictureCache::default(),
            picture_groups: HashMap::new(),
            uploaded_generation: 0,
            adapter: String::new(),
        })
    }

    /// Forget decoded pictures (after files changed on disk).
    pub fn forget_pictures(&mut self) {
        self.pictures.decoded.clear();
        self.picture_groups.clear();
    }

    /// Render `scene` at `scale` device pixels per unit to pixels.
    pub fn render_rgba(&mut self, scene: &Scene, scale: f32) -> Result<Pixels, String> {
        let batch = self.build(scene, scale)?;
        let (w, h) = (batch.width, batch.height);
        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("canvas"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.record(&mut encoder, &batch, &view);
        let row = (w * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let out = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: u64::from(row) * u64::from(h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &target, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &out, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit([encoder.finish()]);
        let slice = out.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| format!("the GPU did not finish: {e}"))?;
        rx.recv().map_err(|e| e.to_string())?.map_err(|e| format!("can't read the picture back: {e}"))?;
        let data = slice.get_mapped_range().map_err(|e| format!("can't read the picture back: {e}"))?;
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h as usize {
            rgba.extend_from_slice(&data[y * row as usize..y * row as usize + (w * 4) as usize]);
        }
        drop(data);
        out.unmap();
        // The target holds premultiplied colour; hand back straight alpha.
        for p in rgba.chunks_exact_mut(4) {
            let a = u32::from(p[3]);
            if a != 0 && a != 255 {
                for c in &mut p[..3] {
                    *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        Ok(Pixels { width: w, height: h, rgba })
    }

    pub fn render_png(&mut self, scene: &Scene, scale: f32) -> Result<Vec<u8>, String> {
        self.render_rgba(scene, scale)?.png()
    }

    /// Draw `scene` into `view`, a `size` texture of this renderer's format
    /// (the scene is placed at the top-left at `scale`).
    pub fn draw(&mut self, scene: &Scene, scale: f32, view: &wgpu::TextureView, size: (u32, u32)) -> Result<(), String> {
        let mut batch = self.build(scene, scale)?;
        batch.width = size.0.max(1);
        batch.height = size.1.max(1);
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.record(&mut encoder, &batch, view);
        self.queue.submit([encoder.finish()]);
        Ok(())
    }

    fn build(&mut self, scene: &Scene, scale: f32) -> Result<Batch, String> {
        for attempt in 0..2 {
            match batch::build(scene, scale, &mut self.atlas, &mut self.pictures) {
                Ok(b) => return Ok(b),
                Err(Stop::AtlasFull) if attempt == 0 => self.atlas.clear(),
                Err(Stop::AtlasFull) => return Err("too many different glyphs for one picture".into()),
            }
        }
        unreachable!()
    }

    fn picture_group(&mut self, path: &str) -> Option<&wgpu::BindGroup> {
        if !self.picture_groups.contains_key(path) {
            let d = self.pictures.get(path)?;
            let texture = self.device.create_texture_with_data(
                &self.queue,
                &wgpu::TextureDescriptor {
                    label: Some("picture"),
                    size: wgpu::Extent3d { width: d.width, height: d.height, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                &d.rgba,
            );
            let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("picture"),
                layout: &self.picture_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&texture.create_view(&Default::default())) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                ],
            });
            self.picture_groups.insert(path.to_string(), group);
        }
        self.picture_groups.get(path)
    }

    fn record(&mut self, encoder: &mut wgpu::CommandEncoder, batch: &Batch, view: &wgpu::TextureView) {
        if self.atlas.dirty || self.uploaded_generation != self.atlas.generation {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &self.atlas_texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                &self.atlas.pixels,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(atlas::SIZE), rows_per_image: Some(atlas::SIZE) },
                wgpu::Extent3d { width: atlas::SIZE, height: atlas::SIZE, depth_or_array_layers: 1 },
            );
            self.atlas.dirty = false;
            self.uploaded_generation = self.atlas.generation;
        }
        let (w, h) = (batch.width, batch.height);
        self.queue.write_buffer(&self.frame_buffer, 0, bytemuck_cast(&[w as f32, h as f32, 0.0, 0.0]));
        // Make the picture bind groups first; the pass borrows them.
        for d in &batch.draws {
            if let Source::Picture(p) = &d.source {
                self.picture_group(p);
            }
        }
        let vertices = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("vertices"), contents: bytemuck::cast_slice(&batch.vertices), usage: wgpu::BufferUsages::VERTEX });
        let indices = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("indices"), contents: bytemuck::cast_slice(&batch.indices), usage: wgpu::BufferUsages::INDEX });
        let bg = batch.background.map_or(wgpu::Color::TRANSPARENT, |c| wgpu::Color { r: f64::from(c[0]), g: f64::from(c[1]), b: f64::from(c[2]), a: f64::from(c[3]) });
        let msaa = (self.samples > 1).then(|| {
            self.device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("multisample"),
                    size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: self.samples,
                    dimension: wgpu::TextureDimension::D2,
                    format: FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        });
        let (attachment, resolve) = match &msaa {
            Some(m) => (m, Some(view)),
            None => (view, None),
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("forge canvas"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: attachment,
                depth_slice: None,
                resolve_target: resolve,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(bg), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if !batch.draws.is_empty() {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.frame_group, &[]);
            pass.set_bind_group(1, &self.atlas_group, &[]);
            pass.set_vertex_buffer(0, vertices.slice(..));
            pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
            for Draw { source, scissor, first, count } in &batch.draws {
                let [x0, y0, x1, y1] = *scissor;
                let (x0, y0, x1, y1) = (x0.min(w), y0.min(h), x1.min(w), y1.min(h));
                if x1 <= x0 || y1 <= y0 {
                    continue;
                }
                pass.set_scissor_rect(x0, y0, x1 - x0, y1 - y0);
                match source {
                    Source::Plain => pass.set_bind_group(2, &self.blank_group, &[]),
                    Source::Picture(p) => pass.set_bind_group(2, self.picture_groups.get(p).unwrap_or(&self.blank_group), &[]),
                }
                pass.draw_indexed(*first..*first + *count, 0, 0..1);
            }
        }
        let _ = &self.frame_layout;
    }
}

fn bytemuck_cast(v: &[f32; 4]) -> &[u8] {
    bytemuck::cast_slice(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_canvas::{Op, Stroke, Transform};

    #[test]
    fn the_shader_is_valid_wgsl() {
        let module = naga::front::wgsl::parse_str(SHADER).expect("parses");
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&module).expect("validates");
    }

    /// The tests below need an adapter (a GPU, or software Vulkan such as
    /// lavapipe); without one they say so and pass.
    fn renderer() -> Option<Renderer> {
        match Renderer::headless() {
            Ok(r) => Some(r),
            Err(e) => {
                eprintln!("no adapter, skipping: {e}");
                None
            }
        }
    }

    fn near(a: [u8; 4], b: [u8; 4], tol: i32) -> bool {
        a.iter().zip(&b).all(|(x, y)| (i32::from(*x) - i32::from(*y)).abs() <= tol)
    }

    #[test]
    fn shapes_land_where_they_should() {
        let Some(mut r) = renderer() else { return };
        let mut s = Scene::new(40.0, 20.0).background("#ffffff");
        s.push(Op::rect(0.0, 0.0, 10.0, 10.0).fill("#ff0000"));
        s.push(Op::ellipse(30.0, 10.0, 8.0, 8.0).fill("#0000ff"));
        let p = r.render_rgba(&s, 1.0).unwrap();
        assert_eq!((p.width, p.height), (40, 20));
        assert!(near(p.pixel(5, 5).unwrap(), [255, 0, 0, 255], 2), "{:?}", p.pixel(5, 5));
        assert!(near(p.pixel(30, 10).unwrap(), [0, 0, 255, 255], 2));
        assert!(near(p.pixel(20, 18).unwrap(), [255, 255, 255, 255], 2));
        assert_eq!(p.pixel(99, 0), None);
    }

    #[test]
    fn transparent_background_and_blending() {
        let Some(mut r) = renderer() else { return };
        let mut s = Scene::new(10.0, 10.0);
        s.push(Op::rect(0.0, 0.0, 5.0, 5.0).fill("rgba(0,255,0,0.5)"));
        let p = r.render_rgba(&s, 2.0).unwrap();
        assert_eq!((p.width, p.height), (20, 20));
        let px = p.pixel(2, 2).unwrap();
        assert!(px[1] > 250 && (120..136).contains(&px[3]), "{px:?}");
        assert_eq!(p.pixel(15, 15), Some([0, 0, 0, 0]));
        assert!(p.png().unwrap().starts_with(&[0x89, b'P', b'N', b'G']));
    }

    #[test]
    fn groups_clip_translate_and_fade() {
        let Some(mut r) = renderer() else { return };
        let mut s = Scene::new(40.0, 40.0).background("#ffffff");
        s.push(Op::group(vec![Op::rect(0.0, 0.0, 40.0, 40.0).fill("#000000")]).transformed(Transform::translate(10.0, 10.0)).clipped(0.0, 0.0, 10.0, 10.0));
        let p = r.render_rgba(&s, 1.0).unwrap();
        assert!(near(p.pixel(15, 15).unwrap(), [0, 0, 0, 255], 2));
        assert!(near(p.pixel(25, 25).unwrap(), [255, 255, 255, 255], 2), "outside the clip");
        assert!(near(p.pixel(5, 5).unwrap(), [255, 255, 255, 255], 2));
        let mut f = Scene::new(10.0, 10.0).background("#ffffff");
        f.push(Op::group(vec![Op::rect(0.0, 0.0, 10.0, 10.0).fill("#000000")]).faded(0.5));
        let g = r.render_rgba(&f, 1.0).unwrap().pixel(5, 5).unwrap();
        assert!((120..136).contains(&g[0]), "{g:?}");
    }

    #[test]
    fn strokes_dashes_gradients_and_arrows() {
        let Some(mut r) = renderer() else { return };
        let mut d = Scene::new(40.0, 10.0).background("#ffffff");
        d.push(Op::line(0.0, 5.0, 40.0, 5.0, Stroke::new("#000000", 4.0).dashed(5.0, 5.0)));
        let p = r.render_rgba(&d, 1.0).unwrap();
        assert!(p.pixel(2, 5).unwrap()[0] < 10);
        assert!(p.pixel(7, 5).unwrap()[0] > 245, "the gap");
        let mut g = Scene::new(100.0, 10.0);
        g.push(Op::rect(0.0, 0.0, 100.0, 10.0).fill_linear(0.0, 0.0, 100.0, 0.0, &[(0.0, "#000000"), (1.0, "#ffffff")]));
        let p = r.render_rgba(&g, 1.0).unwrap();
        assert!(p.pixel(2, 5).unwrap()[0] < 30 && p.pixel(97, 5).unwrap()[0] > 225);
        let mid = p.pixel(50, 5).unwrap()[0];
        assert!((110..146).contains(&mid), "{mid}");
        let mut a = Scene::new(60.0, 20.0).background("#ffffff");
        a.push(Op::line(0.0, 10.0, 50.0, 10.0, Stroke::new("#000000", 4.0).arrow()));
        let p = r.render_rgba(&a, 1.0).unwrap();
        assert!(p.pixel(38, 6).unwrap()[0] < 128, "the arrowhead is wider than the shaft");
    }

    #[test]
    fn text_draws_when_a_font_is_installed() {
        let Some(mut r) = renderer() else { return };
        if !forge_canvas::raster::has_fonts() {
            return;
        }
        let mut s = Scene::new(120.0, 40.0).background("#ffffff");
        s.push(Op::text(4.0, 4.0, "Hello", 20.0, "#000000"));
        let p = r.render_rgba(&s, 1.0).unwrap();
        let dark = (0..120).flat_map(|x| (0..40).map(move |y| (x, y))).filter(|&(x, y)| p.pixel(x, y).unwrap()[0] < 128).count();
        assert!(dark > 60, "ink was drawn: {dark}");
    }

    #[test]
    fn pictures_are_drawn_and_missing_ones_outlined() {
        let Some(mut r) = renderer() else { return };
        let dir = std::env::temp_dir().join(format!("forge-gpu-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("red.png");
        image::RgbaImage::from_pixel(4, 4, image::Rgba([255, 0, 0, 255])).save(&path).unwrap();
        let mut s = Scene::new(30.0, 30.0).background("#ffffff");
        s.push(Op::image(path.to_str().unwrap(), 5.0, 5.0, 20.0, 20.0));
        let p = r.render_rgba(&s, 1.0).unwrap();
        assert!(near(p.pixel(15, 15).unwrap(), [255, 0, 0, 255], 3), "{:?}", p.pixel(15, 15));
        assert!(near(p.pixel(2, 2).unwrap(), [255, 255, 255, 255], 2));
        let mut m = Scene::new(30.0, 30.0).background("#ffffff");
        m.push(Op::image("/no/such/file.png", 5.0, 5.0, 20.0, 20.0));
        assert!(r.render_rgba(&m, 1.0).is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn it_agrees_with_the_cpu_rasteriser() {
        let Some(mut r) = renderer() else { return };
        let mut s = Scene::new(80.0, 60.0).background("#202030");
        s.push(Op::rect(5.0, 5.0, 40.0, 30.0).fill("#ff8800").stroked(Stroke::new("#ffffff", 2.0)).rounded(6.0));
        s.push(Op::ellipse(60.0, 40.0, 14.0, 10.0).fill("#44ccaa"));
        s.push(Op::path("M5 55 L40 45 L75 55").stroked(Stroke::new("#ff00ff", 3.0)));
        let gpu = r.render_rgba(&s, 1.0).unwrap();
        let cpu = forge_canvas::raster::render(&s, 1.0).unwrap();
        let mut worse = 0;
        for y in 0..60 {
            for x in 0..80 {
                let (a, b) = (gpu.pixel(x, y).unwrap(), cpu.pixel(x, y).unwrap());
                if !near(a, b, 60) {
                    worse += 1;
                }
            }
        }
        // Edges are anti-aliased differently; the interiors must agree.
        assert!(worse < 120, "{worse} of 4800 pixels differ");
    }
}
