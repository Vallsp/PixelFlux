<h1 align="center">Pixelflux</h1>

<p align="center">
  A real-time, multiplayer pixel canvas written in Rust, wrapped in a
  complete, production-grade software delivery pipeline.
</p>

<p align="center">
  <img alt="CI" src="https://github.com/Vallsp/pixelflux/actions/workflows/ci.yml/badge.svg" />
  <img alt="Rust" src="https://img.shields.io/badge/Rust-stable-000000?logo=rust&logoColor=white" />
  <img alt="Nix" src="https://img.shields.io/badge/Nix-flakes-5277C3?logo=nixos&logoColor=white" />
  <img alt="Container" src="https://img.shields.io/badge/image-distroless%20%7C%20%3C20MB-2496ED?logo=docker&logoColor=white" />
  <img alt="CVE" src="https://img.shields.io/badge/CVE-0%20target-3fb950" />
  <img alt="License" src="https://img.shields.io/badge/license-MIT-blue" />
</p>

## Overview

`pixelflux` is a small but complete web application: a shared **64×64 pixel canvas**
with a 16-colour palette. Visitors pick a colour and paint cells; everyone
draws on the same canvas and sees other people's pixels appear live.

The application is intentionally compact — the point of the project is the
**engineering pipeline** built around it: a one-command reproducible
development environment, a distroless container, automated quality and security
gates, multi-level testing, and continuous integration.

## Table of contents

- [Features](#features)
- [Tech stack](#tech-stack)
- [Getting started](#getting-started)
- [Configuration](#configuration)
- [API reference](#api-reference)
- [Architecture](#architecture)
- [Development](#development)
- [Testing](#testing)
- [Container image](#container-image)
- [Continuous integration](#continuous-integration)
- [Contributing](#contributing)
- [License](#license)

## Features

- **Shared state** — the grid is a single `width*height` string persisted in
  Redis, shared across every server instance. Without a `REDIS_URL` it falls
  back to an in-process canvas, so the app runs with zero external dependencies.
- **Real-time updates** — painted pixels are pushed to browsers over
  Server-Sent Events (`/api/events`) through an in-process broadcast channel.
  No polling and no WebSocket dependency; the browser also performs a full
  resync every 10 seconds as a safety net.
- **Single static binary** — the entire web UI (HTML, CSS, JS) is embedded in
  the binary at compile time, so the runtime artifact is just one executable.
- **Reproducible and minimal** — the dev shell and the container image are both
  built by Nix; the image is distroless, runs as a non-root user, and stays
  under 20 MB.

## Tech stack

| Concern              | Tool                            |
| -------------------- | ------------------------------- |
| Language             | Rust + [axum]                   |
| Reproducible shell   | [Nix] flake + [direnv]          |
| Task runner          | [go-task] (Taskfile)            |
| Container build      | Nix `dockerTools` (distroless)  |
| Git hooks            | [lefthook]                      |
| Formatting           | [treefmt]                       |
| Secret scanning      | [gitleaks]                      |
| SBOM                 | [Syft]                          |
| CVE scanning         | [Trivy]                         |
| Load / benchmark     | [k6]                            |
| Integration tests    | [Testcontainers]                |
| API contract         | [Hurl] + [OpenAPI]              |
| CI                   | [GitHub Actions]                |
| Persistence          | Redis                           |

[axum]: https://github.com/tokio-rs/axum
[Nix]: https://nixos.org/
[direnv]: https://direnv.net/
[go-task]: https://taskfile.dev/
[lefthook]: https://lefthook.dev/
[treefmt]: https://github.com/numtide/treefmt
[gitleaks]: https://gitleaks.io/
[Syft]: https://github.com/anchore/syft
[Trivy]: https://trivy.dev/
[k6]: https://k6.io/
[Testcontainers]: https://testcontainers.com/
[Hurl]: https://hurl.dev/
[OpenAPI]: https://swagger.io/specification/
[GitHub Actions]: https://github.com/features/actions

## Getting started

### Prerequisites

- [Nix](https://nixos.org/download) with flakes enabled
- A container runtime (Docker or Podman)

No other tooling is required — the Nix dev shell provides Rust, the task
runner, all linters, the security scanners, and the test tooling at pinned
versions.

### Quick start

```bash
# 1. Enter the reproducible dev shell (provides every tool)
nix develop            # or: direnv allow   (auto-loads on cd)

# 2. One-time setup
task lock              # generate Cargo.lock
task hooks:install     # install the git hooks

# 3. Run the server
task run               # then open http://localhost:3000
```

Open the page in two browser tabs and paint — pixels appear in both instantly.

## Configuration

The server is configured through environment variables:

| Variable    | Default | Description                                                        |
| ----------- | ------- | ----------------------------------------------------------------- |
| `PORT`      | `3000`  | TCP port the HTTP server listens on.                              |
| `REDIS_URL` | _unset_ | Redis connection string (e.g. `redis://localhost:6379`). When set, the canvas is shared and persisted; otherwise an in-process canvas is used. |

## API reference

| Method | Route          | Description                                                  |
| ------ | -------------- | ----------------------------------------------------------- |
| GET    | `/`            | Web UI (embedded single page).                              |
| GET    | `/health`      | Liveness probe → `{"status":"ok"}`.                         |
| GET    | `/info`        | Binary name and version.                                    |
| GET    | `/api/canvas`  | Whole canvas → `{width, height, palette, pixels}`.          |
| POST   | `/api/pixel`   | Paint one pixel: `{x, y, color}` → `{ok}` (400 if invalid). |
| GET    | `/api/events`  | Live pixel stream (Server-Sent Events).                     |

The full specification is in [`api/openapi.yaml`](api/openapi.yaml).

## Architecture

```
┌─────────────┐   SSE  /api/events    ┌────────────────────────┐
│  Browser A  │◀──────────────────────│                        │
│  (canvas)   │──── POST /api/pixel ──▶│ pixelflux (Rust / axum)│      ┌─────────┐
└─────────────┘                        │                        │─────▶│  Redis  │
┌─────────────┐                        │  broadcast channel ──▶ │  GET  │ canvas  │
│  Browser B  │◀──────────────────────│  SSE fan-out           │◀──────│ (string)│
│  (canvas)   │──── POST /api/pixel ──▶│  in-mem fallback       │ SET   └─────────┘
└─────────────┘                        └────────────────────────┘
```

A painted pixel is written to Redis (`SETRANGE`) and fanned out to every
connected browser through an in-process broadcast channel exposed as SSE.

## Development

### Project structure

```
.
├── flake.nix              # dev shell + static musl build + distroless image
├── rust-toolchain.toml    # pinned toolchain (+ musl target)
├── .envrc                 # direnv -> use flake
├── Taskfile.yml           # task definitions (go-task)
├── treefmt.toml           # one formatter for all file types
├── lefthook.yml           # pre-commit / commit-msg / pre-push hooks
├── .gitleaks.toml         # secret scanning config
├── Cargo.toml
├── src/
│   ├── lib.rs             # canvas logic + router + unit tests
│   ├── main.rs            # binary entrypoint
│   └── index.html         # embedded web UI
├── tests/integration.rs   # Testcontainers (real Redis)
├── load/health.js         # k6 load test
├── api/openapi.yaml       # OpenAPI specification
├── api/contract.hurl      # Hurl contract tests
└── .github/workflows/ci.yml
```

### Available tasks

Run `task` with no arguments to list every task.

| Task                   | Description                                          |
| ---------------------- | ---------------------------------------------------- |
| `task build`           | Debug build.                                         |
| `task run`             | Run the server on port 3000.                         |
| `task fmt`             | Auto-format every file type (treefmt).               |
| `task lint`            | clippy + treefmt + YAML/Markdown/Actions linters.    |
| `task test`            | Unit tests.                                          |
| `task test:integration`| Integration tests with a real Redis (Testcontainers).|
| `task test:api`        | API contract tests (Hurl) against a running server.  |
| `task bench`           | Load test with k6.                                   |
| `task secrets`         | Scan the codebase for leaked credentials (gitleaks). |
| `task container`       | Build the distroless image with Nix.                 |
| `task container:load`  | Load the image into the local Docker daemon.         |
| `task container:size`  | Print the image size and fail if it exceeds 20 MB.   |
| `task container:inspect`| Inspect image layers with dive.                     |
| `task sbom`            | Generate an SBOM (Syft).                             |
| `task cve`             | Scan the image for CVEs (Trivy).                     |
| `task ci`              | Run the full pipeline locally.                       |

### Code quality

Git hooks are managed by lefthook and run automatically:

- **pre-commit** — formatting (treefmt), linting (clippy), and secret detection
  (gitleaks) on staged files.
- **commit-msg** — enforces [Conventional Commits](https://www.conventionalcommits.org/).
- **pre-push** — unit tests and a release build.

Install them once with `task hooks:install`.

## Testing

Four levels of testing, from fast to thorough:

1. **Unit** (`src/lib.rs`) — canvas logic and routes exercised in-memory.
2. **Integration** (`tests/integration.rs`) — a pixel painted via one instance
   is read back from a real Redis through a separate instance, using
   Testcontainers (requires Docker or Podman).
3. **API contract** (`api/contract.hurl`) — validated against the OpenAPI spec.
4. **Load / benchmark** (`load/health.js`) — k6 reads the canvas and paints
   random pixels under load (thresholds: p95 < 200 ms, < 1% errors).

## Container image

The image is built entirely by Nix (`nix build .#container`):

- **Distroless** — the contents are the static musl binary closure only: no
  shell, no package manager, no libc layered on top.
- **Non-root** — runs as `uid 65532`.
- **Small** — the release profile is size-optimised (`opt-level = "z"`, LTO,
  stripped, `panic = "abort"`); the resulting image stays well under 20 MB,
  enforced by `task container:size`.
- **Zero-CVE target** — there is essentially no attack surface beyond the
  binary, verified by `task cve` (Trivy).

```bash
task container:load     # build and load into Docker
task container:size     # verify size budget
task cve                # scan for vulnerabilities
docker run --rm -p 3000:3000 pixelflux:0.1.0
```

## Continuous integration

[`.github/workflows/ci.yml`](.github/workflows/ci.yml) runs three jobs, all
inside `nix develop` so CI and local development use the same toolchain:

| Job         | Responsibilities                                              |
| ----------- | ------------------------------------------------------------ |
| `quality`   | Lint, format check, and secret scan.                         |
| `test`      | Build, unit tests, and Testcontainers integration tests.     |
| `container` | Build the distroless image, enforce the size budget, generate the SBOM, and run the Trivy CVE scan. |

The build fails if any check fails, so the server-side pipeline catches issues
even when local git hooks are skipped.

## Contributing

1. Enter the dev shell: `nix develop`.
2. Install the hooks: `task hooks:install`.
3. Make your change and keep it green: `task ci`.
4. Commit using Conventional Commits (e.g. `feat: add eraser tool`).
5. Open 