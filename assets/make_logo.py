"""Builds the Orion3D logo: the running-man sign with an extruded "3D" badge.

Usage (from the repo root):
    python assets/make_logo.py [centre|bottom|bottom-left]

Writes assets/logo.png (1024), assets/icon256.png and assets/orion3d.ico.
Needs Pillow (pip install pillow). Edit the settings below to restyle it.
"""
import sys, os
from PIL import Image, ImageDraw, ImageFont, ImageFilter

HERE = os.path.dirname(os.path.abspath(__file__))
POSITION = sys.argv[1] if len(sys.argv) > 1 else "centre"
TEXT = "3D"
FONT = os.environ.get("ORION3D_LOGO_FONT", "/usr/share/fonts/opentype/inter/InterDisplay-BlackItalic.otf")
BADGE_WIDTH = 0.62          # fraction of the icon width
FACE_TOP, FACE_BOTTOM = (120, 200, 255), (40, 130, 230)
src = os.path.join(HERE, "logo_source.png")
out = os.path.join(HERE, "logo.png")
N = 1024
sign = Image.open(src).convert("RGBA")
canvas = Image.new("RGBA", (N, N), (0, 0, 0, 0))
if POSITION == "bottom-left":
    # Sign slightly smaller, shifted right/up to make room for the badge.
    s = int(N * 0.86)
    sign = sign.resize((s, s), Image.LANCZOS)
    canvas.alpha_composite(sign, (N - s, 0))
else:
    sign = sign.resize((N, N), Image.LANCZOS)
    canvas.alpha_composite(sign, (0, 0))

font = ImageFont.truetype(FONT, 390)
text = TEXT
bbox = font.getbbox(text)
tw, th = bbox[2] - bbox[0], bbox[3] - bbox[1]
pad = 60
layer = Image.new("RGBA", (tw + pad * 2 + 120, th + pad * 2 + 120), (0, 0, 0, 0))
d = ImageDraw.Draw(layer)
ox, oy = pad - bbox[0], pad - bbox[1]
depth = 46
# Extrusion: stacked copies down-right, darkening.
for i in range(depth, 0, -1):
    t = i / depth
    c = (int(18 + 20 * (1 - t)), int(60 + 40 * (1 - t)), int(120 + 60 * (1 - t)), 255)
    d.text((ox + i, oy + i), text, font=font, fill=c)
# Dark outline around the whole block for contrast on red and white.
alpha = layer.split()[3]
outline = alpha.filter(ImageFilter.MaxFilter(25))
ol = Image.new("RGBA", layer.size, (15, 20, 30, 255))
ol.putalpha(outline)
base = Image.new("RGBA", layer.size, (0, 0, 0, 0))
base.alpha_composite(ol)
base.alpha_composite(layer)
# Face with a vertical gradient.
face = Image.new("RGBA", layer.size, (0, 0, 0, 0))
fd = ImageDraw.Draw(face)
fd.text((ox, oy), text, font=font, fill=(255, 255, 255, 255))
grad = Image.new("RGBA", layer.size)
gd = ImageDraw.Draw(grad)
top, bot = FACE_TOP, FACE_BOTTOM
y0, y1 = oy + bbox[1], oy + bbox[3]
for y in range(layer.size[1]):
    t = min(max((y - y0) / max(1, y1 - y0), 0), 1)
    gd.line([(0, y), (layer.size[0], y)], fill=tuple(int(top[k] + (bot[k] - top[k]) * t) for k in range(3)) + (255,))
grad.putalpha(face.split()[3])
base.alpha_composite(grad)
# Thin highlight edge on the face.
hl = face.split()[3].filter(ImageFilter.FIND_EDGES)
hli = Image.new("RGBA", layer.size, (230, 245, 255, 200))
hli.putalpha(hl)
base.alpha_composite(hli)
base = base.crop(base.getbbox())
# Fit the badge in the lower-left.
w = int(N * BADGE_WIDTH)
h = int(base.size[1] * w / base.size[0])
base = base.resize((w, h), Image.LANCZOS)
# Soft shadow under the badge.
sh = Image.new("RGBA", base.size, (0, 0, 0, 0))
sh.putalpha(base.split()[3].point(lambda a: int(a * 0.45)))
sh = sh.filter(ImageFilter.GaussianBlur(14))
if POSITION == "centre":
    x, y = (N - w) // 2, (N - h) // 2
elif POSITION == "bottom":
    x, y = (N - w) // 2, N - h - int(N * 0.06)
else:
    x, y = 8, N - h - 12
canvas.alpha_composite(sh, (x + 10, y + 14))
canvas.alpha_composite(base, (x, y))
canvas.save(out)
canvas.resize((256, 256), Image.LANCZOS).save(os.path.join(HERE, "icon256.png"))
canvas.save(os.path.join(HERE, "orion3d.ico"), sizes=[(s, s) for s in (256, 128, 64, 48, 32, 24, 16)])
print("Wrote", out)
