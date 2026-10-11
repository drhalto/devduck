"""Build DevDuck's icon and web assets from the two keyed mascot images.

Sources (transparent, 1024x1024, generated with gpt-image-2 and chroma-keyed):
  assets/brand/devduck-1024.png         the duck holding a network plug (main mascot)
  assets/brand/devduck-thumbs-1024.png  the duck with headphones, thumbs up
  assets/brand/devduck.svg              the vector mascot (exported from devduck.vectorcraft
                                        in VectorCraft), copied to docs/img for the site

Writes:
  assets/brand/icon.png        1024 app icon: duck head on a blue rounded tile
  assets/brand/devduck.ico     Windows icon, 16-256 px (embedded into the exes by build.rs)
  assets/brand/icon-64.rgba    raw RGBA for the window icon (no PNG decoder needed at runtime)
  assets/brand/logo-96.rgba    raw RGBA for the in-app logo
  docs/img/*, docs/favicon.png, docs/apple-touch-icon.png, docs/img/og.png (the GitHub Pages site)

usage: python scripts/make-brand.py   (needs Pillow)
"""
import shutil
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
BRAND = ROOT / "assets" / "brand"
DOCS = ROOT / "docs"
IMG = DOCS / "img"
IMG.mkdir(parents=True, exist_ok=True)

duck = Image.open(BRAND / "devduck-1024.png").convert("RGBA")
thumbs = Image.open(BRAND / "devduck-thumbs-1024.png").convert("RGBA")


def gradient(size, top, bottom):
    w, h = size
    img = Image.new("RGBA", size)
    px = img.load()
    for y in range(h):
        f = y / max(h - 1, 1)
        c = tuple(round(top[i] + (bottom[i] - top[i]) * f) for i in range(3)) + (255,)
        for x in range(w):
            px[x, y] = c
    return img


def rounded_mask(size, radius_frac):
    s = size * 4
    big = Image.new("L", (s, s), 0)
    ImageDraw.Draw(big).rounded_rectangle((0, 0, s - 1, s - 1), radius=int(s * radius_frac), fill=255)
    return big.resize((size, size), Image.LANCZOS)


# App icon: the duck's head (the plug is cropped out; it turns to noise at 16 px)
# on a blue "water" tile.
S = 1024
head = duck.crop((200, 8, 744, 552)).resize((int(S * 1.02), int(S * 1.02)), Image.LANCZOS)
tile = gradient((S, S), (0x4C, 0x8D, 0xFF), (0x2A, 0x55, 0xC9))
tile.alpha_composite(head, (int(S * 0.02), int(S * 0.06)))
icon = Image.new("RGBA", (S, S), (0, 0, 0, 0))
icon.paste(tile, (0, 0), rounded_mask(S, 0.23))
icon.save(BRAND / "icon.png")

sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
icon.save(BRAND / "devduck.ico", sizes=[(s, s) for s in sizes])
(BRAND / "icon-64.rgba").write_bytes(icon.resize((64, 64), Image.LANCZOS).tobytes())
(BRAND / "logo-96.rgba").write_bytes(icon.resize((96, 96), Image.LANCZOS).tobytes())

# Web: favicon, touch icon, mascots.
icon.resize((64, 64), Image.LANCZOS).save(DOCS / "favicon.png")
icon.resize((180, 180), Image.LANCZOS).save(DOCS / "apple-touch-icon.png")
icon.resize((192, 192), Image.LANCZOS).save(IMG / "icon-192.png")


def trimmed(img, width):
    img = img.crop(img.getbbox())
    h = round(img.height * width / img.width)
    return img.resize((width, h), Image.LANCZOS)


shutil.copyfile(BRAND / "devduck.svg", IMG / "devduck.svg")
trimmed(thumbs, 360).save(IMG / "devduck-thumbs.png", optimize=True)

# Social card (1200x630) for link previews.
W, H = 1200, 630
card = gradient((W, H), (0x10, 0x13, 0x1B), (0x0B, 0x0D, 0x12))
glow = Image.new("RGBA", (W, H), (0, 0, 0, 0))
ImageDraw.Draw(glow).ellipse((700, 40, 1220, 560), fill=(0x4C, 0x8D, 0xFF, 38))
card.alpha_composite(glow)
mascot = trimmed(duck, 430)
card.alpha_composite(mascot, (W - mascot.width - 70, (H - mascot.height) // 2))
fonts = ROOT / "assets" / "fonts"
title = ImageFont.truetype(str(fonts / "Geist-SemiBold.ttf"), 108)
body = ImageFont.truetype(str(fonts / "Geist-Regular.ttf"), 36)
mono = ImageFont.truetype(str(fonts / "GeistMono-Medium.ttf"), 30)
d = ImageDraw.Draw(card)
d.text((80, 150), "DevDuck", font=title, fill=(0xE7, 0xEA, 0xF3))
for i, line in enumerate(["Every dev server on your PC,", "grouped by project.", "Open it, restart it, stop it."]):
    d.text((84, 282 + i * 46), line, font=body, fill=(0xA4, 0xAC, 0xC2))
d.rounded_rectangle((84, 450, 470, 506), radius=12, fill=(0x1A, 0x1E, 0x28), outline=(0x33, 0x3A, 0x4B), width=2)
d.text((106, 461), "> devduck who 3000", font=mono, fill=(0xFF, 0xD6, 0x0A))
card.convert("RGB").save(IMG / "og.png", optimize=True)

for f in ["screenshot.png", "screenshot-list.png"]:
    src = ROOT / f
    if src.exists():
        Image.open(src).save(IMG / f, optimize=True)
print("brand assets written")
