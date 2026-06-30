# PixelPush

PixelPush paints an image onto a running PixelFlux canvas through the public HTTP
API. It reads the canvas size and palette from the server, resizes and quantizes
the input image, then posts only the pixels that differ from the live canvas.

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
rate window — and spreads paints across them. To reuse existing tokens instead:

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
- `--rate <n>` and `--rate-window <s>` control client-side pacing per token
  (defaults match PixelFlux: `4096` paints per `30` seconds).
- `--contain`, `--nearest`, `--dither`, and `--flatten` control image handling.
- `--bruteforce` repeats passes to keep the image on the canvas.
