# AGENTS.md

Instructions for AI coding assistants contributing to `megh-rs`.

---

## 1. Dependencies: always at the edge

- **Always upgrade every dependency to its latest release**, majors included, direct and transitive. This is a standing decision: no approval is needed, and there is full freedom to take breaking releases and adapt the code.
- How: bump the requirement of every direct dependency in each workspace member (`Cargo.toml`, `megh-derive/Cargo.toml`), run `cargo update`, fix the code, then run the full test suite and `cargo audit` (must exit 0).
- **Only hold a dependency back** when its latest release is known to be buggy or broken, or is incompatible with another dependency we need. Record every hold-back in the table below with the reason and what to wait for, and re-check it on the next upgrade.
- Upgrades go in their own `chore(deps)` PR, not inside feature PRs.
- Never stage or delete untracked user files (review notes, scratch documents such as `docs/review.md`); stage explicit paths only.

### Held back

| Dependency | Held at | Latest | Reason | Waiting for |
|---|---|---|---|---|
| `matchit` (transitive) | 0.8.4 | 0.8.6 | `axum` 0.8.9 pins `=0.8.4` | the next `axum` release |
