#!/usr/bin/env python3
"""Paint any image onto the PixelFlux canvas (64x64, 16-color palette)."""

import sys
import requests
from PIL import Image
from concurrent.futures import ThreadPoolExecutor, as_completed

HOST = "http://localhost:3000"

# PixelFlux palette — index matches the color value sent to the API
PALETTE = [
    (255, 255, 255),  # 0  white
    (228, 228, 228),  # 1  light gray
    (136, 136, 136),  # 2  gray
    (34,  34,  34),   # 3  black
    (255, 167, 209),  # 4  pink
    (229,   0,   0),  # 5  red
    (229, 149,   0),  # 6  orange
    (160, 106,  66),  # 7  brown
    (229, 217,   0),  # 8  yellow
    (148, 224,  68),  # 9  light green
    (  2, 190,   1),  # 10 green
    (  0, 211, 221),  # 11 cyan
    (  0, 131, 199),  # 12 blue
    (  0,   0, 234),  # 13 dark blue
    (207, 110, 228),  # 14 purple
    (130,   0, 128),  # 15 dark purple
]


def closest_color(r: int, g: int, b: int) -> int:
    """Return the palette index whose RGB is closest to (r, g, b)."""
    return min(
        range(len(PALETTE)),
        key=lambda i: (r - PALETTE[i][0]) ** 2
                    + (g - PALETTE[i][1]) ** 2
                    + (b - PALETTE[i][2]) ** 2,
    )


def paint_pixel(x: int, y: int, color: int) -> tuple[int, int, bool]:
    try:
        r = requests.post(
            f"{HOST}/api/pixel",
            json={"x": x, "y": y, "color": color},
            timeout=5,
        )
        return x, y, r.ok
    except requests.RequestException:
        return x, y, False


def main(image_path: str) -> None:
    img = Image.open(image_path).convert("RGB").resize((64, 64), Image.LANCZOS)

    # Build the list of (x, y, color) for all 4096 pixels
    pixels = []
    for y in range(64):
        for x in range(64):
            r, g, b = img.getpixel((x, y))
            pixels.append((x, y, closest_color(r, g, b)))

    print(f"Painting {len(pixels)} pixels onto {HOST} ...")

    done = 0
    failed = 0

    # Send requests in parallel (32 workers = fast without overwhelming the server)
    with ThreadPoolExecutor(max_workers=32) as pool:
        futures = {pool.submit(paint_pixel, x, y, c): (x, y) for x, y, c in pixels}
        for future in as_completed(futures):
            _, _, ok = future.result()
            if ok:
                done += 1
            else:
                failed += 1
            if (done + failed) % 256 == 0:
                print(f"  {done + failed}/4096 …")

    print(f"Done — {done} ok, {failed} failed.")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print("Usage: python paint_image.py <image_path>")
        sys.exit(1)
    main(sys.argv[1])
