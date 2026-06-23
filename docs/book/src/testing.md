# Testing & Load

Pixelflux is tested at several levels — unit tests (in-memory), integration
tests against a **real Redis** via Testcontainers, API contract tests (Hurl +
OpenAPI), and load/benchmark tests (k6):

| Level                    | Command                 |
| ------------------------ | ----------------------- |
| Unit                     | `task test`             |
| Integration (real Redis) | `task test:integration` |
| API contract             | `task test:api`         |
| Load / benchmark         | `task bench`            |

{{#include ../../../load/README.md}}
