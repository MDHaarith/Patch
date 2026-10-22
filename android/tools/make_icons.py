#!/usr/bin/env python3
"""Generates the adaptive launcher icon's foreground layer (the glowing check
of assets/images/icon-1024.png) for every density. The background is a
gradient drawable and the monochrome layer a vector, both in res/drawable.

    python3 android/tools/make_icons.py
"""
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

RES = Path(__file__).resolve().parents[1] / "app/src/main/res"
DENSITIES = {"mdpi": 1, "hdpi": 1.5, "xhdpi": 2, "xxhdpi": 3, "xxxhdpi": 4}
BLUE = (10, 132, 255)
# The check in the 108dp adaptive canvas, measured from icon-1024.png (whose
# 888px rounded square maps onto the 72dp visible area).
POINTS = [(37.4, 55.5), (49.0, 67.2), (71.4, 42.3)]
STROKE = 7.1


def stroke(size, scale, width, color):
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    pts = [(x * scale, y * scale) for x, y in POINTS]
    w = width * scale
    d.line(pts, fill=color, width=round(w), joint="curve")
    for x, y in (pts[0], pts[-1]):
        d.ellipse([x - w / 2, y - w / 2, x + w / 2, y + w / 2], fill=color)
    return img


def foreground(px):
    ss = 4  # supersample for smooth edges
    size = px * ss
    scale = size / 108
    glow = stroke(size, scale, STROKE * 1.6, BLUE + (150,)).filter(ImageFilter.GaussianBlur(2.6 * scale))
    img = Image.alpha_composite(glow, stroke(size, scale, STROKE, BLUE + (255,)))
    return img.resize((px, px), Image.LANCZOS)


for name, d in DENSITIES.items():
    out = RES / f"mipmap-{name}"
    out.mkdir(parents=True, exist_ok=True)
    foreground(round(108 * d)).save(out / "ic_launcher_foreground.png", optimize=True)
    print("wrote", out / "ic_launcher_foreground.png")
