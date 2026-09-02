# foundation-001 review 03

## Judgment

`pass`.

The blocking active-composition defect from review 02 is fixed. Empty marked text now retains composition activity through `composition_snapshot`, submit is guarded by that activity rather than by a non-empty `marked_range`, and `unmark` ends the composition before later submit attempts are accepted. The regression test enters this state through the production `replace_and_mark` transition instead of manually populating composition fields. The architecture dependency decision now consistently records both required GPUI features.

This judgment keeps `foundation-native-shell` and `macos-arm64` at `implemented`, not `verified`. The documented manual/native gaps still prevent promotion to `verified` but do not block the current task state.

## Findings

No blocking or non-blocking findings.

| Severity | Blocking | Location | Finding |
|---|---:|---|---|
| None | No | — | No correctness, test-validity, documentation-consistency, or parity-manifest defect found in the reviewed scope. |

## Review 02 findings

| Review 02 finding | Current status | Evidence |
|---|---|---|
| P2 blocking — Empty marked text loses the active-composition submit guard | Closed | `crates/deepchat-app/src/main.rs:168-175` starts/retains composition through `composition_snapshot` even when `new_text` is empty. `crates/deepchat-app/src/main.rs:195-200` rejects submit while that snapshot exists. `crates/deepchat-app/src/main.rs:187-193` clears composition activity on unmark. The regression at `crates/deepchat-app/src/main.rs:1065-1083` calls `replace_and_mark(None, "", Some(0..0))`, proves the marked range is empty while composition remains active, proves submit is suppressed without incrementing `submit_attempts`, then calls `unmark` and proves a subsequent submit attempt is accepted by the state machine. The test does not manually fabricate `marked_range` or `composition_snapshot`. The production GPUI paths delegate to these same transitions at `crates/deepchat-app/src/main.rs:393-396`, `crates/deepchat-app/src/main.rs:483-485`, and `crates/deepchat-app/src/main.rs:499-508`. |
| P3 non-blocking — Architecture dependency decision omits the required text feature | Closed | `docs/architecture.md:152` records `default-features = false` with `features = ["font-kit", "macos-blade"]` and explains the `NoopTextSystem` failure avoided by `font-kit`. This agrees with `docs/plan/tasks/foundation-001.md:45-53`, `docs/ui/main-window.md:28-30`, and `parity/manifest.json:62`. |

## Contract assessment

- Empty marked text and active composition are correctly represented as distinct facts: `marked_range` may be `None` while `composition_snapshot` remains `Some`.
- Submit eligibility is derived from composition activity, so `replace_and_mark(..., "", ...)` cannot submit prematurely.
- `unmark` consumes `composition_snapshot`, clears `marked_range`, and makes the next submit attempt eligible while preserving the draft.
- The focused regression reaches the defective state through the production state transition and checks observable submit-attempt behavior; it is not a manually forged-state test.
- The manifest remains internally consistent: the feature and `macos-arm64` statuses are `implemented`, required native/manual checks remain listed, and hidden-until-ready plus OS accessibility remain explicit gaps.

## Non-interactive verification

Run on 2026-09-02 from the repository root without launching or controlling the GUI:

| Check | Result |
|---|---|
| `cargo fmt --check` | PASS |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo test --workspace` | PASS — 9 passed, 0 failed |
| `uv run python tools/parity-audit/validate.py` | PASS — `parity contract: PASS` |
| `git diff --check` | PASS |

## Remaining uncertainty

- No GUI was launched or manipulated during this review, as required.
- Live native IME candidate interaction, platform clipboard shortcuts, and long-fixture scrolling remain manual/native verification gaps.
- Hidden-until-ready behavior and OS accessibility remain explicit GPUI 0.2.2 gaps and are not claimed as passing.
- The regression validates the production state transitions and their GPUI delegation, but it does not synthesize a native IME session or native Enter key event; that remains part of the documented manual/native verification boundary.
