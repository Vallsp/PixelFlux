# PixelPush

PixelPush paints an image onto a running PixelFlux canvas through the public HTTP
API. It reads the canvas size (and palette, if the server enforces one) from the
server, resizes the input image, then posts only the pixels that differ from the
live canvas. The canvas accepts any RGB colour by default, so no quantising
happens unless the server has palette enforcement turned on — in which case
PixelPush nearest-matches to it automatically.

PixelPush is a standalone Cargo package inside this repository. It is not a root
workspace member, so root PixelFlux builds and checks remain unchanged.

## Build

```bash
task pixelpush:build
```

## Run

Start PixelFlux first:

```bash
task run
```

Then push an image:

```bash
task pixelpush:run -- image.png --host http://localhost:3000
```

PixelFlux gates `POST /api/pixel` behind a token (`x-token` header), rate-limited
per token. By default PixelPush registers a small pool of tokens with
`POST /register` — auto-sized (in parallel) to repaint the whole image within one
rate window — and spreads paints across them. Registered tokens are cached on
disk per host (`~/.cache/pixelpush/tokens.json`, valid ~23h), so this ~5s
registration delay only happens once per host — every run after the first just
reuses them. Pass `--no-cache` to always register fresh tokens, or `--token` to
supply your own:

```bash
task pixelpush:run -- image.png \
  --host http://localhost:3000 \
  --token "$PIXELFLUX_TOKEN"
```

Run PixelPush-only checks with:

```bash
task pixelpush:check
```

Useful options:

- `--tokens <n>` registers exactly `n` paint tokens (in parallel) instead of the
  auto-sized default; `--tokens 0` registers none (use with `--token`).
- `--no-cache` skips the on-disk token cache and always registers fresh tokens.
- `--rate <n>` and `--rate-window <s>` control client-side pacing per token
  (defaults match PixelFlux: `4096` paints per `30` seconds).
- `--contain`, `--nearest`, and `--flatten` control image handling; `--dither`
  only matters if the server enforces a fixed palette.
- `--bruteforce` repeats passes to keep the image on the canvas.
