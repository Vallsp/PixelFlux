# Contributing to Pixelflux

This guide covers everything you need to develop, test, and ship Pixelflux:
the dev environment, the git workflow, and how to run each kind of test.

## Table of contents

- [Prerequisites](#prerequisites)
- [Getting started](#getting-started)
- [Git workflow](#git-workflow)
  - [Branches](#branches)
  - [Commit messages](#commit-messages)
  - [Git hooks](#git-hooks)
  - [Pull requests](#pull-requests)
- [Everyday commands](#everyday-commands)
- [Running the tests](#running-the-tests)
- [Quality, formatting & security](#quality-formatting--security)
- [Building the container](#building-the-container)
- [Deploying](#deploying)
- [Continuous integration](#continuous-integration)

## Prerequisites

- [Nix](https://nixos.org/download) with flakes enabled
- A container runtime (Docker or Podman) — needed for integration tests and the
  container image

Nothing else has to be installed: the Nix dev shell provides Rust, the task
runner, all linters, the security scanners, and the test tooling at pinned
versions.

## Getting started

```bash
git clone git@github.com:Vallsp/PixelFlux.git
cd PixelFlux

nix develop            # enter the reproducible dev shell (or: direnv allow)
task lock              # generate Cargo.lock (first time only)
task hooks:install     # install the git hooks
task run               # run the server -> http://localhost:3000
```

Run `task` with no arguments at any time to list every available task.

## Git workflow

### Branches

- `main` is always releasable; CI runs on every push and pull request.
- Do your work on a short-lived branch named after the change, e.g.
  `feat/eraser-tool`, `fix/redis-reconnect`, `docs/contributing`.
- Keep branches small and focused; rebase on `main` before opening a PR.

```bash
git switch -c feat/eraser-tool
# ...work...
git push -u origin feat/eraser-tool
```

### Commit messages

We follow [Conventional Commits](https://www.conventionalcommits.org/). The
`commit-msg` hook **rejects** any message that doesn't match. Format:

```text
<type>(<optional scope>): <description>
```

Allowed types:

| Type       | When to use it                                          |
| ---------- | ------------------------------------------------------- |
| `feat`     | A new feature                                           |
| `fix`      | A bug fix                                               |
| `docs`     | Documentation only                                      |
| `style`    | Formatting, no code change                              |
| `refactor` | Code change that neither fixes a bug nor adds a feature |
| `perf`     | A performance improvement                               |
| `test`     | Adding or fixing tests                                  |
| `build`    | Build system or dependencies                            |
| `ci`       | CI configuration                                        |
| `chore`    | Maintenance, tooling                                    |
| `revert`   | Reverting a previous commit                             |

Examples:

```text
feat: add an eraser tool to the canvas
fix(redis): reconnect the pub/sub subscriber on drop
ci: implement sbom task with Syft
```

### Git hooks

Installed with `task hooks:install` (managed by [lefthook](https://lefthook.dev/)):

| Hook         | What runs                                                                                     |
| ------------ | --------------------------------------------------------------------------------------------- |
| `pre-commit` | format staged files (treefmt), `clippy -D warnings`, secret scan on staged changes (gitleaks) |
| `commit-msg` | enforce Conventional Commits                                                                  |
| `pre-push`   | unit tests + a release build                                                                  |

To bypass them in an emergency: `git commit --no-verify` (avoid on shared
branches — CI will still enforce the same checks).

### Pull requests

1. Make sure the full gate passes locally: `task check` (lint + secrets + tests).
2. Push your branch and open a PR against `main`.
3. CI must be green before merging (it runs the same checks plus the container
   build and CVE scan).

## Everyday commands

```bash
task run                # run the server on :3000
task build              # debug build
task fmt                # auto-format every file type (treefmt)
task check              # full local gate: lint + secrets + tests
```

For the shared, persisted canvas (mirrors production), run Redis alongside:

```bash
docker run -d -p 6379:6379 redis:7-alpine
REDIS_URL=redis://localhost:6379 task run
```

## Running the tests

There are four levels of tests. Unit tests run anywhere; the others need a
container runtime and/or a running server.

| Level            | Command                 | What it does                                                                         | Needs         |
| ---------------- | ----------------------- | ------------------------------------------------------------------------------------ | ------------- |
| Unit             | `task test`             | Canvas logic and routes exercised in-memory                                          | nothing       |
| Integration      | `task test:integration` | A pixel painted via one instance is read back from a **real Redis** (Testcontainers) | Docker/Podman |
| API contract     | `task test:api`         | Boots the server, validates it against `api/openapi.yaml` with Hurl                  | —             |
| Load / benchmark | `task bench`            | k6 reads the canvas and paints random pixels under load (p95 < 200 ms, < 1% errors)  | —             |

`task test:api` and `task bench` start the server themselves and stop it when
they finish, so you don't need a server running beforehand.

## Quality, formatting & security

```bash
task fmt                # format Rust, TOML, Nix, shell, Markdown, YAML, JSON (treefmt)
task lint               # treefmt --fail-on-change + clippy + yamllint + actionlint + markdownlint
task secrets            # scan the whole repo for leaked credentials (gitleaks)
```

`task lint` is what CI runs; `task fmt` fixes most of what it would flag.

## Building the container

The image is built entirely by Nix — a distroless static binary, non-root,
under 20 MB.

```bash
task container          # build the image with Nix
task container:load     # build and load it into Docker
task container:size     # print the size and fail if it exceeds 20 MB
task container:inspect  # explore the layers with dive
task sbom               # generate an SBOM (Syft) -> sbom.json
task cve                # scan the image for CVEs (Trivy), fails on HIGH/CRITICAL
```

## Deploying

Kubernetes + Traefik on a single-node k3s host. See the
[Deploy section of the README](README.md#deploy-kubernetes--traefik) for the
full flow; in short:

```bash
task deploy:k3s-install                  # once: install k3s + Traefik
task deploy                              # build, import, apply the app
DOMAIN=your.domain.com task deploy:ingress              # expose over HTTP
# or HTTPS (Let's Encrypt):
DOMAIN=your.domain.com ACME_EMAIL=you@domain.com task deploy:tls
```

> `deploy:ingress` (HTTP) and `deploy:tls` (HTTPS) define the same route, so the
> last one applied wins — don't mix them. After enabling HTTPS, use
> `task deploy:restart` (not `task deploy`) for code changes so the TLS route is
> preserved.

## Continuous integration

`.github/workflows/ci.yml` runs three jobs, all inside `nix develop` so CI and
local development share the same toolchain:

| Job         | Checks                                                            |
| ----------- | ----------------------------------------------------------------- |
| `quality`   | lint, format check, secret scan                                   |
| `test`      | build, unit tests, integration tests (Testcontainers)             |
| `container` | build the distroless image, enforce < 20 MB, SBOM, Trivy CVE scan |

The build fails if any check fails, so the server-side pipeline catches issues
even when local git hooks are skipped.
