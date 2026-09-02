# baseline-001 Review 01

## Judgment

**BLOCKED.** The repository documents most of the intended baseline contract and the current validator prints `PASS` on canonical inputs, but the validator does not enforce several blocking requirements. A malformed baseline can therefore pass the gate.

## Findings

### P1 — Blocking: validator does not validate the baseline schema or canonical platform mapping

- **Location:** `tools/parity-audit/validate.py:13-20`, `tools/parity-audit/validate.py:29-32`
- **Evidence:** The validator reads `baseline['reference']['commit']`, status vocabulary, and manifest fields, but never validates `baseline.schemaVersion`, `baseline.reference` field shape, `baseline.platforms`, `baseline.evidenceContract`, comparison environment, surfaces, contract counts, or discovery/platformStatus vocabulary. The hard-coded `PLATFORMS` mapping is only used to validate manifest platform names; it is never compared with `baseline.platforms[*].rustTarget`.
- **Mutation proof:** In a temporary copy, changing `baseline.platforms[0].rustTarget` to `WRONG` still returned `parity contract: PASS` (exit 0). Removing `baseline.schemaVersion` also returned `PASS`.
- **Required correction:** Add explicit shape/type checks for both JSON documents and compare the complete canonical release-ID-to-Rust-target mapping between the baseline and manifest/validator contract. Reject missing or extra platform entries and malformed baseline sections before indexed access can raise a `KeyError`.

### P1 — Blocking: evidence selector practicality and exact source traceability are not enforced

- **Location:** `tools/parity-audit/validate.py:33-36`; `parity/reference-baseline.json:60-65`; `parity/manifest.json` feature evidence entries
- **Evidence:** Evidence validation checks only that an object has `kind`/`path`, that `kind` is known, and that the frozen Git path exists. It does not require a selector where a source/count claim needs one, does not locate supplied selectors, and accepts directory paths as evidence. A temporary mutation changing the first manifest evidence selector to `NO_SUCH_SYMBOL` still returned `PASS`. The canonical manifest has a supplied selector `v1.1.0 and v1.1.1` in the `ocr-media-artifacts` changelog evidence that is not a meaningful exact locator (the frozen changelog contains the two headings separately).
- **Required correction:** Resolve the frozen object at the commit, reject directory-only evidence when a file/symbol is required, and validate selectors against practical exact symbols, test names, headings, or other locators. Enforce exact selectors for the route/event and 41/39/38 count records, and make the manifest evidence use meaningful selectors rather than prose combinations.

### P1 — Blocking: status invariants use the wrong field name and do not cover required data

- **Location:** `tools/parity-audit/validate.py:37-38`; manifest contract fields at `parity/manifest.json` (for example `foundation-native-shell`)
- **Evidence:** The manifest uses `remainingGaps`, but the validator checks `f.get('gaps')`. Consequently `blocked` and non-waived `verified`/terminal checks are evaluated against a field that is absent from every feature. The implementation also does not enforce the required data for `verified`, `waived`, and `blocked` beyond a partial `verification` check; it does not validate `remainingGaps` type, waiver reason, blocked reason, or evidence/verification requirements. Mutation proof: changing the first feature to `verified` and removing `verification` failed only because of the separate `verification` check; the error was `verified requires gaps/reason`, proving the field-name mismatch, while malformed status data and missing required reason structures are otherwise permissive.
- **Required correction:** Define and enforce status-specific contracts using `remainingGaps` (or rename consistently): `identified`/`specified` must not imply verification, `verified` must have reproducible verification and no remaining gaps, `blocked` must have a non-empty blocking reason/gap, and `waived` must have an explicit waiver rationale. Validate all relevant field types and reject status-vs-remainingGaps contradictions.

### P2 — Blocking: platformStatus semantics are incomplete and the special full-parity dependency contract is not explicit

- **Location:** `tools/parity-audit/validate.py:23-32`; `parity/manifest.json` `full-parity-audit` feature dependencies
- **Evidence:** The validator checks only key-set equality for platform-specific features and emptiness for `platforms: ["all"]`; it does not validate platformStatus values against the status vocabulary, does not reject malformed/non-object platformStatus, and does not state/enforce whether `full-parity-audit` must depend on every other manifest feature exactly once. It also does not validate that every canonical platform in `baseline.platforms` is represented. The existing full-parity dependency list appears to contain the other 22 IDs, and the roadmap contains all 23 IDs, but this is not a validator-enforced special rule.
- **Required correction:** Validate platformStatus object shape and per-platform status vocabulary/completeness against the canonical baseline mapping. Add an explicit rule that `full-parity-audit` depends on every other feature ID exactly once, while ordinary dependencies remain known IDs and do not self-reference.

### P2 — Non-blocking documentation precision: route/event selectors are too broad in the manifest

- **Location:** `parity/reference-baseline.json:60-65`; `parity/manifest.json` `full-parity-audit.referenceEvidence`
- **Evidence:** The baseline records count evidence with selectors `typed route definitions`, `typed event definitions`, `CATALOG_DEFINITIONS`, `createMainSchemaCatalog.createTables`, and `migrationTables (acpTurns excluded)`. The manifest instead uses `routes and events` for `src/shared/contracts`, which is not an exact symbol or test locator. The frozen reference has 46 route files and 30 event files; searching frozen source found 141 `defineEventContract` occurrences, while the claimed 564 route count requires the actual exported catalog rather than a generic directory phrase.
- **Required correction:** Preserve the baseline’s exact count selectors and add explicit source selectors for the manifest’s traceability claims, or clearly mark directory evidence as discovery scope rather than count evidence.

## Passing evidence

- `uv run python tools/parity-audit/validate.py` on canonical files: `parity contract: PASS` (exit 0).
- Frozen tag check: `git -C /Users/colab/Documents/workspace/deepchat-2 rev-parse v1.1.1^{commit}` returned `ca75acfdc680fa3d0a2bbde13575fa711d08a3bd`.
- Programmatic evidence-path audit against `git cat-file -e <commit>:<path>`: all manifest referenceEvidence paths resolved; 0 missing paths. This proves path existence only, not selector correctness.
- Programmatic roadmap audit: 23 manifest features and 23 exact roadmap table IDs; sets match.
- Platform IDs and Rust target mappings are consistently written in `PORTING.md:23-32` and `parity/reference-baseline.json:11-17`.
- Discovery wording is appropriately non-completion wording in `parity/reference-baseline.json:71-76`, `tools/parity-audit/README.md:32`, and `docs/plan/analysis/porting-roadmap.md:1-3`.
- The default `800 × 620` versus narrow regression `800 × 640` distinction is explicit in `parity/reference-baseline.json:28-30`, `PORTING.md:41`, and `docs/plan/analysis/porting-roadmap.md:44`.
- The roadmap states the manifest is the sole status tracker and contains all 23 IDs (`docs/plan/analysis/porting-roadmap.md:1-31`), so it is not a second status tracker.
- The GPUI audit decision and licensing obligations are recorded without claiming cross-platform completion in `docs/architecture.md:138-149`: macOS `gpui 0.2.2`, `default-features = false`, `macos-blade`; Windows/Linux feature selection remains unaudited; MPL-2.0 `cbindgen` and CC0-1.0 `hexf-parse` obligations; GPL disallowed.

## Commands and results

```text
uv run python tools/parity-audit/validate.py
# parity contract: PASS

# frozen tag
# ca75acfdc680fa3d0a2bbde13575fa711d08a3bd

# evidence path audit
# missing 0

# roadmap/manifest audit
# roadmap count 23; manifest feature count 23

# temporary mutation checks
# wrong baseline rustTarget -> PASS (unexpected)
# missing baseline schemaVersion -> PASS (unexpected)
# nonexistent evidence selector -> PASS (unexpected)
# verified feature with removed verification -> FAIL, but reports the wrong-field gaps error too
# incomplete platformStatus -> FAIL (one of the few enforced rules)
```

## Remaining uncertainty

- I did not alter canonical implementation or documentation. Mutation fixtures were temporary copies only.
- The exact authoritative route count algorithm should be pinned to the frozen exported route catalog (rather than inferred from file count or raw `defineRouteContract` occurrences) before adding the validator rule; the baseline’s declared count remains 564.
- I did not independently execute dynamic UI discovery, Rust commands, or a full dependency/license toolchain audit; this review is limited to the baseline artifacts and dependency-free validator contract.
