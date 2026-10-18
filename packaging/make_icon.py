"""Draw the Mirrorly icon: packaging/mirrorly.ico (16-256 px) and mirrorly-256.png.

Run from the repository root:  python packaging/make_icon.py
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

S = 1024  # draw large, downsample for clean edges
OUT = Path(__file__).resolve().parent


def lerp(a, b, t):
    return tuple(round(a[i] + (b[i] - a[i]) * t) for i in range(len(a)))


def icon() -> Image.Image:
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))

    # Rounded-square tile with a diagonal blue -> violet gradient.
    grad = Image.new("RGBA", (S, S))
    top, bottom = (38, 132, 255, 255), (124, 58, 237, 255)
    px = grad.load()
    for y in range(S):
        for x in range(S):
            px[x, y] = lerp(top, bottom, (x * 0.35 + y * 0.65) / S)
    mask = Image.new("L", (S, S), 0)
    ImageDraw.Draw(mask).rounded_rectangle((40, 40, S - 40, S - 40), radius=230, fill=255)
    img.paste(grad, (0, 0), mask)

    d = ImageDraw.Draw(img)
    # Phone outline.
    pw, ph = 380, 680
    x0, y0 = (S - pw) // 2, (S - ph) // 2
    d.rounded_rectangle((x0, y0, x0 + pw, y0 + ph), radius=84, outline=(255, 255, 255, 255), width=46)
    # Island at the top of the phone.
    d.rounded_rectangle((S // 2 - 62, y0 + 58, S // 2 + 62, y0 + 92), radius=17, fill=(255, 255, 255, 255))

    # "Mirror" reflection: a soft diagonal light stripe across the screen.
    shine = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    ds = ImageDraw.Draw(shine)
    ds.polygon([(x0 + 70, y0 + ph - 150), (x0 + pw - 70, y0 + 230), (x0 + pw - 70, y0 + 330), (x0 + 70, y0 + ph - 50)],
               fill=(255, 255, 255, 120))
    screen = Image.new("L", (S, S), 0)
    ImageDraw.Draw(screen).rounded_rectangle((x0 + 23, y0 + 23, x0 + pw - 23, y0 + ph - 23), radius=62, fill=255)
    shine.putalpha(Image.composite(shine.getchannel("A"), Image.new("L", (S, S), 0), screen))
    img = Image.alpha_composite(img, shine.filter(ImageFilter.GaussianBlur(2)))
    return img


def main() -> None:
    big = icon()
    big.resize((256, 256), Image.LANCZOS).save(OUT / "mirrorly-256.png")
    sizes = [(s, s) for s in (16, 20, 24, 32, 40, 48, 64, 128, 256)]
    big.resize((256, 256), Image.LANCZOS).save(OUT / "mirrorly.ico", sizes=sizes)
    print("wrote", OUT / "mirrorly.ico", "and", OUT / "mirrorly-256.png")


if __name__ == "__main__":
    main()
