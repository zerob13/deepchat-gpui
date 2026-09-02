# DeepChat GPUI Port

## Objective

Reimplement DeepChat as a release-ready Rust + GPUI desktop application while preserving the frozen reference release's user-visible behavior, serialized data contracts, migrations, security boundaries, recovery semantics, shortcuts, and supported platforms.

## Frozen reference

| Field | Value |
|---|---|
| Repository | `/Users/colab/Documents/workspace/deepchat-2` |
| Remote | `https://github.com/ThinkInAIXYZ/deepchat.git` |
| Tag | `v1.1.1` |
| Commit | `ca75acfdc680fa3d0a2bbde13575fa711d08a3bd` |
| Product version | `1.1.1` |
| Commit timestamp | `2026-08-31T12:14:05+08:00` |
| Frozen on | `2026-09-02` |

All source, tests, release notes, and runtime comparisons use the full commit above. The port does not follow newer reference commits or releases automatically.

## Product support baseline

| Release/build platform ID | Rust target triple | Reference packages |
|---|---|---|
| `macos-arm64` | `aarch64-apple-darwin` | DMG, ZIP |
| `macos-x64` | `x86_64-apple-darwin` | DMG, ZIP |
| `windows-arm64` | `aarch64-pc-windows-msvc` | NSIS installer |
| `windows-x64` | `x86_64-pc-windows-msvc` | NSIS installer |
| `linux-arm64` | `aarch64-unknown-linux-gnu` | AppImage, tar.gz |
| `linux-x64` | `x86_64-unknown-linux-gnu` | AppImage, tar.gz |

Release/build IDs are the canonical manifest and baseline identifiers. Rust target triples are the compilation mapping and must not be substituted for release IDs. Reference evidence: `electron-builder.yml` and `.github/workflows/_package-{macos,windows,linux}.yml` at the frozen commit.

## Fixed comparison environment

| Setting | Value |
|---|---|
| Host | macOS, Apple Silicon |
| Locale | `zh-CN` |
| Theme | Light unless a feature explicitly tests another theme |
| Main window | `800 × 620` logical pixels |
| Display | `1920 × 1080`, scale factor `1.0` |
| Test profile | Synthetic isolated profile only; never the user's real DeepChat profile |
| Network providers | Protocol fixtures/local fake servers by default; no paid requests |
| Feature flags | Reference release defaults unless the manifest item records an override |

Dynamic comparison uses disposable isolated profiles for both applications. Existing logged-in profiles, provider credentials, chat records, browser cookies, and personal paths are outside the test boundary.

## Required architecture

- Main windows and ordinary application UI use Rust + GPUI.
- WebView is restricted to browser-semantic surfaces such as MCP Apps, HTML/React artifacts, OAuth, and real browser pages.
- Code Mode runs model-generated code in a dedicated helper process backed by QuickJS. The GPUI process never evaluates generated code.
- The final application contains no temporary Node compatibility bridge unless the user explicitly approves it.
- Dependencies must remain compatible with the Apache-2.0 distribution boundary. GPL Zed UI/editor/terminal crates are not allowed.

The reference release currently implements Code Mode with a Node `UtilityProcess` and `node:vm`. This is a reference behavior oracle, not the target runtime architecture: the target requirement intentionally replaces it with QuickJS while retaining its observable limits, cancellation, timeout, heartbeat, nested-call, and cleanup semantics.

## Durable state

- `parity/reference-baseline.json`: machine-readable frozen product surfaces.
- `parity/manifest.json`: the only feature status tracker.
- `parity/evidence/<feature-id>/`: reproducible verification evidence.
- `tools/parity-audit/`: discovery and coverage audit.
- `docs/architecture.md`: current architecture and stable decisions.

Do not create a second progress tracker. Human-readable task planning, when needed for implementation coordination, must point back to manifest feature IDs and must not redefine feature status.

## Development and verification loop

1. Run the parity audit and reconcile the manifest.
2. Specify the highest-value unblocked vertical slice in the manifest.
3. Implement the complete slice without speculative abstractions.
4. Run format, lint, unit, contract, integration, UI, and packaging checks required by that slice.
5. Compare reference and target in the fixed environment, including actions, output, persistence, errors, recovery, cancellation, keyboard, focus, IME, and accessibility.
6. Store evidence and set the item to `verified` only when its reproducible checks pass.
7. Commit the verified slice with an English Conventional Commit and push `main`.
8. Run regression and parity audit before selecting the next slice.

## Current status

- The target repository was empty at discovery time; development and delivery are locked to `main`.
- The reference baseline and first static discovery pass are frozen at the commit above.
- `foundation-native-shell` is implemented on macOS Apple Silicon but is not verified while native IME/clipboard/scroll, hidden-until-ready, and OS-accessibility gaps remain.
- `storage-sqlcipher` foundation, static catalog, schema diagnosis/repair, production repair hooks, and startup one-shot recovery are implemented: `storage-001`, `storage-002a-1`, and `storage-002a-2` are complete. FTS/projection, backup/import, encryption-change, and integration parity remain deferred to later `storage-002` slices.
- The application is not release-ready; all other completion claims come only from `parity/manifest.json` and reproducible evidence.

## Recovery

On resume, read this file, `docs/architecture.md`, `parity/reference-baseline.json`, and `parity/manifest.json`; then inspect `git status`, the latest commits, and unpushed commits. Continue from the highest-priority unblocked manifest item. Do not infer completion from conversation history.
