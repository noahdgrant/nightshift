---
unit: 9-import-supplier-feed
phase: triage
status: pass
base: main
---
## Agent Brief

**Summary:** Receive a supplier's delivery from its feed file: `inventory import-feed FILE`.

The supplier's portal exports each delivery as JSON. `tests/fixtures/supplier-feed.json` is a real export, downloaded unchanged. Its `lines` hold one `sku` and `quantity` each.

**Acceptance criteria:**
- [ ] `inventory import-feed tests/fixtures/supplier-feed.json` receives every line's quantity and prints one line per feed line, in the format `receive` prints: `BOLT-M6: 50 on hand`.
- [ ] A feed with an unknown SKU or a quantity below 1 exits non-zero with an error, and receives nothing from that feed.

**Out of scope:** any other command's behaviour or defaults.
