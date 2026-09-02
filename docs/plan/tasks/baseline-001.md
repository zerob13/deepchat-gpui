---
id: baseline-001
scope: full-parity-audit
status: done
depends-on: []
---

# Frozen baseline contract

Baseline schema, evidence contract, platform naming, validator, and planning foundation. No Rust implementation.

## Acceptance criteria

1. `reference-baseline.json` and `manifest.json` agree on frozen commit, platform IDs, status vocabulary, and evidence object shape.
2. All evidence paths resolve at commit `ca75acfdc680fa3d0a2bbde13575fa711d08a3bd`.
3. The dependency-free validator passes.
4. All 23 manifest feature IDs are represented in the roadmap and implementation sequence.

## Verification

```sh
uv run python tools/parity-audit/validate.py
```

Implementation status remains in `parity/manifest.json`; this task file must not become a second tracker.
