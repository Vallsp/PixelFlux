#!/usr/bin/env bash
# One-command full GitOps bring-up in a local k3d cluster:
#   k3d cluster + Argo CD + secrets (from .env) + the pixelflux Application.
#
# Usage:  bash cluster/k3d-up.sh   (or: task k3d:up)
# Requires: a .env file (copy .env.example). Re-runnable (idempotent).
set -euo pipefail

CLUSTER="${CLUSTER:-pixelflux}"
HOST_PORT="${HOST_PORT:-8080}"
ARGOCD_REF="${ARGOCD_REF:-stable}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# --- load .env -------------------------------------------------------------
if [ ! -f "$REPO_ROOT/.env" ]; then
  echo "ERROR: $REPO_ROOT/.env not found — copy .env.example to .env and fill it in." >&2
  exit 1
fi
set -a
# shellcheck source=/dev/null
. "$REPO_ROOT/.env"
set +a
: "${GHCR_USER:?set GHCR_USER in .env}"
: "${GHCR_PAT:?set GHCR_PAT in .env}"
: "${DEPLOY_KEY_PATH:?set DEPLOY_KEY_PATH in .env}"
DEPLOY_KEY_PATH="${DEPLOY_KEY_PATH/#\~/$HOME}"
if [ ! -f "$DEPLOY_KEY_PATH" ]; then
  echo "ERROR: deploy key not found at $DEPLOY_KEY_PATH" >&2
  exit 1
fi

# --- 1. cluster ------------------------------------------------------------
if k3d cluster list "$CLUSTER" >/dev/null 2>&1; then
  echo "==> k3d cluster '$CLUSTER' already exists"
else
  echo "==> creating k3d cluster '$CLUSTER' (host :$HOST_PORT -> Traefik :80)"
  k3d cluster create "$CLUSTER" -p "${HOST_PORT}:80@loadbalancer" --wait
fi

# --- 2. Argo CD ------------------------------------------------------------
echo "==> installing Argo CD"
kubectl get namespace argocd >/dev/null 2>&1 || kubectl create namespace argocd
kubectl apply -n argocd \
  -f "https://raw.githubusercontent.com/argoproj/argo-cd/${ARGOCD_REF}/manifests/install.yaml"
echo "==> waiting for argocd-server"
kubectl -n argocd rollout status deploy/argocd-server --timeout=300s

# --- 3. namespace + secrets (from .env) ------------------------------------
echo "==> creating namespace + secrets"
kubectl get namespace pixelflux >/dev/null 2>&1 || kubectl create namespace pixelflux
for _ in $(seq 1 30); do
  kubectl -n pixelflux get serviceaccount default >/dev/null 2>&1 && break
  sleep 1
done

# Argo CD repository credential (SSH deploy key).
kubectl create secret generic pixelflux-repo -n argocd \
  --from-literal=type=git \
  --from-literal=url=git@github.com:Vallsp/PixelFlux.git \
  --from-file=sshPrivateKey="$DEPLOY_KEY_PATH" \
  --dry-run=client -o yaml | kubectl apply -f -
kubectl -n argocd label secret pixelflux-repo \
  argocd.argoproj.io/secret-type=repository --overwrite

# GHCR pull credential, attached to the namespace's default ServiceAccount.
kubectl create secret docker-registry ghcr-pull -n pixelflux \
  --docker-server=ghcr.io \
  --docker-username="$GHCR_USER" \
  --docker-password="$GHCR_PAT" \
  --dry-run=client -o yaml | kubectl apply -f -
kubectl -n pixelflux patch serviceaccount default \
  -p '{"imagePullSecrets":[{"name":"ghcr-pull"}]}'

# --- 4. Application --------------------------------------------------------
echo "==> applying the Argo CD Application"
kubectl apply -f "$REPO_ROOT/argocd/application.yaml"
kubectl -n argocd annotate application pixelflux \
  argocd.argoproj.io/refresh=hard --overwrite >/dev/null

# --- 5. wait for the rollout + report --------------------------------------
echo "==> waiting for Argo to create and roll out the Deployment"
for _ in $(seq 1 60); do
  kubectl -n pixelflux get deploy pixelflux >/dev/null 2>&1 && break
  sleep 3
done
kubectl -n pixelflux rollout status deploy/pixelflux --timeout=300s

host="$(kubectl -n pixelflux get ingressroute pixelflux \
  -o jsonpath='{.spec.routes[0].match}' 2>/dev/null |
  sed -E 's/.*Host\(`([^`]+)`\).*/\1/')"
host="${host:-pixelflux.example.com}"
cat <<EOF

==> up. The service is host-routed through Traefik on :$HOST_PORT.
    curl -H "Host: $host" http://localhost:$HOST_PORT/
    # or add to /etc/hosts:  127.0.0.1 $host    then open http://$host:$HOST_PORT
EOF
