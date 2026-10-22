//! Offscreen wgpu renderer. The 3D scene is drawn into a texture which
//! egui then shows as an image, so the same code also renders the shape
//! library thumbnails.
//!
//! Passes: shaded scene (MSAA) -> object-id buffer -> outline composite
//! (dark silhouettes plus the blue selection outline, like Tinkercad).

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
pub const ID_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Uint;
pub const MSAA: u32 = 4;
const OBJ_STRIDE: u64 = 256;
const SEL_BIT: u32 = 0x8000_0000;
const HOV_BIT: u32 = 0x4000_0000;

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
    ids: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct OutlineParams {
    dark: [f32; 4],
    select: [f32; 4],
    hover: [f32; 4],
    misc: [f32; 4],
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
    /// Linear RGBA.
    pub color: [f32; 4],
    pub hole: bool,
    pub selected: bool,
    pub hovered: bool,
    /// Items with the same group number share one silhouette (e.g. the
    /// parts of a bundle still outline separately; a union is one item).
    pub group: u32,
    /// Translucent preview (e.g. where Align will move a shape).
    pub ghost: bool,
}

pub struct SceneParams<'a> {
    pub view_proj: Mat4,
    pub eye: Vec3,
    pub items: &'a [DrawItem],
    /// Workplane size; None for thumbnails.
    pub workplane: Option<[f32; 2]>,
    pub clear: [f64; 4],
    /// Draw the minor grid lines (only when zoomed in).
    pub fine_grid: bool,
    /// Grid spacing in mm (minor, major); None hides the grid.
    pub grid: Option<(f32, f32)>,
    /// Distance at which fine grid lines have faded out.
    pub grid_fade: f32,
    pub extra_lines: &'a [LineVertex],
    /// Device pixels per UI point (outline thickness scales with it).
    pub ppp: f32,
    /// Where the workplane is (identity = the default workplane).
    pub plane_model: Mat4,
    /// Tint the workplane to show a user workplane is active.
    pub plane_custom: bool,
    /// Selection footprint on the floor (drawn translucent, under objects).
    pub footprint: Option<[Vec3; 4]>,
}

struct GpuMesh {
    buf: wgpu::Buffer,
    count: u32,
    edges: wgpu::Buffer,
    edge_count: u32,
    last_used: u64,
}

/// A render target: MSAA colour + depth, resolved into a texture egui can
/// show, plus the id buffer used for outlines.
pub struct Target {
    pub size: [u32; 2],
    msaa_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    resolve_view: wgpu::TextureView,
    id_view: wgpu::TextureView,
    id_depth_view: wgpu::TextureView,
    outline_bg: Option<wgpu::BindGroup>,
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
        let ra = wgpu::TextureUsages::RENDER_ATTACHMENT;
        Target {
            size,
            msaa_view: mk("msaa", COLOR_FORMAT, MSAA, ra),
            depth_view: mk("depth", DEPTH_FORMAT, MSAA, ra),
            resolve_view: mk("resolve", COLOR_FORMAT, 1, ra | wgpu::TextureUsages::TEXTURE_BINDING),
            id_view: mk("ids", ID_FORMAT, 1, ra | wgpu::TextureUsages::TEXTURE_BINDING),
            id_depth_view: mk("id depth", DEPTH_FORMAT, 1, ra),
            outline_bg: None,
            texture_id: None,
        }
    }
}

struct Plane {
    key: [f32; 4],
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
    edge_pipe: wgpu::RenderPipeline,
    id_pipe: wgpu::RenderPipeline,
    outline_pipe: wgpu::RenderPipeline,
    outline_layout: wgpu::BindGroupLayout,
    outline_buf: wgpu::Buffer,
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

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

impl Renderer {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("orion3d shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let uniform_entry = |binding, dynamic| wgpu::BindGroupLayoutEntry {
            binding,
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
            entries: &[uniform_entry(0, false)],
        });
        let obj_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("obj"),
            entries: &[uniform_entry(0, true)],
        });
        let outline_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("outline"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                uniform_entry(1, false),
            ],
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
        let outline_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("outline params"),
            size: std::mem::size_of::<OutlineParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("layout"),
            bind_group_layouts: &[Some(&globals_layout), Some(&obj_layout)],
            immediate_size: 0,
        });
        let outline_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("outline layout"),
            bind_group_layouts: &[Some(&outline_layout)],
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
        let edge_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: 12,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3],
        })];

        struct P<'a> {
            label: &'a str,
            layout: &'a wgpu::PipelineLayout,
            vs: &'a str,
            fs: &'a str,
            buffers: &'a [Option<wgpu::VertexBufferLayout<'a>>],
            topology: wgpu::PrimitiveTopology,
            depth: Option<bool>,
            blend: Option<wgpu::BlendState>,
            format: wgpu::TextureFormat,
            samples: u32,
        }
        let make = |p: P| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(p.label),
                layout: Some(p.layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(p.vs),
                    compilation_options: Default::default(),
                    buffers: p.buffers,
                },
                primitive: wgpu::PrimitiveState { topology: p.topology, cull_mode: None, ..Default::default() },
                depth_stencil: p.depth.map(|write| wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(write),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState { count: p.samples, ..Default::default() },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(p.fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: p.format,
                        blend: p.blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let tri = wgpu::PrimitiveTopology::TriangleList;
        let lines = wgpu::PrimitiveTopology::LineList;
        let alpha = Some(wgpu::BlendState::ALPHA_BLENDING);

        let mesh_pipe = make(P { label: "mesh", layout: &layout, vs: "vs_mesh", fs: "fs_mesh", buffers: &mesh_buffers, topology: tri, depth: Some(true), blend: None, format: COLOR_FORMAT, samples: MSAA });
        let hole_pipe = make(P { label: "hole", layout: &layout, vs: "vs_mesh", fs: "fs_mesh", buffers: &mesh_buffers, topology: tri, depth: Some(false), blend: alpha, format: COLOR_FORMAT, samples: MSAA });
        let line_pipe = make(P { label: "line", layout: &layout, vs: "vs_line", fs: "fs_line", buffers: &line_buffers, topology: lines, depth: Some(false), blend: alpha, format: COLOR_FORMAT, samples: MSAA });
        let edge_pipe = make(P { label: "edge", layout: &layout, vs: "vs_edge", fs: "fs_edge", buffers: &edge_buffers, topology: lines, depth: Some(false), blend: alpha, format: COLOR_FORMAT, samples: MSAA });
        let id_pipe = make(P { label: "id", layout: &layout, vs: "vs_id", fs: "fs_id", buffers: &mesh_buffers, topology: tri, depth: Some(true), blend: None, format: ID_FORMAT, samples: 1 });
        let outline_pipe = make(P { label: "outline", layout: &outline_pl, vs: "vs_full", fs: "fs_outline", buffers: &[], topology: tri, depth: None, blend: alpha, format: COLOR_FORMAT, samples: 1 });

        let obj_capacity = 64;
        let (obj_buf, obj_bg) = Self::make_obj_buffer(device, &obj_layout, obj_capacity);

        Renderer {
            mesh_pipe,
            hole_pipe,
            line_pipe,
            edge_pipe,
            id_pipe,
            outline_pipe,
            outline_layout,
            outline_buf,
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
            contents: if verts.is_empty() { &[0u8; 24] } else { bytemuck::cast_slice(&verts) },
            usage: wgpu::BufferUsages::VERTEX,
        });
        let edge_pts: Vec<[f32; 3]> = mesh.edges.iter().flat_map(|e| [e[0].to_array(), e[1].to_array()]).collect();
        let edges = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("edges"),
            contents: if edge_pts.is_empty() { &[0u8; 24] } else { bytemuck::cast_slice(&edge_pts) },
            usage: wgpu::BufferUsages::VERTEX,
        });
        self.meshes.insert(
            key,
            GpuMesh {
                buf,
                count: verts.len() as u32,
                edges,
                edge_count: edge_pts.len() as u32,
                last_used: self.frame,
            },
        );
    }

    fn ensure_plane(&mut self, device: &wgpu::Device, size: [f32; 2], grid: (f32, f32)) {
        let key = [size[0], size[1], grid.0, grid.1];
        if self.plane.as_ref().is_some_and(|p| p.key == key) {
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
        let quad = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("plane"),
            contents: bytemuck::cast_slice(&quad),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let mut major: Vec<LineVertex> = Vec::new();
        let mut minor: Vec<LineVertex> = Vec::new();
        let minor_c = [0.45, 0.64, 0.88, 0.28];
        let major_c = [0.30, 0.50, 0.82, 0.75];
        let border_c = [0.20, 0.40, 0.75, 1.0];
        let line = |v: &mut Vec<LineVertex>, a: [f32; 3], b: [f32; 3], c: [f32; 4]| {
            v.push(LineVertex { pos: a, color: c });
            v.push(LineVertex { pos: b, color: c });
        };
        // Lines start at the centre so the origin is always on a line.
        let (step, big) = (grid.0.max(0.01), grid.1.max(0.01));
        let per = (big / step).round().max(1.0) as i64;
        let mut push_lines = |axis: usize| {
            let (half, other) = if axis == 0 { (hx, hy) } else { (hy, hx) };
            let n = (half / step).floor() as i64;
            let fine = n * 2 <= 4000;
            for i in -n..=n {
                let t = i as f32 * step;
                let is_major = i % per == 0;
                if !is_major && !fine {
                    continue;
                }
                let (a, b) = if axis == 0 { ([t, -other, 0.0], [t, other, 0.0]) } else { ([-other, t, 0.0], [other, t, 0.0]) };
                if is_major {
                    line(&mut major, a, b, major_c);
                } else {
                    line(&mut minor, a, b, minor_c);
                }
            }
            // Border.
            let (a0, b0, a1, b1) = if axis == 0 {
                ([-half, -other, 0.0], [-half, other, 0.0], [half, -other, 0.0], [half, other, 0.0])
            } else {
                ([-other, -half, 0.0], [other, -half, 0.0], [-other, half, 0.0], [other, half, 0.0])
            };
            line(&mut major, a0, b0, border_c);
            line(&mut major, a1, b1, border_c);
        };
        push_lines(0);
        push_lines(1);
        let mk = |v: &Vec<LineVertex>| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("grid"),
                contents: if v.is_empty() { &[0u8; 28] } else { bytemuck::cast_slice(v) },
                usage: wgpu::BufferUsages::VERTEX,
            })
        };
        self.plane = Some(Plane {
            key,
            quad,
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
        if target.outline_bg.is_none() {
            target.outline_bg = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("outline"),
                layout: &self.outline_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&target.id_view) },
                    wgpu::BindGroupEntry { binding: 1, resource: self.outline_buf.as_entire_binding() },
                ],
            }));
        }

        // Globals.
        let light = (scene.eye.normalize_or_zero() + Vec3::new(0.3, -0.2, 0.9)).normalize_or(Vec3::Z);
        let globals = Globals {
            view_proj: scene.view_proj.to_cols_array_2d(),
            cam_pos: scene.eye.extend(1.0).to_array(),
            light_dir: light.extend(scene.grid_fade).to_array(),
        };
        queue.write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(&globals));

        let any_selected = scene.items.iter().any(|i| i.selected);
        let lin = |r: u8, g: u8, b: u8, a: f32| {
            [srgb_to_linear(r as f32 / 255.0), srgb_to_linear(g as f32 / 255.0), srgb_to_linear(b as f32 / 255.0), a]
        };
        let outline = OutlineParams {
            dark: lin(28, 34, 44, 0.9),
            select: lin(79, 195, 247, 1.0),
            hover: lin(92, 186, 238, 0.9),
            misc: [(2.5 * scene.ppp).round().max(3.0), any_selected as u8 as f32, 0.0, 0.0],
        };
        queue.write_buffer(&self.outline_buf, 0, bytemuck::bytes_of(&outline));

        // Per-object uniforms (+1 slot for the workplane, +1 footprint).
        let needed = scene.items.len() as u64 + 2;
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
            let mut id = (it.group + 1) & 0x3fff_ffff;
            if it.selected {
                id |= SEL_BIT;
            }
            if it.hovered {
                id |= HOV_BIT;
            }
            write(
                i,
                ObjUniform {
                    model: it.model.to_cols_array_2d(),
                    normal_mat: normal.to_cols_array_2d(),
                    color: it.color,
                    flags: [
                        if it.ghost { 2.0 } else { it.hole as u8 as f32 },
                        it.selected as u8 as f32,
                        it.hovered as u8 as f32,
                        0.0,
                    ],
                    ids: [id, 0, 0, 0],
                },
            );
        }
        let plane_slot = scene.items.len();
        write(
            plane_slot,
            ObjUniform {
                model: scene.plane_model.to_cols_array_2d(),
                normal_mat: scene.plane_model.inverse().transpose().to_cols_array_2d(),
                color: if scene.plane_custom { [0.99, 0.80, 0.55, 0.30] } else { [0.86, 0.92, 0.98, 1.0] },
                flags: [0.0, 0.0, 0.0, 1.0],
                ids: [0; 4],
            },
        );
        let foot_slot = plane_slot + 1;
        write(
            foot_slot,
            ObjUniform {
                model: Mat4::IDENTITY.to_cols_array_2d(),
                normal_mat: Mat4::IDENTITY.to_cols_array_2d(),
                color: [0.22, 0.36, 0.60, 0.28],
                flags: [0.0, 0.0, 0.0, 1.0],
                ids: [0; 4],
            },
        );
        queue.write_buffer(&self.obj_buf, 0, &data);
        let foot_buf = scene.footprint.map(|q| {
            let n = [0.0, 0.0, 1.0];
            let v = |p: Vec3| MeshVertex { pos: (p + Vec3::Z * 0.02).to_array(), normal: n };
            let quad = [v(q[0]), v(q[1]), v(q[2]), v(q[0]), v(q[2]), v(q[3])];
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("footprint"),
                contents: bytemuck::cast_slice(&quad),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });

        for it in scene.items {
            self.upload_mesh(device, it.key, &it.mesh);
        }
        if let Some(wp) = scene.workplane {
            self.ensure_plane(device, wp, scene.grid.unwrap_or((1.0, 10.0)));
        }
        let extra_buf = (!scene.extra_lines.is_empty()).then(|| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("extra lines"),
                contents: bytemuck::cast_slice(scene.extra_lines),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });
        let off = |i: usize| (i as u64 * OBJ_STRIDE) as u32;

        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("scene") });

        // 1. Shaded scene.
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
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
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
                if it.hole || it.ghost {
                    continue;
                }
                let m = &self.meshes[&it.key];
                pass.set_bind_group(1, &self.obj_bg, &[off(i)]);
                pass.set_vertex_buffer(0, m.buf.slice(..));
                pass.draw(0..m.count, 0..1);
            }

            // Workplane (only from above) and grid.
            if let (Some(pl), true) = (&self.plane, scene.workplane.is_some()) {
                if !scene.plane_custom && scene.plane_model.inverse().transform_point3(scene.eye).z > 0.0 {
                    pass.set_bind_group(1, &self.obj_bg, &[off(plane_slot)]);
                    pass.set_vertex_buffer(0, pl.quad.slice(..));
                    pass.draw(0..6, 0..1);
                }
                if scene.grid.is_some() {
                    pass.set_pipeline(&self.line_pipe);
                    pass.set_bind_group(1, &self.obj_bg, &[off(plane_slot)]);
                    if scene.fine_grid {
                        pass.set_vertex_buffer(0, pl.minor.slice(..));
                        pass.draw(0..pl.minor_count, 0..1);
                    }
                    pass.set_vertex_buffer(0, pl.major.slice(..));
                    pass.draw(0..pl.major_count, 0..1);
                }
            }

            if let Some(buf) = &extra_buf {
                pass.set_pipeline(&self.line_pipe);
                pass.set_bind_group(1, &self.obj_bg, &[off(foot_slot)]);
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw(0..scene.extra_lines.len() as u32, 0..1);
            }

            // Hard edges of solids.
            pass.set_pipeline(&self.edge_pipe);
            for (i, it) in scene.items.iter().enumerate() {
                if it.hole || it.ghost {
                    continue;
                }
                let m = &self.meshes[&it.key];
                if m.edge_count > 0 {
                    pass.set_bind_group(1, &self.obj_bg, &[off(i)]);
                    pass.set_vertex_buffer(0, m.edges.slice(..));
                    pass.draw(0..m.edge_count, 0..1);
                }
            }

            // Holes last, translucent; the footprint first, under everything.
            pass.set_pipeline(&self.hole_pipe);
            // A user workplane is see-through, so shapes behind it stay visible.
            if let (Some(pl), true) = (&self.plane, scene.plane_custom && scene.workplane.is_some()) {
                pass.set_bind_group(1, &self.obj_bg, &[off(plane_slot)]);
                pass.set_vertex_buffer(0, pl.quad.slice(..));
                pass.draw(0..6, 0..1);
            }
            if let Some(buf) = &foot_buf {
                pass.set_bind_group(1, &self.obj_bg, &[off(foot_slot)]);
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw(0..6, 0..1);
            }
            for (i, it) in scene.items.iter().enumerate() {
                if !it.hole && !it.ghost {
                    continue;
                }
                let m = &self.meshes[&it.key];
                pass.set_bind_group(1, &self.obj_bg, &[off(i)]);
                pass.set_vertex_buffer(0, m.buf.slice(..));
                pass.draw(0..m.count, 0..1);
            }
        }

        // 2. Object ids.
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ids"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.id_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.id_depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.globals_bg, &[]);
            pass.set_pipeline(&self.id_pipe);
            for (i, it) in scene.items.iter().enumerate() {
                if it.ghost {
                    continue;
                }
                let m = &self.meshes[&it.key];
                pass.set_bind_group(1, &self.obj_bg, &[off(i)]);
                pass.set_vertex_buffer(0, m.buf.slice(..));
                pass.draw(0..m.count, 0..1);
            }
        }

        // 3. Outlines on top of the resolved image.
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("outline"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.resolve_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.outline_pipe);
            pass.set_bind_group(0, target.outline_bg.as_ref().unwrap(), &[]);
            pass.draw(0..3, 0..1);
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
