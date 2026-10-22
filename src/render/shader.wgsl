struct Globals {
    view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,
    // xyz: key light direction, w: fine grid fade distance
    light_dir: vec4<f32>,
};
@group(0) @binding(0) var<uniform> g: Globals;

struct Obj {
    model: mat4x4<f32>,
    normal_mat: mat4x4<f32>,
    color: vec4<f32>,
    // x: hole, y: selected, z: hovered, w: unlit
    flags: vec4<f32>,
    // x: object id for the outline pass
    ids: vec4<u32>,
};
@group(1) @binding(0) var<uniform> o: Obj;

// ---------------------------------------------------------------------------
// Shaded meshes
// ---------------------------------------------------------------------------

struct MeshIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
};

struct MeshOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
};

@vertex
fn vs_mesh(v: MeshIn) -> MeshOut {
    var out: MeshOut;
    let w = o.model * vec4<f32>(v.pos, 1.0);
    out.clip = g.view_proj * w;
    out.world = w.xyz;
    out.normal = (o.normal_mat * vec4<f32>(v.normal, 0.0)).xyz;
    return out;
}

@fragment
fn fs_mesh(i: MeshOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    if (o.flags.w > 0.5) {
        return o.color;
    }
    var n = normalize(i.normal);
    if (!front) {
        n = -n;
    }
    let v = normalize(g.cam_pos.xyz - i.world);
    let l = normalize(g.light_dir.xyz);
    let key = max(dot(n, l), 0.0);
    let head = max(dot(n, v), 0.0);
    let sky = mix(0.30, 0.46, n.z * 0.5 + 0.5);
    let light = sky + key * 0.45 + head * 0.30;

    var base = o.color.rgb;
    var alpha = o.color.a;
    if (o.flags.x > 1.5) {
        // Ghost preview: pale, translucent version of the shape.
        let c = mix(base * light, vec3<f32>(1.0, 1.0, 1.0), 0.45);
        return vec4<f32>(c, 0.55);
    }
    if (o.flags.x > 0.5) {
        // Holes: translucent grey with diagonal stripes, like Tinkercad.
        let s = fract((i.clip.x + i.clip.y) / 16.0);
        let grey = select(0.62, 0.80, s < 0.5);
        base = vec3<f32>(grey, grey, grey);
        alpha = 0.5;
    }
    var col = base * light;
    let spec = pow(max(dot(reflect(-l, n), v), 0.0), 32.0) * 0.10;
    col = col + vec3<f32>(spec, spec, spec);
    if (o.flags.z > 0.5 && o.flags.y < 0.5) {
        col = col * 1.10 + vec3<f32>(0.02, 0.03, 0.05);
    }
    return vec4<f32>(col, alpha);
}

// ---------------------------------------------------------------------------
// Object id pass (for silhouettes and the selection outline)
// ---------------------------------------------------------------------------

@vertex
fn vs_id(v: MeshIn) -> @builtin(position) vec4<f32> {
    return g.view_proj * (o.model * vec4<f32>(v.pos, 1.0));
}

@fragment
fn fs_id() -> @location(0) u32 {
    return o.ids.x;
}

// ---------------------------------------------------------------------------
// Hard edges of objects (dark lines, pulled slightly towards the camera)
// ---------------------------------------------------------------------------

@vertex
fn vs_edge(@location(0) pos: vec3<f32>) -> @builtin(position) vec4<f32> {
    var c = g.view_proj * (o.model * vec4<f32>(pos, 1.0));
    c.z = c.z - 0.00004 * c.w;
    return c;
}

@fragment
fn fs_edge() -> @location(0) vec4<f32> {
    if (o.flags.x > 0.5) {
        return vec4<f32>(0.35, 0.37, 0.40, 0.55);
    }
    return vec4<f32>(o.color.rgb * 0.12 + vec3<f32>(0.02, 0.025, 0.03), 0.9);
}

// ---------------------------------------------------------------------------
// Lines (grid)
// ---------------------------------------------------------------------------

struct LineIn {
    @location(0) pos: vec3<f32>,
    @location(1) color: vec4<f32>,
};

struct LineOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) world: vec3<f32>,
};

@vertex
fn vs_line(v: LineIn) -> LineOut {
    var out: LineOut;
    let w = o.model * vec4<f32>(v.pos, 1.0);
    out.clip = g.view_proj * w;
    // Push the grid slightly behind surfaces lying on the workplane, so a
    // shape's bottom face never flickers with grid lines (seen from below).
    if (g.cam_pos.z < 0.0) {
        out.clip.z = out.clip.z + 0.00004 * out.clip.w;
    }
    out.color = v.color;
    out.world = w.xyz;
    return out;
}

@fragment
fn fs_line(i: LineOut) -> @location(0) vec4<f32> {
    // Fade grid lines with distance so they never turn into moire.
    // Coarse lines (alpha >= 0.5) fade much further out.
    let d = distance(g.cam_pos.xyz, i.world);
    var fade_at = g.light_dir.w;
    if (i.color.a >= 0.5) {
        fade_at = fade_at * 5.0;
    }
    let k = clamp(1.0 - (d - fade_at) / (fade_at * 0.6), 0.0, 1.0);
    return vec4<f32>(i.color.rgb, i.color.a * k);
}

// ---------------------------------------------------------------------------
// Outline composite (full screen)
// ---------------------------------------------------------------------------

struct OutlineParams {
    dark: vec4<f32>,
    select: vec4<f32>,
    hover: vec4<f32>,
    // x: selection outline radius (px), y: any selected (0/1)
    misc: vec4<f32>,
};
@group(0) @binding(0) var idtex: texture_2d<u32>;
@group(0) @binding(1) var<uniform> op: OutlineParams;

const SEL: u32 = 0x80000000u;
const HOV: u32 = 0x40000000u;
const MASK: u32 = 0x3fffffffu;

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4<f32>(x * 2.0 - 1.0, y * 2.0 - 1.0, 0.0, 1.0);
}

fn id_at(p: vec2<i32>) -> u32 {
    let dim = vec2<i32>(textureDimensions(idtex));
    let q = clamp(p, vec2<i32>(0, 0), dim - vec2<i32>(1, 1));
    return textureLoad(idtex, q, 0).x;
}

@fragment
fn fs_outline(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = vec2<i32>(pos.xy);
    let id = id_at(c);
    let low = id & MASK;

    // Selection glow outside selected objects.
    if ((id & SEL) == 0u && op.misc.y > 0.5) {
        let r = i32(op.misc.x);
        var best = 1e9;
        for (var dy = -r; dy <= r; dy = dy + 1) {
            for (var dx = -r; dx <= r; dx = dx + 1) {
                if ((id_at(c + vec2<i32>(dx, dy)) & SEL) != 0u) {
                    best = min(best, f32(dx * dx + dy * dy));
                }
            }
        }
        if (best <= op.misc.x * op.misc.x + 0.5) {
            return op.select;
        }
    }

    // Thin dark silhouette where object ids change.
    if (low != 0u) {
        let n = array<vec2<i32>, 4>(vec2<i32>(1, 0), vec2<i32>(-1, 0), vec2<i32>(0, 1), vec2<i32>(0, -1));
        for (var k = 0; k < 4; k = k + 1) {
            let nid = id_at(c + n[k]);
            if ((nid & MASK) != low) {
                if ((id & HOV) != 0u) {
                    return op.hover;
                }
                return op.dark;
            }
        }
    }
    discard;
}
