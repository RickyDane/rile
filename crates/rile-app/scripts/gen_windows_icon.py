#!/usr/bin/env python3
"""Generate Windows Folded Folio assets (requires librsvg and Pillow).

Run independently on any platform, or via gen_app_icon.sh. Each ICO frame is
rasterized from the SVG at its native size rather than downscaled from a PNG.
"""

from pathlib import Path
import subprocess
import tempfile

from PIL import Image


def main():
    assets = Path(__file__).resolve().parent.parent / "assets"
    source = assets / "app-icons" / "windows" / "folded-folio.svg"
    sizes = [16, 24, 32, 48, 64, 128, 256]
    with tempfile.TemporaryDirectory() as tmp:
        frames = {}
        for size in [*sizes, 512]:
            target = Path(tmp) / f"{size}.png"
            subprocess.run(
                ["rsvg-convert", "-w", str(size), "-h", str(size), str(source), "-o", str(target)],
                check=True,
            )
            with Image.open(target) as image:
                frames[size] = image.convert("RGBA")
        frames[512].save(source.with_suffix(".png"))
        frames[256].save(
            assets / "Rile.ico",
            sizes=[(size, size) for size in sizes],
            append_images=[frames[size] for size in sizes[:-1]],
        )
    print("Generated Windows Folded Folio PNG and seven-size ICO")


if __name__ == "__main__":
    main()
