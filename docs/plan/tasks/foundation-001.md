---
id: foundation-001
scope: foundation-native-shell
status: done
depends-on: [baseline-001]
---

# Native GPUI shell on macOS Apple Silicon

## Objective

Deliver the first real native GPUI vertical slice on the current macOS Apple Silicon host. The slice opens a usable DeepChat shell with the reference fresh-window bounds, native title region, sidebar, empty conversation region, transcript scroll foundation, and a fully editable composer. It does not implement persistence, providers, fake chat data, settings, OS accessibility parity, or cross-platform packaging.

## Context

- `docs/INDEX.md`
- `PORTING.md`
- `docs/architecture.md`
- `docs/ui/main-window.md`
- `parity/reference-baseline.json`
- `parity/manifest.json` (`foundation-native-shell`)
- Frozen reference commit `ca75acfdc680fa3d0a2bbde13575fa711d08a3bd`

## Path

- `Cargo.toml`
- `Cargo.lock`
- `crates/deepchat-app/`
- `parity/evidence/foundation-native-shell/`
- `parity/manifest.json`
- `docs/ui/main-window.md`

Do not create empty `deepchat-core`, `deepchat-services`, `deepchat-platform`, or `deepchat-ui` crates. Split a crate only when a concrete second caller requires the boundary.

## Contracts

### Window

- Use a fresh default content size of `800 × 620` logical pixels. This is not a minimum size.
- Do not set a main-window minimum size.
- Use macOS transparent titlebar/inset traffic-light treatment where GPUI 0.2.2 exposes it.
- The reference's `show: false → ready-to-show → show/focus` lifecycle has no verified GPUI 0.2.2 equivalent. Do not claim hidden-until-ready parity; record it as a remaining gap.
- The independent Settings window (`1300 × 800`, minimum `900 × 640`) is outside this task.

### Dependency features

On the current macOS Apple Silicon host use:

```toml
gpui = { version = "0.2.2", default-features = false, features = ["font-kit", "macos-blade"] }
```

This is a host-specific audited choice, not a cross-platform policy. `font-kit` is required because disabling GPUI defaults otherwise selects `NoopTextSystem` and suppresses glyph rendering; `macos-blade` avoids the default Metal graph that currently fails on this host without the separately downloadable Metal Toolchain. Windows, Linux, and macOS x86_64 feature sets remain unaudited.

### Composer input

- Implement the GPUI `EntityInputHandler` contract, not a key-only text buffer.
- Keep UTF-8 storage and convert selections to/from the UTF-16 ranges required by platform text input.
- Support marked text, selected marked ranges, replacement ranges, bounds queries, and unmarking.
- `Enter` submits only when no composition is active; `Shift+Enter` inserts a newline.
- Support platform-equivalent Select All, Copy, Paste, Cut, Undo, and Redo actions (`Cmd` on macOS; action definitions must remain portable to `Ctrl`).
- Preserve the draft on invalid or unavailable submit; this task has no provider and therefore must not fabricate a successful send.

### Transcript scroll

- The transcript owns exactly one `ScrollHandle`.
- `follow_tail` is enabled only while the user is at the bottom.
- Once the user scrolls away, streaming-style row insertion or layout changes in test fixtures must not write the scroll position.
- Static long rows are allowed only behind test/demo fixtures used to validate scrolling. Product empty state must not invent chats, assistant replies, providers, or model availability.

### Accessibility boundary

GPUI 0.2.2 has no verified public OS accessibility-tree API for this slice. Keep semantic labels/roles in the view model where useful, but do not claim VoiceOver or OS accessibility parity. Record the gap in manifest evidence.

## Verification

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo run -p deepchat-app
uv run python tools/parity-audit/validate.py
```

Native evidence on the current host must additionally prove:

1. A fresh launch opens at `800 × 620` logical pixels and the window can be resized below that size.
2. The native shell renders the documented empty-state region ownership without provider/chat fixtures.
3. Composer editing, UTF-8/UTF-16 selection conversion, marked-text replacement, Enter/Shift+Enter, clipboard actions, and undo/redo pass focused tests.
4. Active composition prevents submit.
5. A long transcript fixture scrolls independently; after leaving the bottom, fixture row/layout updates preserve the observed scroll position.
6. The app starts and exits cleanly on macOS Apple Silicon.
7. Hidden-until-ready and OS accessibility remain explicitly unverified; the evidence must not present them as passing.

Store reproducible commands, output, host metadata, and screenshots under `parity/evidence/foundation-native-shell/`. Set only `macos-arm64` to `implemented` or `verified` when its evidence supports that claim; all other platform entries remain `identified`.
