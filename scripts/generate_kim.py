#!/usr/bin/env python3
"""Generate a 64x64 pixel art portrait of Kim Jong-un and save it as kim.png."""

from PIL import Image, ImageDraw

img = Image.new("RGB", (64, 64))
d = ImageDraw.Draw(img)

# PixelFlux palette colours used
BG         = (136, 136, 136)  # 2  gray
SKIN       = (229, 149,   0)  # 6  orange (closest to skin)
SHADOW     = (160, 106,  66)  # 7  brown  (shadow / darker skin)
BLACK      = ( 34,  34,  34)  # 3  black
DARK_BLUE  = (  0,   0, 234)  # 13 dark blue (suit)
WHITE      = (255, 255, 255)  # 0  white (shirt/collar)
LIGHT_GRAY = (228, 228, 228)  # 1  light gray (eye whites)

# --- Background ---
d.rectangle([0, 0, 63, 63], fill=BG)

# --- Face (round, chubby) ---
d.ellipse([10, 14, 54, 56], fill=SKIN)

# --- Chubby cheeks (shadow) ---
d.ellipse([ 9, 30, 18, 46], fill=SHADOW)
d.ellipse([46, 30, 55, 46], fill=SHADOW)

# --- Flat-top haircut ---
d.rectangle([12,  6, 52, 20], fill=BLACK)   # flat top block
d.ellipse  ([10,  8, 24, 22], fill=BLACK)   # left curve
d.ellipse  ([40,  8, 54, 22], fill=BLACK)   # right curve

# --- Shaved sides ---
d.rectangle([10, 18, 16, 38], fill=BLACK)   # left side
d.rectangle([48, 18, 54, 38], fill=BLACK)   # right side

# --- Ears ---
d.ellipse([ 7, 30, 13, 40], fill=SKIN)
d.ellipse([51, 30, 57, 40], fill=SKIN)

# --- Eyebrows (thick) ---
d.rectangle([17, 24, 29, 27], fill=BLACK)
d.rectangle([35, 24, 47, 27], fill=BLACK)

# --- Eyes (small, narrow) ---
d.ellipse([17, 27, 30, 34], fill=LIGHT_GRAY)
d.ellipse([34, 27, 47, 34], fill=LIGHT_GRAY)
d.ellipse([20, 28, 27, 33], fill=BLACK)     # left pupil
d.ellipse([37, 28, 44, 33], fill=BLACK)     # right pupil
# heavy upper eyelids
d.rectangle([17, 27, 30, 29], fill=BLACK)
d.rectangle([34, 27, 47, 29], fill=BLACK)

# --- Nose (wide, flat) ---
d.ellipse([26, 36, 38, 45], fill=SHADOW)
d.ellipse([24, 41, 30, 46], fill=BLACK)     # left nostril
d.ellipse([34, 41, 40, 46], fill=BLACK)     # right nostril

# --- Mouth (small, thin) ---
d.rectangle([25, 48, 39, 50], fill=BLACK)
d.arc([23, 46, 31, 52], start=180, end=270, fill=SHADOW)  # left corner
d.arc([33, 46, 41, 52], start=270, end=360, fill=SHADOW)  # right corner

# --- Suit (Mao-style dark jacket) ---
d.rectangle([ 0, 54, 63, 63], fill=DARK_BLUE)
d.rectangle([ 8, 52, 56, 63], fill=DARK_BLUE)

# --- White collar ---
d.polygon([(26, 52), (32, 52), (30, 63), (26, 63)], fill=WHITE)
d.polygon([(32, 52), (38, 52), (38, 63), (34, 63)], fill=WHITE)

img.save("kim.png")
print("Saved → kim.png  (64×64)")
