//! Offscreen wgpu renderer. The 3D scene is drawn into a texture which
//! egui then shows as an image, so the same code also renders the shape
//! library thumbnails.

pub mod camera;

use crate::geom::EvalMesh;
use bytemuck::{Pod, Zeroable};
use egui_wgpu::wgpu;
use egui_wgpu::wgpu::util::DeviceExt;
use glam::{Mat4, Vec3};
use std::collections::HashMap;
use std::sync::Arc;

pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
pub const MSAA: u32 = 4;
const OBJ_STRIDE: u64 = 256;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    cam_pos: [f32; 4],
    light_dir: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ObjUniform {
    model: [[f32; 4]; 4],
    normal_mat: [[f32; 4]; 4],
    color: [f32; 4],
    flags: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MeshVertex {
    pos: [f32; 3],
    normal: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct LineVertex {
    pub pos: [f32; 3],
    pub color: [f32; 4],
}

pub struct DrawItem {
    pub key: u64,
    pub mesh: Arc<EvalMesh>,
    pub model: Mat4,
    pub color: [f32; 4],
    pub hole: bool,
    pub selected: bool,
    pub hovered: bool,
}

pub struct SceneParams<'a> {
    pub view_proj: Mat4,
    pub eye: Vec3,
    pub items: &'a [DrawItem],
    /// Workplane size; None for thumbnails.
    pub workplane: Option<[f32; 2]>,
    pub clear: [f64; 4],
    /// Draw the 1 mm grid lines (only when zoomed in).
    pub fine_grid: bool,
    /// Distance at which fine grid lines have faded out.
    pub grid_fade: f32,
    pub extra_lines: &'a [LineVertex],
}

struct GpuMesh {
    buf: wgpu::Buffer,
    count: u32,
    last_used: u64,
}

/// A render target: MSAA colour + depth, resolved into a texture egui can show.
pub struct Target {
    pub size: [u32; 2],
    msaa_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    resolve_view: wgpu::TextureView,
    pub texture_id: Option<egui::TextureId>,
}

impl Target {
    fn create(device: &wgpu::Device, size: [u32; 2]) -> Self {
        let extent = wgpu::Extent3d {
            width: size[0].max(1),
            height: size[1].max(1),
            depth_or_array_layers: 1,
        };
        let mk = |label, format, samples, usage| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: extent,
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        Target {
            size,
            msaa_view: mk("msaa", COLOR_FORMAT, MSAA, wgpu::TextureUsages::RENDER_ATTACHMENT),
            depth_view: mk("depth", DEPTH_FORMAT, MSAA, wgpu::TextureUsages::RENDER_ATTACHMENT),
            resolve_view: mk(
                "resolve",
                COLOR_FORMAT,
                1,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            ),
            texture_id: None,
        }
    }
}

struct Plane {
    size: [f32; 2],
    quad: wgpu::Buffer,
    major: wgpu::Buffer,
    major_count: u32,
    minor: wgpu::Buffer,
    minor_count: u32,
}

pub struct Renderer {
    mesh_pipe: wgpu::RenderPipeline,
    hole_pipe: wgpu::RenderPipeline,
    line_pipe: wgpu::RenderPipeline,
    globals_buf: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    obj_layout: wgpu::BindGroupLayout,
    obj_buf: wgpu::Buffer,
    obj_bg: wgpu::BindGroup,
    obj_capacity: u64,
    meshes: HashMap<u64, GpuMesh>,
    plane: Option<Plane>,
    frame: u64,
}

impl Renderer {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("orion3d shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let uniform_entry = |dynamic| wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: dynamic,
                min_binding_size: None,
            },
            count: None,
        };
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[uniform_entry(false)],
        });
        let obj_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("obj"),
            entries: &[uniform_entry(true)],
        });

        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals_buf.as_entire_binding() }],
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("layout"),
            bind_group_layouts: &[Some(&globals_layout), Some(&obj_layout)],
            immediate_size: 0,
        });

        let mesh_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<MeshVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3],
        })];
        let line_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<LineVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4],
        })];

        let make = |label: &str,
                    vs: &str,
                    fs: &str,
                    buffers: &[Option<wgpu::VertexBufferLayout>],
                    topology: wgpu::PrimitiveTopology,
                    depth_write: bool,
                    blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers,
                },
                primitive: wgpu::PrimitiveState {
                    topology,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(depth_write),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState { count: MSAA, ..Default::default() },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: COLOR_FORMAT,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };

        let mesh_pipe = make(
            "mesh",
            "vs_mesh",
            "fs_mesh",
            &mesh_buffers,
            wgpu::PrimitiveTopology::TriangleList,
            true,
            None,
        );
        let hole_pipe = make(
            "hole",
            "vs_mesh",
            "fs_mesh",
            &mesh_buffers,
            wgpu::PrimitiveTopology::TriangleList,
            false,
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );
        let line_pipe = make(
            "line",
            "vs_line",
            "fs_line",
            &line_buffers,
            wgpu::PrimitiveTopology::LineList,
            false,
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );

        let obj_capacity = 64;
        let (obj_buf, obj_bg) = Self::make_obj_buffer(device, &obj_layout, obj_capacity);

        Renderer {
            mesh_pipe,
            hole_pipe,
            line_pipe,
            globals_buf,
            globals_bg,
            obj_layout,
            obj_buf,
            obj_bg,
            obj_capacity,
            meshes: HashMap::new(),
            plane: None,
            frame: 0,
        }
    }

    fn make_obj_buffer(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        capacity: u64,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("objects"),
            size: capacity * OBJ_STRIDE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("objects"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buf,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<ObjUniform>() as u64),
                }),
            }],
        });
        (buf, bg)
    }

    fn upload_mesh(&mut self, device: &wgpu::Device, key: u64, mesh: &EvalMesh) {
        if let Some(m) = self.meshes.get_mut(&key) {
            m.last_used = self.frame;
            return;
        }
        let verts: Vec<MeshVertex> = mesh
            .positions
            .iter()
            .zip(&mesh.normals)
            .map(|(p, n)| MeshVertex { pos: p.to_array(), normal: n.to_array() })
            .collect();
        let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh"),
            contents: if verts.is_empty() {
                &[0u8; 24]
            } else {
                bytemuck::cast_slice(&verts)
            },
            usage: wgpu::BufferUsages::VERTEX,
        });
        self.meshes.insert(key, GpuMesh { buf, count: verts.len() as u32, last_used: self.frame });
    }

    fn ensure_plane(&mut self, device: &wgpu::Device, size: [f32; 2]) {
        if self.plane.as_ref().is_some_and(|p| p.size == size) {
            return;
        }
        let (hx, hy) = (size[0] * 0.5, size[1] * 0.5);
        let z = -0.05;
        let n = [0.0, 0.0, 1.0];
        let quad = [
            MeshVertex { pos: [-hx, -hy, z], normal: n },
            MeshVertex { pos: [hx, -hy, z], normal: n },
            MeshVertex { pos: [hx, hy, z], normal: n },
            MeshVertex { pos: [-hx, -hy, z], normal: n },
            MeshVertex { pos: [hx, hy, z], normal: n },
            MeshVertex { pos: [-hx, hy, z], normal: n },
        ];
        let plane_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("plane"),
            contents: bytemuck::cast_slice(&quad),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let mut major = Vec::new();
        let mut minor = Vec::new();
        let minor_c = [0.45, 0.64, 0.88, 0.28];
        let major_c = [0.30, 0.50, 0.82, 0.75];
        let border_c = [0.20, 0.40, 0.75, 1.0];
        let line = |v: &mut Vec<LineVertex>, a: [f32; 3], b: [f32; 3], c: [f32; 4]| {
            v.push(LineVertex { pos: a, color: c });
            v.push(LineVertex { pos: b, color: c });
        };
        let nx = size[0] as i32;
        let ny = size[1] as i32;
        for i in 0..=nx {
            let x = -hx + i as f32;
            let (dst, c) = if i == 0 || i == nx {
                (&mut major, border_c)
            } else if i % 10 == 0 {
                (&mut major, major_c)
            } else {
                (&mut minor, minor_c)
            };
            line(dst, [x, -hy, 0.0], [x, hy, 0.0], c);
        }
        for j in 0..=ny {
            let y = -hy + j as f32;
            let (dst, c) = if j == 0 || j == ny {
                (&mut major, border_c)
            } else if j % 10 == 0 {
                (&mut major, major_c)
            } else {
                (&mut minor, minor_c)
            };
            line(dst, [-hx, y, 0.0], [hx, y, 0.0], c);
        }
        let mk = |v: &Vec<LineVertex>| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("grid"),
                contents: if v.is_empty() { &[0u8; 28] } else { bytemuck::cast_slice(v) },
                usage: wgpu::BufferUsages::VERTEX,
            })
        };
        self.plane = Some(Plane {
            size,
            quad: plane_buf,
            major: mk(&major),
            major_count: major.len() as u32,
            minor: mk(&minor),
            minor_count: minor.len() as u32,
        });
    }

    pub fn create_target(&self, device: &wgpu::Device, size: [u32; 2]) -> Target {
        Target::create(device, size)
    }

    /// Render a scene into `target`, recreating it if the size changed, and
    /// make sure egui has a texture id for it.
    pub fn render(
        &mut self,
        rs: &egui_wgpu::RenderState,
        target: &mut Target,
        size: [u32; 2],
        scene: &SceneParams,
    ) -> egui::TextureId {
        let device = &rs.device;
        let queue = &rs.queue;
        self.frame += 1;

        let size = [size[0].clamp(1, 8192), size[1].clamp(1, 8192)];
        if target.size != size {
            let id = target.texture_id;
            *target = Target::create(device, size);
            target.texture_id = id;
            if let Some(id) = id {
                rs.renderer.write().update_egui_texture_from_wgpu_texture(
                    device,
                    &target.resolve_view,
                    wgpu::FilterMode::Linear,
                    id,
                );
            }
        }
        if target.texture_id.is_none() {
            target.texture_id = Some(rs.renderer.write().register_native_texture(
                device,
                &target.resolve_view,
                wgpu::FilterMode::Linear,
            ));
        }

        // Globals.
        let light = (scene.eye.normalize_or_zero() + Vec3::new(0.3, -0.2, 0.9)).normalize_or(Vec3::Z);
        let globals = Globals {
            view_proj: scene.view_proj.to_cols_array_2d(),
            cam_pos: scene.eye.extend(1.0).to_array(),
            light_dir: light.extend(scene.grid_fade).to_array(),
        };
        queue.write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(&globals));

        // Per-object uniforms (+1 slot for the workplane).
        let needed = scene.items.len() as u64 + 1;
        if needed > self.obj_capacity {
            self.obj_capacity = needed.next_power_of_two();
            let (b, g) = Self::make_obj_buffer(device, &self.obj_layout, self.obj_capacity);
            self.obj_buf = b;
            self.obj_bg = g;
        }
        let mut data = vec![0u8; (needed * OBJ_STRIDE) as usize];
        let mut write = |slot: usize, u: ObjUniform| {
            let off = slot * OBJ_STRIDE as usize;
            data[off..off + std::mem::size_of::<ObjUniform>()].copy_from_slice(bytemuck::bytes_of(&u));
        };
        for (i, it) in scene.items.iter().enumerate() {
            let normal = it.model.inverse().transpose();
            write(
                i,
                ObjUniform {
                    model: it.model.to_cols_array_2d(),
                    normal_mat: normal.to_cols_array_2d(),
                    color: it.color,
                    flags: [
                        it.hole as u8 as f32,
                        it.selected as u8 as f32,
                        it.hovered as u8 as f32,
                        0.0,
                    ],
                },
            );
        }
        let plane_slot = scene.items.len();
        write(
            plane_slot,
            ObjUniform {
                model: Mat4::IDENTITY.to_cols_array_2d(),
                normal_mat: Mat4::IDENTITY.to_cols_array_2d(),
                color: [0.86, 0.92, 0.98, 1.0],
                flags: [0.0, 0.0, 0.0, 1.0],
            },
        );
        queue.write_buffer(&self.obj_buf, 0, &data);

        for it in scene.items {
            self.upload_mesh(device, it.key, &it.mesh);
        }
        if let Some(wp) = scene.workplane {
            self.ensure_plane(device, wp);
        }
        let extra_buf = (!scene.extra_lines.is_empty()).then(|| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("extra lines"),
                contents: bytemuck::cast_slice(scene.extra_lines),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });

        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("scene") });
        {
            let c = scene.clear;
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.msaa_view,
                    depth_slice: None,
                    resolve_target: Some(&target.resolve_view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: c[0], g: c[1], b: c[2], a: c[3] }),
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.globals_bg, &[]);

            // Opaque solids.
            pass.set_pipeline(&self.mesh_pipe);
            for (i, it) in scene.items.iter().enumerate() {
                if it.hole {
                    continue;
                }
                let m = &self.meshes[&it.key];
                pass.set_bind_group(1, &self.obj_bg, &[(i as u64 * OBJ_STRIDE) as u32]);
                pass.set_vertex_buffer(0, m.buf.slice(..));
                pass.draw(0..m.count, 0..1);
            }

            // Workplane (only from above) and grid.
            if let (Some(pl), true) = (&self.plane, scene.workplane.is_some()) {
                if scene.eye.z > 0.0 {
                    pass.set_bind_group(1, &self.obj_bg, &[(plane_slot as u64 * OBJ_STRIDE) as u32]);
                    pass.set_vertex_buffer(0, pl.quad.slice(..));
                    pass.draw(0..6, 0..1);
                }
                pass.set_pipeline(&self.line_pipe);
                pass.set_bind_group(1, &self.obj_bg, &[0]);
                if scene.fine_grid {
                    pass.set_vertex_buffer(0, pl.minor.slice(..));
                    pass.draw(0..pl.minor_count, 0..1);
                }
                pass.set_vertex_buffer(0, pl.major.slice(..));
                pass.draw(0..pl.major_count, 0..1);
            }

            // Extra lines (selection boxes etc.).
            if let Some(buf) = &extra_buf {
                pass.set_pipeline(&self.line_pipe);
                pass.set_bind_group(1, &self.obj_bg, &[0]);
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw(0..scene.extra_lines.len() as u32, 0..1);
            }

            // Holes last, translucent.
            pass.set_pipeline(&self.hole_pipe);
            for (i, it) in scene.items.iter().enumerate() {
                if !it.hole {
                    continue;
                }
                let m = &self.meshes[&it.key];
                pass.set_bind_group(1, &self.obj_bg, &[(i as u64 * OBJ_STRIDE) as u32]);
                pass.set_vertex_buffer(0, m.buf.slice(..));
                pass.draw(0..m.count, 0..1);
            }
        }
        queue.submit([enc.finish()]);

        // Evict GPU meshes unused for a while.
        if self.frame % 300 == 0 {
            let cutoff = self.frame.saturating_sub(600);
            self.meshes.retain(|_, m| m.last_used >= cutoff);
        }
        target.texture_id.unwrap()
    }
}
