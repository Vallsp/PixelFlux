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

## Prerequisites

- A cluster running Argo CD (in the `argocd` namespace). Quick install:

  ```bash
  kubectl create namespace argocd
  kubectl apply -n argocd -f https://raw.githubusercontent.com/argoproj/argo-cd/stable/manifests/install.yaml
  ```

- The container image available on the node(s). Argo CD syncs **manifests, not
  images**: `k8s/pixelflux.yaml` uses `image: pixelflux:latest`
  (`imagePullPolicy: IfNotPresent`). On single-node k3s, import it first with
  `task deploy:image`; for multi-node, push to a registry (e.g. GHCR) and point
  `image:` there with `imagePullPolicy: Always`.

## Usage

Set your hostname in `application.yaml` (the `Host(...)` value in the patch),
commit it, then apply the Application once:

```bash
kubectl apply -f argocd/application.yaml
```

From then on Argo CD reconciles automatically. Manage it with:

```bash
kubectl -n argocd get application pixelflux           # status
kubectl delete -f argocd/application.yaml             # tear down (finalizer cascades)
# pause auto-sync without deleting:
kubectl -n argocd patch application pixelflux --type merge -p '{"spec":{"syncPolicy":null}}'
```

## Caveats

- **HTTP only.** The synced kustomization includes the HTTP `ingressroute`. The
  HTTPS route, redirect middleware, and ACME config (`k8s/ingressroute-tls.yaml`,
  `k8s/traefik-acme.yaml`) are **not** managed here.
- **Don't mix with `task deploy:tls`.** `selfHeal`/`prune` would revert or remove
  a manually applied TLS route.
- **Separate namespace.** Argo CD deploys into `pixelflux`; if you previously ran
  `task deploy` (default namespace), run `task deploy:down` first to avoid two
  copies.

To run full HTTPS via GitOps, move the TLS route into the synced manifests and
patch its host too — see the note in `k8s/README.md`.
