# foundation-native-shell evidence

## Judgment

`macos-arm64` is **implemented**, not verified. All blocking findings from `foundation-001` review 01 are closed. The native GPUI shell visibly renders glyphs, empty-composer hit testing stays in the empty editable index domain, newline visualization preserves byte indices, rendering does not mutate transcript scroll position, and one IME composition produces one undo unit.

## Host

```text
Date: 2026-09-02
OS: macOS 26.6.2 (25G83)
Kernel: Darwin 25.6.0 arm64
Rust host: aarch64-apple-darwin
rustc: 1.94.0 (4a4ef493e 2026-03-02)
cargo: 1.94.0 (85eff7c80 2026-01-15)
Binary: Mach-O 64-bit executable arm64
GPUI: 0.2.2, default-features=false, features=["font-kit", "macos-blade"]
```

## Reproducible commands and results

| Command/check | Result |
|---|---|
| `cargo fmt --check` | PASS |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo test --workspace` | PASS — 9 passed, 0 failed |
| `uv run python tools/parity-audit/validate.py` | PASS — `parity contract: PASS` |
| `git diff --check` | PASS |
| `cargo build -p deepchat-app` and native launch | PASS |
| CUA + OCR on `native-shell.png` | PASS — visible English shell labels and `Message DeepChat` placeholder |
| Empty composer middle click followed by text input | PASS — `middle click input` rendered; process remained live |
| `Shift+Enter`, then continued input | PASS — `continued` rendered and process remained live |
| CUA resize | PASS — `800 × 620` to `650 × 500`, proving no default-size minimum |
| CUA `Cmd+Q`, process/window checks | PASS — zero windows and no process remained |

Automated tests cover:

- UTF-8 storage and UTF-16 conversion without splitting surrogate pairs.
- Placeholder hit testing mapping every candidate offset to editable byte offset zero for an empty draft, followed by safe insertion.
- Newline visualization preserving byte length and valid selection/marked/IME range conversion.
- Marked-text replacement and relative selection.
- One composition with multiple marked-text updates coalescing into one undo unit.
- Active-composition Enter suppression and unavailable-submit draft preservation.
- Unicode-safe newline insertion, clipboard-style replacement, undo, and redo.
- Test-fixture transcript mutation preserving scroll offset away from tail.
- Product rendering contains no `content_changed()` call and therefore performs no scroll write during render.

## Native screenshots

- `native-shell.png`: fresh native window at `800 × 620`; visible English shell labels and placeholder; product empty state contains no fake chats.
- `native-shell-edited.png`: text remains visible after empty-field middle click, input, `Shift+Enter`, and continued editing.
- `native-shell-resized.png`: same process resized to `650 × 500` with visible shell content.

Screenshots contain only synthetic shell labels and test input. They contain no provider credentials, personal chats, model claims, or fabricated provider data.

## Index-domain contract

The placeholder is shaped and painted separately and is never stored as the editable line. Empty-draft hit testing always returns byte offset zero. Editable text, hit testing, caret, selection, marked ranges, and IME bounds share the stored UTF-8 byte domain. This slice remains a single visual line: each newline is displayed as one ASCII space, which preserves byte length and is explicitly not claimed as multiline layout parity.

## Known gaps and uncertainty

- Hidden-until-ready lifecycle remains unverified and not implemented.
- OS accessibility tree / VoiceOver parity remains unverified and not implemented.
- Live native IME candidate interaction and platform clipboard shortcuts were not manually exercised. IME state transitions and composition undo coalescing are covered at the Rust contract level.
- The product state intentionally contains no transcript rows. Independent scroll behavior is exercised only by test fixture/state tests.
- macOS x86_64, Windows, and Linux feature graphs and native behavior remain unaudited/identified.
