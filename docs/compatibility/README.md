# Provider compatibility records

These records capture bounded observations against exact provider artifacts. They are evidence for adapter design, not permanent end-to-end test scripts.

Rules:

- record release/commit identity, binary checksum, protocol/schema fingerprint, host architecture, commands exercised, and cleanup;
- distinguish observed facts from design decisions and remaining live checks;
- use disposable repositories and provider namespaces;
- never install global agent integrations, hooks, skills, aliases, or shell configuration during a spike;
- keep sanitized minimal fixtures in the owning adapter module once implementation begins;
- rerun only the affected provider record when its pin changes.

Current records:

- [`2026-08-04-br-bv.md`](2026-08-04-br-bv.md)
- [`2026-08-04-herdr.md`](2026-08-04-herdr.md)
