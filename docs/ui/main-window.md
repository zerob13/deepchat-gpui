# Main Window Layout

Status: Current contract for `foundation-001`.

## First-slice boundary

The first native slice is a real GPUI application shell on macOS Apple Silicon. It contains a native window, sidebar, empty conversation region, transcript scroll foundation, and editable composer. It intentionally contains no provider execution, persisted chats, fake model availability, settings window, WebView, or generated assistant messages.

```text
┌──────────────────────── fresh default: 800 × 620 ────────────────────────┐
│ macOS traffic lights / native title region                               │
├──────────────┬─────────────────────────────────────────────────────────────┤
│ Sidebar      │ Conversation                                               │
│ 240 px       │                                                            │
│              │             DeepChat                                       │
│ + New chat   │        Start a conversation                                │
│ Search       │                                                            │
│              │                                                            │
│ No chats yet │ ┌─────────────────────────────────────────────────────────┐ │
│              │ │ Composer (native text input / IME)                     │ │
│              │ │ DeepChat · Agent mode                  Send unavailable │ │
│ Settings     │ └─────────────────────────────────────────────────────────┘ │
└──────────────┴─────────────────────────────────────────────────────────────┘
```

`800 × 620` is a fresh default, not a minimum. The main window must remain resizable below that size. The independent Settings window is a later slice with a fresh default of `1300 × 800` and a minimum of `900 × 640`.

The reference opens hidden, waits for `ready-to-show`, then shows and focuses. GPUI 0.2.2 has no verified public per-window reveal API, so hidden-until-ready parity is a recorded gap rather than a claim of this slice.

On the audited macOS Apple Silicon host, GPUI uses `default-features = false` with `features = ["font-kit", "macos-blade"]`. `font-kit` supplies real glyph shaping/rendering instead of `NoopTextSystem`; `macos-blade` avoids the unaudited default Metal Toolchain dependency. This feature set is host-specific and does not define policy for other targets.

## Future full-shell structure

The region model stays compatible with the later full shell:

```text
┌────────────────────────────── DeepChat ───────────────────────────────────┐
│ Native title region / tab strip                              Window chrome │
├──────────────┬──────────────────────────────────────────┬──────────────────┤
│ Sidebar      │ Conversation                            │ Side panel       │
│ 240 px       │ flexible, min 360 px                    │ conditional      │
│              │ ┌──────────────────────────────────────┐ │ 320–480 px       │
│ Workspace    │ │ Transcript                           │ │ Artifact/browser │
│ Sessions     │ │ virtualized, independently scrolled │ │ /workspace       │
│ Search       │ └──────────────────────────────────────┘ │                  │
│ Settings     │ ┌──────────────────────────────────────┐ │                  │
│              │ │ Composer                             │ │                  │
│              │ └──────────────────────────────────────┘ │                  │
└──────────────┴──────────────────────────────────────────┴──────────────────┘
```

The side panel, workspace selector, persisted sessions, and Settings navigation are not implemented by `foundation-001`.

## Region ownership

| Region | Owns in the first slice | Does not own |
|---|---|---|
| Window shell | Native lifecycle, fresh default bounds, title region, global action dispatch | Persistence, provider execution, hidden-until-ready parity |
| Sidebar | Static navigation structure and focus targets | Persisted workspaces/sessions |
| Transcript | Row layout fixture in tests, exactly one `ScrollHandle`, `follow_tail` state | Composer draft or provider stream |
| Composer | Text, selection, IME marked text, clipboard, undo/redo, submit intent | Provider execution or fabricated success |
| Side panel | Nothing yet | All artifact/browser/workspace content |

## Composer input contract

The composer implements GPUI's `EntityInputHandler`, including UTF-8 storage with correct conversion to and from platform UTF-16 ranges.

| Input | Behavior |
|---|---|
| `Enter` | Emit submit intent only when no marked-text composition is active; unavailable submit preserves the draft |
| `Shift+Enter` | Insert newline |
| `Cmd/Ctrl+A` | Select all |
| `Cmd/Ctrl+C` | Copy selection |
| `Cmd/Ctrl+V` | Paste at selection/replacement range |
| `Cmd/Ctrl+X` | Cut selection |
| `Cmd/Ctrl+Z` | Undo |
| `Cmd/Ctrl+Shift+Z` | Redo |
| IME update | Set marked text and selected marked range without submission |
| IME commit/cancel | Replace/unmark according to `EntityInputHandler`; never lose unrelated text; one composition is one undo unit |

Placeholder shaping is presentation-only and never contributes editable offsets. Editable text, hit testing, selection, caret geometry, marked ranges, and IME bounds share the stored UTF-8 byte-index domain. In this first slice, newline bytes are rendered as same-length spaces in the single visual line; this is not a multiline layout claim, but it keeps Shift+Enter and every input range in one honest index domain.

Focused tests must cover multibyte Unicode, surrogate-pair UTF-16 offsets, replacement ranges, marked text, and active-composition Enter suppression.

## Transcript scroll contract

The transcript owns one `ScrollHandle`. `follow_tail` is true only while the viewport is at the bottom.

```text
at bottom ── new/layout-updated row ──► follow tail
    │
    └─ user scrolls away ──► preserve observed scroll offset
                                  │
                                  └─ user returns to bottom ──► follow tail
```

After the user leaves the bottom, row insertion, draft growth, or layout changes must not write the transcript scroll position. Static long rows may exist only as test/demo fixtures; the product empty state must not display invented messages.

## Focus and accessibility

- Launch focus goes to the composer after the shell is ready unless a real recovery surface later takes priority.
- `Tab` and `Shift+Tab` traverse visible controls in semantic order.
- `Esc` closes the topmost dismissible layer when such a layer exists and restores prior focus.
- Icon-only controls carry semantic label data and tooltips.
- GPUI 0.2.2 has no verified public OS accessibility-tree API for this slice. Semantic role/label data is preparatory only; VoiceOver and OS accessibility parity remain blocked and must not be reported as implemented.

## State variants

- **Empty**: stable shell, no sessions, editable composer, Send unavailable because no provider slice exists.
- **Input active**: composer retains draft, selection, focus, undo history, and marked text.
- **Submit unavailable**: emit no fake message and preserve the draft.
- **Constrained**: no main-window minimum; sidebar may collapse before the composer violates its own control layout.
- **Fixture transcript**: test-only long rows exercise scrolling and layout updates without representing product data.
- **Error**: typed local error in its owning region; user input is never silently cleared.
