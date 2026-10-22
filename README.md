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

### Mouse

| Action | How |
|---|---|
| Add a shape | Click it in the library, then move over the view: it follows the mouse until you click again to place it (Esc cancels). Dragging it in also works. Either way it lands on whichever face is under the cursor (highlighted green), standing on that face, and snaps to the face's centre when close. Hold C to place it on the workplane instead. Can be turned off in Settings (Cruise) |
| Select | Click; Shift+click to add/remove; drag on empty space for a box selection |
| Move | Drag the object. Alt+drag moves a copy |
| Resize | White corner squares (X+Y), black side squares (one axis), top square (height). Shift = uniform, Alt = about the centre. Hover a handle to see its dimensions; they stay until you click elsewhere. Click a value, type a new size (e.g. `35`, `1 1/2`, `2cm`, `0.5in`) and press Enter, or Tab to accept it and move to the next value; Esc cancels. A handle only resizes once the mouse moves |
| Seen from below | Viewing from underneath flips the handles: the height handle and lift cone move to the bottom and work downwards |
| Raise / lower | Drag the black cone above the object |
| Rotate | Drag a curved arrow. Snaps 22.5 deg; Shift = 45 deg, Ctrl = 1 deg; Alt rotates a copy |
| Orbit | Right-drag, or Ctrl+Left-drag |
| Pan | Middle-drag, Shift+Right-drag, or Ctrl+Shift+Left-drag |
| Zoom | Wheel (towards the cursor), or + / - |

### Keys

| | |
|---|---|
| Union group / Bundle group / Intersect group | Ctrl+G / Ctrl+B / Ctrl+I |
| Ungroup | Ctrl+Shift+G |
| Hole / Solid | H / S |
| Lock or unlock | Ctrl+L |
| Hide selected / Show all | Ctrl+H / Ctrl+Shift+H |
| Align / Mirror / Measure | L / M / R |
| Drop to workplane | D |
| Fit view | F |
| Nudge (1 grid step, Shift = x10) / Raise and lower | Arrows / Ctrl+Up, Ctrl+Down |
| Duplicate / Copy / Cut / Paste / Delete | Ctrl+D / Ctrl+C / Ctrl+X / Ctrl+V / Del |
| Select all | Ctrl+A |
| Undo / Redo | Ctrl+Z / Ctrl+Y or Ctrl+Shift+Z |
| New / Open / Save / Save As | Ctrl+N / Ctrl+O / Ctrl+S / Ctrl+Shift+S |
| Export | Ctrl+E |
| Leave a tool, or clear the selection | Esc |

### Workspace settings

Click **Settings** at the bottom right. The options are:

- **Background colour, Show grid, Zoom speed**: saved for every design.
- **Units**: Metric or Imperial.
- **Scale**: millimetres, centimetres or metres; or inches or feet. Designs are always stored in millimetres, so changing units only changes what you see and type.
- **Printer / workplane**: printer presets set the workplane to that printer's bed (Bambu Lab, Prusa, Creality and others). You can also type a custom width and length. The default is 300 x 300 mm.
- **Make default**: uses the current units, scale and workplane for new designs.

### Group types

- **Union**: solids merge, holes cut. The classic Tinkercad group.
- **Bundle**: parts stay together but keep their own colours, and holes don't cut.
- **Intersect**: only the volume shared by all the solids is kept.

Select a group to switch its type in the inspector. Every group can be ungrouped again.

### Tools

- **Align (L)**: with two or more objects selected, handles appear around the selection: front / middle / back, left / middle / right, and bottom / middle / top. Hovering a handle turns it red and shows a preview of where the shapes will go; clicking applies it. Click one of the selected shapes to pin it: the handles move onto that shape, and the others align to it while it stays put. Click it again to unpin.
- **Mirror (M)**: click an arrow to flip the selection along that axis.
- **Measure (R)**: hover to see what will be picked. Click an edge, circle or flat face, then a second one. A single circle shows its radius and diameter. Two picks give the distance between them, e.g. the gap between two parallel edges of a notch, centre to centre of two holes, or face to face. It works on the finished shape, so nothing needs ungrouping.

### Import and export

- **Import**: STL, OBJ or 3MF. A multi-part 3MF comes in as a bundle of parts with their colours. Imported meshes are stored inside the `.orion3d` file. A mesh that isn't watertight can still be moved and exported, but holes won't cut it.
- **Export**: STL (single colour, for printing), 3MF (separate coloured parts, for multi-colour printing) or OBJ (with colours). You can export everything or just the selection. Holes and hidden objects are never exported.

### Shapes

Box Hole and Cylinder Hole, then Box (with rounded edges via Radius), Cylinder, Sphere, MetaFillet, Half Sphere, Cone, Pyramid, Roof, Wedge, Tube, Torus and Polygon.

**MetaFillet** recreates the retired Tinkercad shape generator, with Radius, Height and Type (Fillet or Chamfer).

Resizing a shape updates its own parameters where possible, so a rounded box keeps round corners and a MetaFillet keeps a true radius when stretched.

## The `.orion3d` file format

A zip archive containing `design.json`: the complete design as a tree.

- Each **node** has an `id`, `name`, `hole` flag, `color`, a 4x4 `transform` (column-major, millimetres), optional `locked` / `hidden` flags, and `content`.
- `content` is one of:
  - a `shape`: its type plus parameters;
  - a `group`: its `op` (`union`, `bundle` or `intersect`) plus its `children`;
  - a `mesh`: an imported mesh, whose triangles are stored as `meshes/<id>.bin` in the same archive.
- Groups store their children, never a merged mesh. The merged result (solids united, then holes subtracted) is recomputed on load, so ungrouping always works, at any depth.

The JSON is human-readable and diff-friendly. Files carry a format version; newer files are refused by older builds instead of being misread.

## Roadmap

- Workplane tool and ruler
- Multicolour unions
- Text and more shapes
- Windows installer with `.orion3d` file association
