"""Make every raster brand asset from the vector logo in docs/assets/logo:

  packaging/mirrorly.ico, mirrorly-256.png    the app icon (16-256 px)
  packaging/wizard-*.bmp                      the installer's pictures
  crates/app/assets/{logo,mark}-{light,dark}.png   for Mirrorly's own window
  crates/display/src/apple-touch-icon.png     the iPad's Home Screen icon

The icon uses the mark's lifted titanium (mark-dark.svg): it reads on light and
dark taskbars alike, where the logo's darkest titanium would vanish on a dark
one. The SVGs are rendered by headless Edge (present on every Windows).

Run from anywhere:  python packaging/make_icon.py
"""

import subprocess
import tempfile
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
LOGO = ROOT / "docs" / "assets" / "logo"
PACKAGING = ROOT / "packaging"
APP_ASSETS = ROOT / "crates" / "app" / "assets"
EDGE = r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"

# Titanium & Ice.
DEEP = (20, 24, 28)  # the darkest titanium, behind the mark


def render(svg: Path, width: int, height: int) -> Image.Image:
    """The SVG at this size, on transparency, as a browser draws it in <img>."""
    with tempfile.TemporaryDirectory() as tmp:
        page = Path(tmp) / "page.html"
        shot = Path(tmp) / "shot.png"
        page.write_text(
            f"<!doctype html><body style='margin:0;background:transparent'>"
            f"<img src='{svg.as_uri()}' style='display:block;width:{width}px;height:{height}px'>",
            encoding="utf-8",
        )
        subprocess.run(
            [EDGE, "--headless=new", "--disable-gpu", "--hide-scrollbars", "--allow-file-access-from-files",
             "--default-background-color=00000000", f"--window-size={width},{height}", f"--screenshot={shot}", page.as_uri()],
            check=True, capture_output=True,
        )
        return Image.open(shot).convert("RGBA").copy()


def on_square(mark: Image.Image, size: int, fill: float) -> Image.Image:
    """The mark centred in a transparent square, `fill` of its width."""
    w = round(size * fill)
    h = round(w * mark.height / mark.width)
    canvas = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    canvas.alpha_composite(mark.resize((w, h), Image.LANCZOS), ((size - w) // 2, (size - h) // 2))
    return canvas


def wizard_big(mark: Image.Image, w: int, h: int) -> Image.Image:
    """The installer's tall side picture: the mark on deep titanium."""
    img = Image.new("RGB", (w, h), DEEP)
    px = img.load()
    for y in range(h):
        t = y / h
        c = tuple(round(DEEP[i] + (44 - DEEP[i]) * t * 0.55) for i in range(3))
        for x in range(w):
            px[x, y] = c
    mw = round(w * 0.74)
    m = mark.resize((mw, round(mw * mark.height / mark.width)), Image.LANCZOS)
    img.paste(m, ((w - mw) // 2, round(h * 0.34)), m)
    return img


def main() -> None:
    # Drawn at the largest size needed, reduced from there.
    mark = render(LOGO / "mark-dark.svg", 1256, 986)

    big = on_square(mark, 1024, 0.94)
    big.resize((256, 256), Image.LANCZOS).save(PACKAGING / "mirrorly-256.png")
    # Each size reduced directly from the large one: crisper than resizing 256.
    sizes = (16, 20, 24, 32, 40, 48, 64, 128, 256)
    # The smallest take the full width: every pixel counts there.
    full = on_square(mark, 1024, 1.0)
    frames = [(full if s < 48 else big).resize((s, s), Image.LANCZOS) for s in sizes]
    frames[-1].save(PACKAGING / "mirrorly.ico", format="ICO", append_images=frames[:-1], sizes=[(s, s) for s in sizes])

    # Inno Setup: 100 % and 200 % scale.
    for suffix, scale in (("", 1), ("@2x", 2)):
        wizard_big(mark, 164 * scale, 314 * scale).save(PACKAGING / f"wizard{suffix}.bmp")
        small = Image.new("RGB", (55 * scale, 58 * scale), (255, 255, 255))
        light = render(LOGO / "mark-light.svg", 1256, 986)
        m = on_square(light, 55 * scale, 0.88)
        small.paste(m, (0, (small.height - m.height) // 2), m)
        small.save(PACKAGING / f"wizard-small{suffix}.bmp")

    # Mirrorly's window draws these (at most about this big).
    APP_ASSETS.mkdir(exist_ok=True)
    for theme in ("light", "dark"):
        logo = render(LOGO / f"logo-{theme}.svg", 2003, 493)
        logo.resize((720, round(720 * 493 / 2003)), Image.LANCZOS).save(APP_ASSETS / f"logo-{theme}.png", optimize=True)
        m = render(LOGO / f"mark-{theme}.svg", 1256, 986)
        m.resize((320, round(320 * 986 / 1256)), Image.LANCZOS).save(APP_ASSETS / f"mark-{theme}.png", optimize=True)

    # iOS wants it opaque, and rounds the corners itself.
    touch = Image.new("RGB", (180, 180), DEEP)
    t = on_square(mark, 180, 0.76)
    touch.paste(t, (0, 0), t)
    touch.save(ROOT / "crates" / "display" / "src" / "apple-touch-icon.png", optimize=True)
    print("wrote the icon, wizard pictures, window logos and touch icon")


if __name__ == "__main__":
    main()
