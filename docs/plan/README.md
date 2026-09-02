# Delivery plan

This directory coordinates implementation without duplicating feature status. `parity/manifest.json` is the only status tracker; every task names manifest feature IDs and records sequencing, contracts, and verification intent only.

- [Porting roadmap](analysis/porting-roadmap.md)
- [baseline-001](tasks/baseline-001.md)
- [foundation-001](tasks/foundation-001.md)
- [storage-001](tasks/storage-001.md)
- [storage-002](tasks/storage-002.md)
- [storage-002a-1](tasks/storage-002a-1.md)
- [storage-002a-2](tasks/storage-002a-2.md)
- [Backlog](backlog.md)

Use the loop: select an unblocked manifest feature, write the smallest independently verifiable task, implement its complete vertical slice, verify it, store evidence under `parity/evidence/<feature-id>/`, then update the manifest. Task status coordinates delivery only; feature/platform completion remains exclusively in the manifest.
