#!/usr/bin/env bash
# Tear down the local k3d cluster.
# Usage:  bash cluster/k3d-down.sh   (or: task k3d:down)
set -euo pipefail

CLUSTER="${CLUSTER:-pixelflux}"
echo "==> deleting k3d cluster '$CLUSTER'"
k3d cluster delete "$CLUSTER"
