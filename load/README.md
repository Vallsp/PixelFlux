# Load tests (k6)

[`health.js`](health.js) is a [k6](https://k6.io/) load test for the HTTP
server. It ramps virtual users up and down while reading the canvas and painting
random pixels, and asserts latency and error-rate thresholds.

## What it does

- **Stages:** 0→20 VUs over 10s, 20→50 VUs over 20s, 50→0 over 10s.
- **Per iteration:** `GET /api/canvas`, then `POST /api/pixel` with random
  `x`/`y`/`color`.
- **Thresholds (the test fails if breached):**
  - `http_req_failed` < 1% (error rate)
  - `http_req_duration` p95 < 200 ms

## Run it

```bash
task bench
```

The task builds the release binary, boots the server, runs k6 against it, and
stops the server afterwards.

To run it against an already-running server (local or remote), set `BASE_URL`:

```bash
BASE_URL=http://localhost:3000 k6 run load/health.js
# or against the deployed instance:
BASE_URL=https://your.domain.com k6 run load/health.js
```

## Tuning

Adjust the `stages` (load profile) and `thresholds` (pass/fail criteria) at the
top of `health.js`. Keep the thresholds realistic for the environment you test
against — a remote cluster will have higher latency than localhost.
