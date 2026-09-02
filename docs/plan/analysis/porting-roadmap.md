# Porting roadmap

The manifest is the sole status source. This roadmap names all 23 feature IDs and describes dependency/integration chains; it does not claim any feature is complete.

## Feature and integration chains

| Sequence | Manifest feature ID | Module chain | Integration chain |
|---:|---|---|---|
| 1 | `foundation-native-shell` | app → UI → platform | window → focus/IME → theme → accessibility |
| 2 | `storage-sqlcipher` | app → services → platform | schema → migrations → backup/recovery → security |
| 3 | `chat-session-stream` | session → provider → contracts | composer → request → stream → tape → persistence |
| 4 | `composer-input-attachments` | chat UI → draft model → file/media | IME → attachments → send → transcript |
| 5 | `rendering-rich-content` | message renderer → markdown/code/math | stream events → rendering → artifacts |
| 6 | `provider-model-registry` | provider adapters → registry → settings | credentials → model selection → stream |
| 7 | `agent-tape-tools` | agent → tape → tools | tool call → journal → replay/recovery |
| 8 | `window-platform-integrations` | windows → tray → shortcuts → deeplinks | single instance → lifecycle → native events |
| 9 | `workspace-terminal-files` | workspace → shell → PTY/files | project scope → watcher → terminal → tool policy |
| 10 | `managed-runtimes` | toolchains → runtime installer | settings → runtime discovery → isolated process |
| 11 | `code-mode-quickjs` | protocol → helper host → QuickJS | tool whitelist → limits → heartbeat → cancellation |
| 12 | `webview-sandbox` | browser/preview/MCP hosts → policy | origin → navigation → CSP → teardown |
| 13 | `mcp-apps-tools` | MCP transport → resources/prompts/tools/apps | registry → permissions → WebView bridge |
| 14 | `acp-agents` | ACP catalog → client/session runtime | runtime → auth → persistence → agent UI |
| 15 | `skills-library` | skill catalog → sync adapters | filesystem import → validation → plugin UI |
| 16 | `memory-knowledge` | memory data → knowledge services | provider/tool events → indexing → settings |
| 17 | `browser-computer-use` | browser → computer-use driver | WebView/browser → input → permission boundary |
| 18 | `ocr-media-artifacts` | OCR/media → artifact server/renderers | attachment → extraction → artifact viewer |
| 19 | `scheduler-remote-hooks` | scheduler → remote → hooks | cron → command authorization → notification |
| 20 | `settings-plugins-i18n` | navigation → settings components → i18n | route → settings state → plugin/locale resources |
| 21 | `cli-local-control` | CLI → local-control contracts | auth token → mutation guard → app dispatcher |
| 22 | `release-distribution` | packaging → CI workflows → updater | target triple → package → regression → release |
| 23 | `full-parity-audit` | audit tool → baseline/manifest/evidence | discovery → mapping → verification → completion gate |

## First actionable implementation sequence

1. Execute `foundation-001` on macOS Apple Silicon: a real resizable native shell with an 800×620 fresh default, complete GPUI text-input/IME handling, and single-owner transcript scrolling. Hidden-until-ready and OS accessibility remain explicit GPUI 0.2.2 gaps rather than passing checks.
2. Complete `storage-002b` backup/import/encryption workflows and the `storage-002` integration gate, then connect `chat-session-stream` persistence and provider-neutral events; catalog, repair, dynamic FTS, and projection slices are already delivered.
3. Add `composer-input-attachments` and `rendering-rich-content` as one user-visible chat slice.
4. Add `provider-model-registry`, then `agent-tape-tools`; preserve explicit cancellation and recovery semantics.
5. Implement `workspace-terminal-files` and `managed-runtimes`, enabling the isolated `code-mode-quickjs` helper.
6. Establish `webview-sandbox` policy before `mcp-apps-tools` and `browser-computer-use`.
7. Add `acp-agents`, `skills-library`, `memory-knowledge`, media/artifacts, scheduler/hooks, settings/i18n, and CLI in dependency order.
8. Finish platform integrations and packaging, then run `full-parity-audit` against the frozen commit.

The 800×640 viewport is only the frozen reference's narrow regression test; it is not the default window size. The main window has no minimum-size contract. The independent Settings window is a later slice with a 1300×800 fresh default and 900×640 minimum.
