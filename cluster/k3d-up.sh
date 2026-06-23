#!/usr/bin/env bash
# One-command full GitOps bring-up in a local k3d cluster:
#   k3d cluster + Argo CD + the pixelflux Application.
#
# The repo and the GHCR image are PUBLIC, so no secrets are needed. DOMAIN /
# ACME_EMAIL come from cluster/config.env (or the environment); locally they only
# affect the Host(...) routing — there's no public DNS, so no real cert is issued.
#
# Usage:  bash cluster/k3d-up.sh   (or: task k3d:up). Re-runnable (idempotent).
set -euo pipefail

CLUSTER="${CLUSTER:-pixelflux}"
HOST_PORT="${HOST_PORT:-8080}"
ARGOCD_REF="${ARGOCD_REF:-stable}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# --- config: env wins, else config.env, else local defaults ----------------
_D="${DOMAIN-}"; _E="${ACME_EMAIL-}"
# shellcheck source=/dev/null
[ -f "$REPO_ROOT/cluster/config.env" ] && . "$REPO_ROOT/cluster/config.env"
DOMAIN="${_D:-${DOMAIN:-pixelflux.example.com}}"
ACME_EMAIL="${_E:-${ACME_EMAIL:-admin@example.com}}"
echo "==> domain=$DOMAIN (local; access via port-forward)"

# --- 1. cluster ------------------------------------------------------------
if k3d cluster list "$CLUSTER" >/dev/null 2>&1; then
  echo "==> k3d cluster '$CLUSTER' already exists"
else
  echo "==> creating k3d cluster '$CLUSTER' (host :$HOST_PORT -> Traefik :80)"
  k3d cluster create "$CLUSTER" -p "${HOST_PORT}:80@loadbalancer" --wait
fi

# --- 2. ACME resolver (email from config; no real cert locally) ------------
echo "==> configuring the ACME resolver on the cluster Traefik"
sed "s/admin@example.com/$ACME_EMAIL/" "$REPO_ROOT/k8s/traefik-acme.yaml" \
  | kubectl apply -f -

# --- 3. Argo CD ------------------------------------------------------------
echo "==> installing Argo CD"
kubectl get namespace argocd >/dev/null 2>&1 || kubectl create namespace argocd
kubectl apply -n argocd \
  -f "https://raw.githubusercontent.com/argoproj/argo-cd/${ARGOCD_REF}/manifests/install.yaml"
echo "==> waiting for argocd-server"
kubectl -n argocd rollout status deploy/argocd-server --timeout=300s

# --- 4. Application (domain rendered into the host patches) -----------------
echo "==> applying the Argo CD Application (host=$DOMAIN)"
sed "s/pixelflux\.example\.com/$DOMAIN/g" "$REPO_ROOT/argocd/application.yaml" \
  | kubectl apply -f -
kubectl -n argocd annotate application pixelflux \
  argocd.argoproj.io/refresh=hard --overwrite >/dev/null

# --- 5. wait for the rollout + report --------------------------------------
echo "==> waiting for Argo to create and roll out the Deployment"
for _ in $(seq 1 60); do
  kubectl -n pixelflux get deploy pixelflux >/dev/null 2>&1 && break
  sleep 3
done
kubectl -n pixelflux rollout status deploy/pixelflux --timeout=300s

cat <<EOF

==> up (GitOps via Argo CD). For a quick look locally, port-forward past Traefik:
    kubectl -n pixelflux port-forward svc/pixelflux 8081:80
    # then open http://localhost:8081
EOF
