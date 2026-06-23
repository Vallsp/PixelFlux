# AGENTS.md

Instructions for AI agents and new contributors working on Pixelflux.
`CLAUDE.md` points here, so the same guidance is picked up by Claude Code and
other agents.

## What this is

A collaborative 64×64 pixel canvas (Rust / axum), with real-time updates over
SSE + Redis pub/sub, wrapped in a full SDLC pipeline (Nix dev shell, distroless
container, multi-level tests, security scans, CI, and a Kubernetes deployment).

## Setup

```bash
nix develop            # reproducible dev shell — provides every tool
task lock              # generate Cargo.lock (first time only)
task hooks:install     # install the git hooks
```

## Commands — your toolbox

Run `task` with no arguments to list everything. The ones you'll use most:

| Goal                           | Command                          |
| ------------------------------ | -------------------------------- |
| Run the server (`:3000`)       | `task run`                       |
| Build                          | `task build`                     |
| Format everything              | `task fmt`                       |
| Lint (code + config)           | `task lint`                      |
| Check docs (prose + links)     | `task docs:lint`                 |
| Unit tests                     | `task test`                      |
| Integration tests (real Redis) | `task test:integration`          |
| API contract tests             | `task test:api`                  |
| Load test                      | `task bench`                     |
| Secret scan                    | `task secrets`                   |
| Build container                | `task container`                 |
| Image size guard (< 20 MB)     | `task container:size`            |
| SBOM / CVE scan                | `task sbom` / `task cve`         |
| Deploy (k3s + Traefik)         | `task deploy`, `task deploy:tls` |
| Full local gate                | `task check`                     |

## Conventions

- **Commits:** [Conventional Commits](https://www.conventionalcommits.org/),
  enforced by the `commit-msg` hook (`feat`, `fix`, `docs`, `ci`, …).
- **Hooks:** `pre-commit` formats and lints staged files and scans for secrets;
  don't bypass with `--no-verify` on shared branches.
- **Branches:** short-lived, named after the change (`feat/…`, `fix/…`); rebase
  on `main` before a PR.
- **CI must be green** before merging.

## Where things are

| Path           | What                                                                         |
| -------------- | ---------------------------------------------------------------------------- |
| `src/`         | Rust app (`lib.rs` = canvas + routes, `main.rs`, `index.html` = embedded UI) |
| `api/`         | OpenAPI spec + Hurl contract tests                                           |
| `load/`        | k6 load test                                                                 |
| `k8s/`         | Kubernetes manifests + Traefik                                               |
| `argocd/`      | Argo CD GitOps Application                                                   |
| `flake.nix`    | Dev shell + container build                                                  |
| `Taskfile.yml` | Every task                                                                   |

## Before you finish a change

1. `task fmt` then `task check` (lint + secrets + tests).
2. `task docs:lint` if you touched documentation.
3. Commit with a Conventional Commit message; push; make sure CI is green.

## Going further with real agent skills

This file documents the `task` commands as the agent toolbox. If you later want
versioned, executable Claude/agent **skills** (e.g. a `deploy` or `review`
skill), add them under `.claude/skills/` and reference them here.
