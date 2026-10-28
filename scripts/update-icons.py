#!/usr/bin/env python3
"""Add Patch's calendar and regenerate the launcher and Linux icons."""
from pathlib import Path
from PIL import Image, ImageDraw

root = Path(__file__).resolve().parent.parent

def calendar(image, x, y, pixel):
    draw = ImageDraw.Draw(image)
    def rect(x0, y0, x1, y1, color):
        draw.rectangle((x + x0 * pixel, y + y0 * pixel, x + x1 * pixel - 1, y + y1 * pixel - 1), fill=color)
    rect(0, 0, 5, 5, '#2c1713')
    rect(1, 1, 4, 4, '#fff0db')
    rect(1, 1, 4, 2, '#e94b40')
    rect(2, 2, 3, 3, '#e94b40')
    rect(1, 4, 4, 5, '#d7c2ad')

source = root / 'assets/images/icon-1024.png'
icon = Image.open(source).convert('RGBA')
calendar(icon, 680, 600, 40)
icon.save(source)
icon.resize((256, 256), Image.Resampling.LANCZOS).save(root / 'assets/images/icon.png')
for size in (16, 22, 24, 32, 48, 64, 128, 256, 512):
    icon.resize((size, size), Image.Resampling.LANCZOS).save(root / f'packaging/linux/icons/hicolor/{size}x{size}/apps/com.mdhaarith.Patch.png')
foreground = root / 'android/app/src/main/res/mipmap-xxxhdpi/ic_launcher_foreground.png'
image = Image.open(foreground).convert('RGBA')
calendar(image, 275, 247, 14)
for density, size in [('mdpi', 108), ('hdpi', 162), ('xhdpi', 216), ('xxhdpi', 324), ('xxxhdpi', 432)]:
    image.resize((size, size), Image.Resampling.NEAREST).save(root / f'android/app/src/main/res/mipmap-{density}/ic_launcher_foreground.png')
assert Image.open(source).size == (1024, 1024)

monochrome = root / 'android/app/src/main/res/drawable/ic_launcher_monochrome.xml'
xml = monochrome.read_text()
if 'M69,61h18v18h-18z' not in xml:
    monochrome.write_text(xml.replace('M57,31h3v3h-3z', 'M69,61h18v18h-18zM57,31h3v3h-3z', 1))
