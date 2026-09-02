# foundation-001 review 02

## Judgment

`blocked`.

The five findings from `docs/plan/reviews/foundation-001-01.md` are closed, and the non-interactive verification suite passes. However, the current composer still permits submit during an active IME composition when the marked text is temporarily empty. This violates the task's explicit composition guard and blocks completion of `foundation-001` until fixed and independently reverified.

This judgment does not require `macos-arm64` to be `verified`. The available native evidence supports retaining `foundation-native-shell` and `macos-arm64` at `implemented`; the remaining manual/native gaps correctly prevent promotion to `verified`.

## Findings

### P2 blocking — Empty marked text loses the active-composition submit guard

- Contract: `docs/plan/tasks/foundation-001.md:57-62` requires marked-text handling and says `Enter` submits only when no composition is active. `docs/plan/tasks/foundation-001.md:89-90` explicitly requires focused coverage that active composition prevents submit. `docs/ui/main-window.md:70-79` repeats that incomplete composition must never submit.
- Code: `crates/deepchat-app/src/main.rs:168-175` records an active composition in `composition_snapshot`, but sets `marked_range` to `None` whenever `new_text` is empty. `crates/deepchat-app/src/main.rs:195-200` suppresses submit only when `marked_range.is_some()`.
- Evidence gap: `crates/deepchat-app/src/main.rs:1065-1078` tests only a manually populated non-empty `marked_range`; it does not exercise `replace_and_mark(..., "", ...)` while `composition_snapshot` remains active.
- Impact: after an IME marked-text update with an empty string and before unmark/commit, `submit()` increments `submit_attempts`. The implementation therefore equates “non-empty marked range” with “active composition,” although its own state model distinguishes them.
- Required fix: derive the submit guard from composition activity, including the empty-marked-text state, and add a focused regression test that enters composition through `replace_and_mark` with empty marked text before pressing Enter.

### P3 non-blocking — Architecture dependency decision omits the required text feature

- Task/UI contract: `docs/plan/tasks/foundation-001.md:45-53` and `docs/ui/main-window.md:28-30` require GPUI features `font-kit` and `macos-blade` on the audited host.
- Code: `Cargo.toml:11` correctly enables both features.
- Documentation: `docs/architecture.md:152` still describes the measured dependency decision as `features = ["macos-blade"]` only.
- Impact: the architecture source can direct a later dependency audit back to the known `NoopTextSystem` configuration.
- Required fix: align the architecture decision with the selected `font-kit` plus `macos-blade` feature set and its recorded license graph.

## Previous blocking findings

| Review 01 finding | Current status | Evidence |
|---|---|---|
| P1 — GPUI text is not rendered | Closed | `Cargo.toml:11` enables `font-kit`; `parity/evidence/foundation-native-shell/native-shell.png` visibly contains shell and placeholder glyphs. |
| P1 — Empty-composer hit testing can panic | Closed | `crates/deepchat-app/src/main.rs:402-418` maps an empty draft directly to offset zero; `crates/deepchat-app/src/main.rs:637-650` shapes the placeholder separately; regression test at `crates/deepchat-app/src/main.rs:1017-1027` passes. |
| P2 — Newline display changes the editable index domain | Closed | `crates/deepchat-app/src/main.rs:550-553` replaces each one-byte newline with one one-byte space; index/range regression tests at `crates/deepchat-app/src/main.rs:1081-1119` pass. The UI contract accurately limits this to a single visual line at `docs/ui/main-window.md:81`. |
| P2 — Transcript content change occurs during render | Closed | Product render at `crates/deepchat-app/src/main.rs:803-940` only observes scroll state; `content_changed` is test-only at `crates/deepchat-app/src/main.rs:790-795` and is invoked only by the fixture test at `crates/deepchat-app/src/main.rs:1121-1146`. |
| P3 — IME composition creates multiple undo entries | Closed | `crates/deepchat-app/src/main.rs:168-190` retains one pre-composition snapshot and commits one undo entry; regression test at `crates/deepchat-app/src/main.rs:1045-1062` passes. |

## Non-interactive verification

Run on 2026-09-02 from the repository root without launching or controlling the GUI:

| Check | Result |
|---|---|
| `cargo fmt --check` | PASS |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo test --workspace` | PASS — 9 passed, 0 failed |
| `uv run python tools/parity-audit/validate.py` | PASS — `parity contract: PASS` |

## Evidence assessment

- `parity/evidence/foundation-native-shell/native-shell.png` is `800 × 620` and visibly proves glyph rendering, documented empty-state ownership, and absence of fake product chats/providers.
- `parity/evidence/foundation-native-shell/native-shell-edited.png` is `800 × 620` and shows retained synthetic composer input.
- `parity/evidence/foundation-native-shell/native-shell-resized.png` is `650 × 500` and supports the no-minimum-size claim.
- The built binary is a Mach-O 64-bit arm64 executable.
- The evidence README explicitly leaves live native IME candidate interaction, platform clipboard shortcuts, product transcript scrolling, hidden-until-ready behavior, OS accessibility, and non-arm64 platforms unverified. Those gaps are consistent with `implemented`, not `verified`, and are not failures of the task's current manifest landing.
- The stored OCR text files are empty, so the README's OCR claim is not independently reproducible from those text artifacts. The screenshots themselves remain sufficient to establish visible glyphs for the `implemented` judgment, but not to strengthen the platform status to `verified`.

## Remaining uncertainty

- No GUI was launched or manipulated during this review, as required. Native runtime claims were assessed from the stored evidence only.
- Live IME behavior, clipboard shortcuts, and long-fixture scrolling remain manual/native verification gaps.
- Hidden-until-ready and OS accessibility remain explicit GPUI 0.2.2 gaps and are not claimed as passing.
