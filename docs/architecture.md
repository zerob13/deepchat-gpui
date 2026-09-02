# Architecture

## System boundary

DeepChat GPUI is a native Rust desktop client. UI state and native-window behavior belong to the GPUI process. Durable data, provider execution, tools, managed runtimes, browser-semantic surfaces, and generated-code execution cross explicit typed boundaries.

```text
┌──────────────────────────────── GPUI process ────────────────────────────────┐
│ Native window shell                                                        │
│  ├─ tabs / sidebar / workspace / panels / settings                         │
│  ├─ composer / virtualized transcript / notifications                      │
│  └─ application state + typed commands/events                              │
└───────────────┬──────────────────────┬──────────────────────┬────────────────┘
                │                      │                      │
        ┌───────▼────────┐     ┌───────▼────────┐     ┌──────▼────────────┐
        │ Application     │     │ WebView hosts   │     │ Helper processes │
        │ services        │     │ MCP Apps        │     │ QuickJS Code Mode│
        │ storage/provider│     │ artifacts/OAuth │     │ ACP/MCP/runtime   │
        │ tools/scheduler │     │ browser pages   │     │ isolated workers  │
        └───────┬────────┘     └────────────────┘     └───────────────────┘
                │
        ┌───────▼────────────────────────────────────────────────────────┐
        │ SQLCipher database + platform secret storage + file artifacts │
        └────────────────────────────────────────────────────────────────┘
```

## Crate direction

The workspace follows one-way dependencies. UI does not own persistence or provider protocols.

```text
deepchat-app ──► deepchat-ui ──► deepchat-core
      │                 │               │
      ├─────────────────┴──────────────► deepchat-services
      │                                 │
      └────────────────────────────────► deepchat-platform

helper binaries ──► deepchat-contracts / deepchat-core
```

Initial crates are introduced only when the first concrete caller exists. `foundation-001` therefore starts with the single real crate `crates/deepchat-app`; it must not create empty boundary crates. The intended ownership boundaries, introduced only with concrete callers, are:

| Boundary | Ownership |
|---|---|
| `deepchat-core` | Domain values, invariants, commands, events, provider-neutral stream events |
| `deepchat-services` | Persistence, providers, tools, scheduler, import/export, recovery workflows |
| `deepchat-platform` | Native windows, secrets, tray, protocol handlers, updater, PTY, file watching |
| `deepchat-ui` | GPUI views, theme, focus, accessibility, IME, transcript virtualization |
| `deepchat-app` | Composition root, lifecycle, dependency injection, single-instance coordination |
| helper binaries | QuickJS, ACP/MCP subprocesses, isolated managed runtime work |

These are boundaries, not a requirement to create empty crates before they have implementations.

## Typed boundary contract

Every cross-component request has:

- A stable name and version.
- Serializable input and output schemas.
- Explicit error variants.
- Bounded payload size and timeout where applicable.
- Cancellation and shutdown semantics.
- Tests proving unknown variants and malformed payloads fail closed.

The reference's 564 route contracts and 141 event contracts are discovery inputs. The Rust application may consolidate internal implementation, but may not erase user-visible behavior or serialized compatibility.

## Window and UI architecture

Ordinary UI remains native GPUI. The initial shell contract is:

```text
┌──────────────────────────── DeepChat window ─────────────────────────────┐
│ traffic lights / title region                         window controls     │
├──────────────┬───────────────────────────────────────────┬───────────────┤
│ Sidebar      │ Transcript / empty conversation           │ Side panel    │
│              │                                           │ (conditional) │
│ Workspaces   │  virtualized messages                     │               │
│ Sessions     │                                           │ artifacts /   │
│ Search       │                                           │ browser /     │
│              ├───────────────────────────────────────────┤ workspace     │
│ Settings     │ Composer                                  │               │
│              │ model · tools · attachments · send        │               │
└──────────────┴───────────────────────────────────────────┴───────────────┘
```

- Fresh default main-window size: `800 × 620` logical pixels; this is not a minimum, and the main window has no minimum-size contract.
- The independent Settings window is a later slice: fresh default `1300 × 800`, minimum `900 × 640`.
- Transcript owns exactly one scroll handle; streaming and layout changes do not write scroll position after the user leaves the bottom.
- Composer implements `EntityInputHandler`, including UTF-8/UTF-16 range conversion and marked text, and never submits an incomplete composition.
- Side panels and dialogs preserve deterministic focus restoration.
- Light and dark themes use semantic tokens rather than component-local colors.
- The reference's hidden-until-`ready-to-show` lifecycle has no verified GPUI 0.2.2 per-window reveal equivalent and remains an explicit parity gap.
- GPUI 0.2.2 has no verified public OS accessibility-tree API for this slice. Semantic label data may be modeled, but OS accessibility parity must not be claimed.

## Storage

The target preserves reference-compatible data invariants while using idiomatic Rust representations.

- Primary database: SQLCipher-compatible SQLite.
- Schema changes: numbered, transactional migrations plus startup integrity checks.
- Backups and corrupt-database retention are explicit workflows.
- Secrets: platform credential storage; encrypted database fields are not a substitute for platform secret storage where the reference uses it.
- Tests use generated fixtures only.

## Provider execution

Provider adapters emit one provider-neutral stream:

```text
started → text/reasoning/tool-call/media/usage* → completed | failed | cancelled
```

Retry is bounded and abortable. A request is never replayed after observable text, reasoning, or tool output has committed unless the reference contract explicitly requires a distinct retry branch. Journaling persists enough state to recover interrupted sessions.

## Generated code

Code Mode is an isolated helper subprocess with QuickJS. The host contract preserves:

- Versioned JSON messages.
- Opaque bindings and an explicit tool whitelist.
- Source, output, store, and nested-call limits.
- Memory limit and interrupt handler.
- Heartbeat, deadline, cancellation, forced kill, and idempotent cleanup.
- No ambient filesystem, process, network, environment, or credential access.

## WebView policy

Allowed:

- MCP Apps.
- HTML/React artifacts.
- OAuth flows.
- Real browser pages and browser automation surfaces.

Disallowed:

- Chat shell, transcript, composer, sidebar, settings, dialogs, menus, or ordinary controls.

Each allowed host has origin isolation, navigation policy, permission policy, CSP/sandbox policy, bounded lifetime, and explicit teardown tests.

## Platform and licensing

The release matrix is macOS, Windows, and Linux on `aarch64` and `x86_64` as declared by the frozen reference. Platform-specific behavior is behind `deepchat-platform` interfaces and verified on native CI runners.

Before a dependency becomes part of a release build:

1. Generate the complete transitive dependency graph for every feature set.
2. Run license and source checks.
3. Reject GPL dependencies and undocumented source exceptions.
4. Record approved non-Apache permissive licenses in the distribution notice.

The measured dependency audit permits `gpui 0.2.2` on the current macOS Apple Silicon host with `default-features = false` and `features = ["font-kit", "macos-blade"]`; an offline check of that graph passed. `font-kit` is required because disabling GPUI defaults otherwise selects `NoopTextSystem` and suppresses glyph rendering. The default feature graph is not the selected policy and currently fails on this host because the separately downloadable Metal Toolchain is absent. This is a per-target decision: macOS x86_64, Windows, and Linux feature selections remain unaudited and must be verified independently before those platform statuses advance. The resolved macOS graph is not uniformly Apache/MIT/BSD: the distribution notice must record MPL-2.0 `cbindgen` and CC0-1.0 `hexf-parse` obligations. GPL dependencies remain disallowed.
