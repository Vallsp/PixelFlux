# Argo CD — GitOps delivery

`application.yaml` is an Argo CD [Application](https://argo-cd.readthedocs.io/)
that continuously syncs the [`k8s/`](../k8s) kustomization to the cluster, so
every push to `main` is rolled out automatically.

## What it does

- Watches `path: k8s` on `targetRevision: main` of this repo.
- Deploys into the `pixelflux` namespace (created on first sync).
- `automated` sync with `prune: true` and `selfHeal: true` — the cluster is kept
  exactly in sync with git; manual changes are reverted, removed manifests are
  pruned.
- Sets the **per-cluster ingress host** via a kustomize patch in
  `spec.source.kustomize.patches` — the shared `k8s/` base stays
  domain-agnostic (`pixelflux.example.com`), the real host lives only here.

## No secrets needed

Both the repo and the GHCR image are **public**, so Argo CD clones over anonymous
HTTPS (`repoURL: https://github.com/...`) and the node pulls the public image —
**no deploy key and no pull secret**. The `images:` override in `application.yaml`
points the app at `ghcr.io/vallsp/pixelflux`, published by CI on every push to
`main`.

## Automatic image rollout (Argo CD Image Updater)

Argo CD syncs **manifests, not images** — so a new image under the _same_ tag
wouldn't otherwise trigger a sync. [Argo CD Image
Updater](https://argocd-image-updater.readthedocs.io/) (pinned `v0.18.0`, the
last classic annotation-driven release) closes that loop. The bring-up script
installs it into the `argocd` namespace; the tracking lives in annotations on
`application.yaml`:

- **What it watches:** `ghcr.io/vallsp/pixelflux:latest` (CI pushes `:latest` on
  every push to `main`).
- **Strategy `digest`:** when the `:latest` _digest_ changes, it pins the new
  digest into this Application's kustomize image override; Argo then rolls the
  Deployment. No version bump, no re-apply.
- **Write-back `argocd`:** it patches **this Application** via the Kubernetes API.
  Its install Role grants `applications: get/list/update/patch`, so it needs **no
  Argo CD API token**; the public GHCR package means **no registry secret**.

So the full loop is: **push to `main` → CI builds & pushes `:latest` → Image
Updater pins the new digest (polls ~every 2 min) → Argo rolls it out.**

```bash
# Watch it work:
kubectl -n argocd logs deploy/argocd-image-updater -f
kubectl -n argocd get application pixelflux \
  -o jsonpath='{.spec.source.kustomize.images}{"\n"}'   # flips to a @sha256: ref
```

> Heads-up: re-running `task k3s:up`/`k3d:up` re-applies `application.yaml`, which
> briefly resets the override to the bootstrap tag until the next poll re-pins the
> digest. To follow a versioned tag instead of `:latest`, switch the strategy to
> `semver` and bump `Cargo.toml`.

## Configuration (no values hard-coded)

The per-cluster settings — your **domain** and the **Let's Encrypt email** — live
in `cluster/config.env`, never in the manifests. The committed files keep the
`pixelflux.example.com` / `admin@example.com` placeholders; the bring-up script
renders your real values in at apply time.

```bash
cp cluster/config.env.example cluster/config.env   # then edit DOMAIN + ACME_EMAIL
```

## Usage

On a fresh k3s/k3d host, one command installs Argo CD and applies everything with
your config:

```bash
task k3s:up        # VPS / single-node k3s   (local Docker: task k3d:up)
```

If Argo CD is already installed and you'd rather apply by hand, render the domain
yourself (the script does this for you otherwise):

```bash
DOMAIN=your.domain.com
sed "s/pixelflux\.example\.com/$DOMAIN/g" argocd/application.yaml | kubectl apply -f -
```

From then on Argo CD reconciles automatically. Manage it with:

```bash
kubectl -n argocd get application pixelflux           # status
kubectl delete -f argocd/application.yaml             # tear down (finalizer cascades)
# pause auto-sync without deleting:
kubectl -n argocd patch application pixelflux --type merge -p '{"spec":{"syncPolicy":null}}'
```

## HTTPS

The synced kustomization carries the **HTTPS routing**: the HTTP→HTTPS redirect
and the TLS route, with the real host injected onto **both** routes by the
Application patches. The cluster-level ACME (Let's Encrypt) resolver lives in
`k8s/traefik-acme.yaml` and is applied once by the bring-up script with your
`ACME_EMAIL` — kept out of the GitOps sync so the email stays a config value, not
committed YAML. The certificate is issued automatically on the first request
(~1 min). DNS for the apex **and** `www.` must point at the cluster's IP.

## Caveats

- **Don't mix with the manual `task deploy*` path.** `selfHeal`/`prune` would
  revert or remove anything you apply by hand on top of what Argo manages.
- **Separate namespace.** Argo CD deploys into `pixelflux`; if you previously ran
  `task deploy` (default namespace), run `task deploy:down` first to avoid two
  copies.
- **The image must exist.** The GHCR image must be published (CI does this on each
  push to `main`) and the package set to public so the node can pull it and Image
  Updater can read its tags.
- **`:latest` is mutable.** Image Updater pins a `@sha256:` digest at runtime, so
  the _running_ image is deterministic — but the bootstrap `images:` tag in git is
  not. Don't read the committed tag as "what's deployed"; check the live
  Application (see the command above).
