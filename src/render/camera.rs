//! Z-up orbit camera.

use glam::{Mat4, Vec3, Vec4Swizzles};

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub target: Vec3,
    /// Radians around Z.
    pub yaw: f32,
    /// Radians above the XY plane.
    pub pitch: f32,
    pub dist: f32,
    pub fov_y: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self::home()
    }
}

impl Camera {
    pub fn home() -> Self {
        Camera {
            target: Vec3::new(0.0, 0.0, 0.0),
            yaw: (-115f32).to_radians(),
            pitch: 32f32.to_radians(),
            dist: 320.0,
            fov_y: 35f32.to_radians(),
        }
    }

    pub fn eye(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        self.target + Vec3::new(cp * cy, cp * sy, sp) * self.dist
    }

    pub fn view(&self) -> Mat4 {
        let up = if self.pitch.abs() > 1.55 {
            // Looking straight down/up: use yaw to orient the screen.
            let (sy, cy) = self.yaw.sin_cos();
            Vec3::new(-cy, -sy, 0.0) * self.pitch.signum()
        } else {
            Vec3::Z
        };
        glam::camera::rh::view::look_at_mat4(self.eye(), self.target, up)
    }

    pub fn proj(&self, aspect: f32) -> Mat4 {
        let near = (self.dist * 0.01).max(0.05);
        let far = self.dist * 8.0 + 1500.0;
        glam::camera::rh::proj::directx::perspective(self.fov_y, aspect.max(0.01), near, far)
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        self.proj(aspect) * self.view()
    }

    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx * 0.008;
        self.pitch = (self.pitch + dy * 0.008).clamp(-1.5707, 1.5707);
    }

    /// Pan by a screen-space delta (pixels), keeping the point under the
    /// cursor under the cursor.
    pub fn pan(&mut self, dx: f32, dy: f32, viewport_h: f32) {
        let world_per_px = 2.0 * self.dist * (self.fov_y * 0.5).tan() / viewport_h.max(1.0);
        let view = self.view();
        let right = view.row(0).xyz();
        let up = view.row(1).xyz();
        self.target += (-right * dx + up * dy) * world_per_px;
    }

    /// Zoom towards a world point (the point under the cursor).
    pub fn zoom(&mut self, factor: f32, towards: Option<Vec3>) {
        let new_dist = (self.dist * factor).clamp(2.0, 20000.0);
        if let Some(p) = towards {
            let k = 1.0 - new_dist / self.dist;
            self.target += (p - self.target) * k;
        }
        self.dist = new_dist;
    }

    /// Ray through a viewport pixel. `uv` is 0..1 across the viewport.
    pub fn ray(&self, uv: [f32; 2], aspect: f32) -> (Vec3, Vec3) {
        let inv = self.view_proj(aspect).inverse();
        let x = uv[0] * 2.0 - 1.0;
        let y = 1.0 - uv[1] * 2.0;
        let near = inv.project_point3(Vec3::new(x, y, 0.0));
        let far = inv.project_point3(Vec3::new(x, y, 1.0));
        (near, (far - near).normalize())
    }

    /// Project a world point to 0..1 viewport coordinates. None if behind.
    pub fn project(&self, p: Vec3, aspect: f32) -> Option<[f32; 2]> {
        let c = self.view_proj(aspect) * p.extend(1.0);
        if c.w <= 1e-5 {
            return None;
        }
        let n = c.xyz() / c.w;
        Some([(n.x + 1.0) * 0.5, (1.0 - n.y) * 0.5])
    }

    /// Frame a bounding box.
    pub fn fit(&mut self, lo: Vec3, hi: Vec3) {
        let center = (lo + hi) * 0.5;
        let radius = ((hi - lo).length() * 0.5).max(5.0);
        self.target = center;
        // Assume a landscape viewport, so the vertical FOV is the limit.
        self.dist = radius / (self.fov_y * 0.5).sin() * 1.6;
    }
}
