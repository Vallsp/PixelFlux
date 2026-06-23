# Architecture Decision Records

Significant architecture decisions are recorded as lightweight
[ADRs](https://adr.github.io/) — each captures the context, the decision, and
its consequences, so newcomers understand _why_ things are the way they are. To
change a decision, a new ADR supersedes the old one rather than rewriting
history.

| #               | Decision                                          | Status   |
| --------------- | ------------------------------------------------- | -------- |
| [0001](0001.md) | Use Nix for the dev shell and the container build | Accepted |
| [0002](0002.md) | Ship a distroless, non-root, static container     | Accepted |
| [0003](0003.md) | Back the canvas with Redis and pub/sub            | Accepted |
| [0004](0004.md) | Use Server-Sent Events instead of WebSockets      | Accepted |
| [0005](0005.md) | Deploy on Kubernetes behind Traefik               | Accepted |
| [0006](0006.md) | Publish documentation with mdBook                 | Accepted |
