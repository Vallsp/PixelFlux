# 0006. Publish documentation with mdBook

- **Status:** Accepted
- **Date:** 2026-06-23

## Context

The project's documentation is good but **scattered**: the `README.md`, these
ADRs, `CONTRIBUTING.md`, `AGENTS.md`, `SECURITY.md`, `CHANGELOG.md`, and a
per-directory `README.md` under `api/`, `k8s/`, `load/`, and `argocd/`. There is
no single navigable, searchable entry point, and the README's many diagrams are
only rendered by GitHub. We want a documentation site that matches the rest of
the pipeline: reproducible (built from the Nix dev shell), automated (built and
published by CI), and with **no content duplication** — the Markdown files stay
the single source of truth.

## Decision

Build the docs with **mdBook**, a self-contained book project under `docs/book/`.
Chapter files are thin stubs that pull in the existing Markdown with mdBook's
`{{#include}}` directive (sections of the README are sliced with invisible
`<!-- ANCHOR -->` comments; self-contained files such as ADRs are included
whole), so editing a source document updates the book. The `mdbook-mermaid`
preprocessor renders the Mermaid diagrams. `mdbook` and `mdbook-mermaid` are
added to the Nix dev shell; `task docs:build` / `task docs:serve` drive it; CI
builds the book on every pull request and **deploys it to GitHub Pages** on
pushes to `main`.

## Consequences

### Positive

- One searchable, themed site covering every document, with rendered diagrams.
- No duplication: the Markdown files remain canonical and the book never drifts.
- Same ergonomics as the rest of the project — a Nix tool, a `task`, a CI job.

### Trade-offs / negative

- `{{#include}}` does not rewrite repository-relative links, so a few cross-file
  links inside included content can resolve differently in the book than on
  GitHub.
- GitHub Pages from a **private** repository requires a paid GitHub plan; until
  the repo is public or the plan is upgraded, only the build/validation step
  runs (the site is still fully usable locally via `task docs:serve`).

### Alternatives considered

- **A static site generator (Docusaurus / MkDocs)** — heavier, pulls in a
  Node/Python toolchain that the project otherwise avoids, and is overkill for a
  handful of Markdown files.
- **Leave the docs as scattered Markdown** — simplest, but no unified
  navigation, search, or rendered diagrams.
