# Orion3D

Offline desktop 3D modelling in the spirit of Tinkercad. Your designs are saved as local `.orion3d` files, and every group can always be ungrouped back to the shapes that made it.

Built in Rust: [egui](https://github.com/emilk/egui) for the interface, [wgpu](https://wgpu.rs) for rendering (DirectX 12 on Windows, Metal on macOS, Vulkan on Linux), and [Manifold](https://github.com/elalish/manifold) for solid/hole booleans.

## Download

Every push to `main` builds Windows, macOS and Linux versions. Open the repository's **Actions** tab, pick the latest **Build** run and download the artifact for your system (e.g. `Orion3D-windows-x64`).

Tagged versions (`v0.1.0` etc.) are also published under **Releases**.

## Build from source (Windows)

One-off setup, from PowerShell terminal:

```
winget install Rustlang.Rustup
winget install Git.Git
winget install Kitware.CMake
winget install Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

Close and reopen PowerShell, then from PowerShell terminal:

```
git clone https://github.com/IAmOrion/Orion3D.git
cd Orion3D
cargo run --release
```

The first build takes several minutes (it compiles the geometry kernel). Later builds are quick.

## Using it

| Action | How |
|---|---|
| Add a shape | Click it in the library (adds at the view centre) or drag it onto the workplane |
| Select | Click; Shift+click to add/remove; drag on empty space for a box selection |
| Move | Drag the object. Arrow keys nudge (Shift = x10), Ctrl+Up/Down lifts |
| Resize | White corner squares (X+Y, Shift = uniform), black side squares (one axis), top square (height) |
| Raise / lower | Drag the black cone above the object |
| Rotate | Drag a curved arrow. Snaps 22.5 deg; Shift = 45 deg, Ctrl = 1 deg |
| Orbit / pan / zoom | Right-drag / middle-drag or Shift+right-drag / wheel (zooms towards the cursor) |
| Solid / Hole | Inspector buttons, or S / H |
| Group / Ungroup | Ctrl+G / Ctrl+Shift+G |
| Duplicate / Copy / Paste | Ctrl+D / Ctrl+C / Ctrl+V |
| Undo / Redo | Ctrl+Z / Ctrl+Y |
| Drop to workplane | D |
| Fit view | F |
| Save / Save As / Open / New | Ctrl+S / Ctrl+Shift+S / Ctrl+O / Ctrl+N |
| Export STL | Ctrl+E (exports the selection, or everything if nothing is selected; holes are never exported) |

The snap grid is set at the bottom right.

### Shapes

Box (with rounded edges via Radius), Cylinder, Sphere, Half Sphere, Cone, Pyramid, Roof, Wedge, Tube, Torus, Polygon, and **MetaFillet**: a recreation of the retired Tinkercad shape generator, with Radius, Height and Type (Fillet or Chamfer).

Resizing a shape updates its own parameters where possible, so a rounded box keeps round corners and a MetaFillet keeps a true radius when stretched.

## The `.orion3d` file format

A zip archive containing `design.json`: the complete design as a tree.

- Each **node** has an `id`, `name`, `hole` flag, `color`, a 4x4 `transform` (column-major, millimetres) and `content`.
- `content` is either a `shape` (type plus its parameters) or a `group` (a list of child nodes).
- Groups store their children, never a merged mesh. The merged result (solids united, then holes subtracted) is recomputed on load, so ungrouping always works, at any depth.

The JSON is human-readable and diff-friendly. Files carry a format version; newer files are refused by older builds instead of being misread.

## Roadmap

- Align tool, ruler, workplane tool
- Multicolour groups
- STL / OBJ / 3MF import, 3MF export
- Text and more shapes
- Windows installer with `.orion3d` file association
