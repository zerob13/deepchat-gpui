# Parity Audit

`parity-audit` is the executable coverage gate for the frozen reference release.

## Required checks

- Validate `parity/reference-baseline.json` and `parity/manifest.json` schemas.
- Verify the frozen tag resolves to the recorded commit.
- Discover reference UI routes, settings routes, windows, deep links, shortcuts, typed routes/events, schema tables/migrations, provider/model/ACP registries, tools, MCP/plugin/skill surfaces, platform branches, tests, release notes, and packaging targets.
- Report every discovered surface that is not mapped to a manifest feature.
- Validate status transitions and evidence requirements.
- Require platform-specific verification for multi-platform features.

## Planned interface

```text
cargo run -p parity-audit -- discover --reference /path/to/deepchat-2
cargo run -p parity-audit -- check
cargo run -p parity-audit -- completion
```

The repository cannot be declared complete while `check` reports an unmapped surface or `completion` reports any non-terminal applicable feature.

## Baseline validator

The dependency-free validator checks JSON shape, status vocabulary, unique feature IDs, dependency references, canonical release/build platform IDs and their Rust mappings, `platformStatus` completeness, structured frozen-commit evidence paths, and status/gap invariants.

```sh
uv run python tools/parity-audit/validate.py
```

By default, frozen evidence is read from the local repository recorded in `parity/reference-baseline.json`. CI and other machines can point the validator at an exact checkout without changing the frozen baseline:

```sh
DEEPCHAT_REFERENCE_REPOSITORY=/path/to/deepchat-2 \
  uv run python tools/parity-audit/validate.py
```

The override repository must contain the recorded commit; every evidence path is still checked against that immutable commit.

Evidence objects use `{ "kind": "source|test|release-notes|workflow|configuration", "path": "repository-relative/path", "selector": "optional locator" }`. A feature with `platforms: ["all"]` must have an empty `platformStatus`; platform-specific features must enumerate every platform ID. `identified` means static evidence was located, not that behavior was implemented or verified.
