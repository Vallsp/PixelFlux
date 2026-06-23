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
points the app at `ghcr.io/vallsp/pixelflux:<version>`, published by CI on every
push to `main`.

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
- **The image must exist.** Argo syncs manifests, not images — the GHCR image in
  the Application's `images:` override must be published (CI does this on each
  push to `main`) and the package set to public so the node can pull it.
