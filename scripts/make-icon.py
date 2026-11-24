"""Draws the app icon (the orbit mark) into assets/icon.ico, icon-256.png and tray-32.png.

Run from the repository root: python scripts/make-icon.py
Needs Pillow. Large sizes show three orbits around a planet; small sizes keep one orbit.
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

SPACE = (11, 13, 26)
HULL = (27, 32, 64)
EDGE = (52, 59, 106)
ION = (92, 225, 230)
SOLAR = (255, 184, 107)
NEBULA = (183, 156, 255)
STAR = (238, 240, 255)

SCALE = 4  # draw big, then shrink: smooth edges


def rounded_background(size: int) -> Image.Image:
    big = size * SCALE
    image = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    mask = Image.new("L", (big, big), 0)
    radius = int(big * 0.23)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, big - 1, big - 1), radius=radius, fill=255)
    gradient = Image.new("RGBA", (big, big))
    pixels = gradient.load()
    for y in range(big):
        for x in range(big):
            t = (x + y) / (2 * big)
            pixels[x, y] = tuple(int(HULL[i] * (1 - t) + SPACE[i] * t) for i in range(3)) + (255,)
    image.paste(gradient, (0, 0), mask)
    border = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    ImageDraw.Draw(border).rounded_rectangle(
        (0, 0, big - 1, big - 1), radius=radius, outline=EDGE + (255,), width=max(SCALE, big // 96)
    )
    return Image.alpha_composite(image, border)


def orbit(draw: ImageDraw.ImageDraw, center: float, radius: float, width: float, color, sweep: float):
    box = (center - radius, center - radius, center + radius, center + radius)
    draw.ellipse(box, outline=color + (40,), width=int(width))
    draw.arc(box, start=-90, end=-90 + sweep, fill=color + (255,), width=int(width))


def satellite(layer: Image.Image, center: float, radius: float, angle_deg: float, size: float, glow):
    import math

    angle = math.radians(angle_deg)
    x = center + radius * math.cos(angle)
    y = center + radius * math.sin(angle)
    halo = Image.new("RGBA", layer.size, (0, 0, 0, 0))
    ImageDraw.Draw(halo).ellipse((x - size * 2, y - size * 2, x + size * 2, y + size * 2), fill=glow + (150,))
    halo = halo.filter(ImageFilter.GaussianBlur(size))
    layer.alpha_composite(halo)
    ImageDraw.Draw(layer).ellipse((x - size, y - size, x + size, y + size), fill=STAR + (255,))


def planet(layer: Image.Image, center: float, radius: float):
    disc = Image.new("RGBA", layer.size, (0, 0, 0, 0))
    draw = ImageDraw.Draw(disc)
    steps = 40
    for i in range(steps):
        t = i / steps
        r = radius * (1 - t)
        color = tuple(int(59 + (143 - 59) * t) if k == 0 else int(63 + (160 - 63) * t) if k == 1 else int(143 + (255 - 143) * t) for k in range(3))
        draw.ellipse((center - r, center - r, center + r, center + r), fill=color + (255,))
    layer.alpha_composite(disc)


def full_icon(size: int) -> Image.Image:
    base = rounded_background(size)
    big = size * SCALE
    c = big / 2
    layer = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    draw = ImageDraw.Draw(layer)
    w = big * 0.045
    orbit(draw, c, big * 0.36, w, ION, 300)
    orbit(draw, c, big * 0.27, w, SOLAR, 215)
    orbit(draw, c, big * 0.18, w, NEBULA, 130)
    satellite(layer, c, big * 0.36, -90 + 300, w * 0.95, ION)
    satellite(layer, c, big * 0.27, -90 + 215, w * 0.95, SOLAR)
    satellite(layer, c, big * 0.18, -90 + 130, w * 0.95, NEBULA)
    planet(layer, c, big * 0.09)
    return Image.alpha_composite(base, layer).resize((size, size), Image.LANCZOS)


def small_icon(size: int) -> Image.Image:
    base = rounded_background(size)
    big = size * SCALE
    c = big / 2
    layer = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    draw = ImageDraw.Draw(layer)
    w = big * 0.11
    orbit(draw, c, big * 0.31, w, ION, 290)
    planet(layer, c, big * 0.15)
    return Image.alpha_composite(base, layer).resize((size, size), Image.LANCZOS)


def main():
    out = Path("crates/app/assets")
    out.mkdir(parents=True, exist_ok=True)
    sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
    images = {s: (small_icon(s) if s <= 24 else full_icon(s)) for s in sizes}
    images[256].save(out / "icon-256.png")
    small_icon(32).save(out / "tray-32.png")
    images[256].save(out / "icon.ico", sizes=[(s, s) for s in sizes], append_images=[images[s] for s in sizes if s != 256])
    print(f"wrote {out / 'icon.ico'} and {out / 'icon-256.png'}")


if __name__ == "__main__":
    main()
