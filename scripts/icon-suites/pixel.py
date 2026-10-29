"""Pixel suite: each Lucide icon snapped to a 16x16 one-bit grid.

Usage: python3 -I pixel.py <lucide icons dir> <names.txt> <out dir>
"""
import io
import os
import re
import subprocess
import sys

from PIL import Image

GRID = 16
SS = 8  # supersampling factor
THRESHOLD = 0.34

src, names_file, out = sys.argv[1:4]
os.makedirs(os.path.join(out, "pixel"), exist_ok=True)
names = [l.strip() for l in open(names_file) if l.strip()]

for name in names:
    svg = open(os.path.join(src, f"{name}.svg")).read()
    # Draw in black on transparent, with a stroke that lands at ~1px on the grid.
    svg = svg.replace("currentColor", "#000")
    svg = re.sub(r'stroke-width="[^"]*"', 'stroke-width="1.6"', svg, count=1)
    size = GRID * SS
    png = subprocess.run(
        ["rsvg-convert", "-w", str(size), "-h", str(size), "-f", "png"],
        input=svg.encode(), capture_output=True, check=True,
    ).stdout
    alpha = Image.open(io.BytesIO(png)).convert("RGBA").getchannel("A")
    small = alpha.resize((GRID, GRID), Image.BOX)
    on = [[small.getpixel((x, y)) / 255 >= THRESHOLD for x in range(GRID)] for y in range(GRID)]
    # Merge horizontal runs into one path.
    d = []
    for y in range(GRID):
        x = 0
        while x < GRID:
            if on[y][x]:
                start = x
                while x < GRID and on[y][x]:
                    x += 1
                d.append(f"M{start} {y}h{x - start}v1h{start - x}z")
            else:
                x += 1
    with open(os.path.join(out, "pixel", f"{name}.svg"), "w") as f:
        f.write(
            '<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 16 16" '
            f'shape-rendering="crispEdges"><path fill="currentColor" d="{"".join(d)}"/></svg>\n'
        )
print(f"{len(names)} pixel icons")
