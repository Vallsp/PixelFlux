// k6 load test.
// Run with: task bench   (boots the server, then runs this script)
import http from "k6/http";
import { check, sleep } from "k6";

export const options = {
  stages: [
    { duration: "10s", target: 20 },
    { duration: "20s", target: 50 },
    { duration: "10s", target: 0 },
  ],
  thresholds: {
    http_req_failed: ["rate<0.01"], // <1% errors
    http_req_duration: ["p(95)<200"], // 95% under 200ms
  },
};

const BASE = __ENV.BASE_URL || "http://localhost:3000";
const headers = { "Content-Type": "application/json" };

export default function () {
  // Read the canvas
  const canvas = http.get(`${BASE}/api/canvas`);
  check(canvas, {
    "canvas 200": (r) => r.status === 200,
    "canvas has pixels": (r) => typeof r.json("pixels") === "string",
  });

  // Paint a random pixel
  const body = JSON.stringify({
    x: Math.floor(Math.random() * 64),
    y: Math.floor(Math.random() * 64),
    color: Math.floor(Math.random() * 16),
  });
  const paint = http.post(`${BASE}/api/pixel`, body, { headers });
  check(paint, { "pixel 200": (r) => r.status === 200 });

  sleep(1);
}
