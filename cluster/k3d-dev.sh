#!/usr/bin/env bash
# Fast local loop — no Argo, no GHCR, no secrets:
#   k3d cluster + Nix-built image imported locally + manifests applied directly.
#
# Usage:  bash cluster/k3d-dev.sh   (or: task k3d:dev)
# Re-run after a code change to rebuild + reimport + roll out.
set -euo pipefail

CLUSTER="${CLUSTER:-pixelflux}"
HOST_PORT="${HOST_PORT:-8080}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# --- 1. cluster ------------------------------------------------------------
if k3d cluster list "$CLUSTER" >/dev/null 2>&1; then
  echo "==> k3d cluster '$CLUSTER' already exists"
else
  echo "==> creating k3d cluster '$CLUSTER' (host :$HOST_PORT -> Traefik :80)"
  k3d cluster create "$CLUSTER" -p "${HOST_PORT}:80@loadbalancer" --wait
fi

# --- 2. build + import the image (no registry) -----------------------------
echo "==> building the distroless image with Nix"
nix build .#container
docker load <result
echo "==> importing pixelflux:latest into the cluster"
k3d image import pixelflux:latest -c "$CLUSTER"

# --- 3. apply the manifests directly into the pixelflux namespace ----------
echo "==> applying k8s manifests (image stays the locally-imported pixelflux:latest)"
kubectl create namespace pixelflux --dry-run=client -o yaml | kubectl apply -f -
kubectl apply -k k8s/ -n pixelflux

# Existing pods keep the old image with IfNotPresent; force a fresh rollout.
kubectl -n pixelflux rollout restart deploy/pixelflux
kubectl -n pixelflux rollout status deploy/pixelflux --timeout=300s

echo
echo "==> up (direct, no GitOps). The service is host-routed through Traefik on :$HOST_PORT."
echo "    curl -H 'Host: pixelflux.example.com' http://localhost:$HOST_PORT/"
echo "    # or port-forward, no host header:  kubectl -n pixelflux port-forward svc/pixelflux 8081:80"
