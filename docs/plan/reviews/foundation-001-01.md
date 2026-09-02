# foundation-001 review 01

## Findings

### P1 blocking — GPUI text is not rendered

- Contract: `docs/ui/main-window.md` requires visible shell labels, placeholder, and controls.
- Code: `Cargo.toml:11` enables only `macos-blade` while disabling default features.
- Evidence: both native screenshots contain shapes and caret but no glyphs; GPUI 0.2.2 selects `NoopTextSystem` without the `font-kit` feature.
- Required fix: enable the audited macOS `font-kit` feature, rebuild, and replace visual evidence after confirming visible text.

### P1 blocking — Clicking an empty composer can create an invalid selection and panic on input

- Code: `crates/deepchat-app/src/main.rs:555-601` shapes the placeholder as if it were the real text; hit-testing returns placeholder byte offsets, then `replace_range` applies them to the empty draft.
- Required fix: render placeholder separately from the editable shaped line. Empty-draft hit testing must always map to byte offset zero.

### P2 blocking — Newline display changes the editable index domain

- Code: `crates/deepchat-app/src/main.rs:558` replaces `\n` with ` ↵ `, while hit-testing, selection, marked ranges, caret geometry, and IME bounds continue to use draft byte offsets.
- Required fix: use one canonical text/index domain. Prefer real multiline shaping; at minimum never change byte length before using layout indices.

### P2 blocking — Transcript content change is invoked as a render side effect

- Code: `crates/deepchat-app/src/main.rs:743-744` calls `content_changed()` every render.
- Required fix: invoke tail-follow behavior only from actual test-fixture/product row or layout mutations. Rendering itself must not write scroll position.

### P3 non-blocking — IME composition produces multiple undo entries

- Code: `crates/deepchat-app/src/main.rs:154-174` snapshots on every marked-text update.
- Suggested fix: coalesce one marked-text composition into one undo unit committed at composition end.

## Conclusion

`blocked`. Keep `foundation-native-shell` and `macos-arm64` below `implemented`, and keep `foundation-001` in progress until all blocking findings are fixed and independently reverified.
