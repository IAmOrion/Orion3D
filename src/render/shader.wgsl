struct Globals {
    view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,
    light_dir: vec4<f32>,
};
@group(0) @binding(0) var<uniform> g: Globals;

struct Obj {
    model: mat4x4<f32>,
    normal_mat: mat4x4<f32>,
    color: vec4<f32>,
    // x: hole, y: selected, z: hovered, w: unlit
    flags: vec4<f32>,
};
@group(1) @binding(0) var<uniform> o: Obj;

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
    if (o.flags.x > 0.5) {
        // Holes: translucent grey with diagonal stripes, like Tinkercad.
        let s = fract((i.clip.x + i.clip.y) / 16.0);
        let grey = select(0.62, 0.78, s < 0.5);
        base = vec3<f32>(grey, grey, grey);
        alpha = 0.45;
    }
    var col = base * light;
    let spec = pow(max(dot(reflect(-l, n), v), 0.0), 32.0) * 0.12;
    col = col + vec3<f32>(spec, spec, spec);

    let rim = pow(1.0 - head, 4.0);
    if (o.flags.y > 0.5) {
        col = mix(col, vec3<f32>(0.25, 0.60, 1.0), clamp(rim * 0.5, 0.0, 0.35));
    } else if (o.flags.z > 0.5) {
        col = col * 1.12 + vec3<f32>(0.03, 0.04, 0.06);
    }
    return vec4<f32>(col, alpha);
}

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
    out.clip = g.view_proj * vec4<f32>(v.pos, 1.0);
    out.color = v.color;
    out.world = v.pos;
    return out;
}

@fragment
fn fs_line(i: LineOut) -> @location(0) vec4<f32> {
    // Fade grid lines with distance so they never turn into moire.
    // light_dir.w holds the fade distance for fine lines; coarse lines
    // (alpha >= 0.5) fade much further out.
    let d = distance(g.cam_pos.xyz, i.world);
    var fade_at = g.light_dir.w;
    if (i.color.a >= 0.5) {
        fade_at = fade_at * 5.0;
    }
    let k = clamp(1.0 - (d - fade_at) / (fade_at * 0.6), 0.0, 1.0);
    return vec4<f32>(i.color.rgb, i.color.a * k);
}
