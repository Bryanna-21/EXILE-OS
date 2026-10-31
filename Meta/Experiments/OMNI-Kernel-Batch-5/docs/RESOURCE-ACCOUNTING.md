# Resource accounting

| resource | accounted by | charged to | bound | released |
|---|---|---|---|---|
| endpoints (channels) | Batch 3 `channels` quota | creating space | space budget | last capability released or revoked, space close |
| queued bytes | Batch 3 `queued_bytes` quota (Batch 4 accounting object) | sending space | space budget; per end `max_queue_bytes` (16384) | on receive or end destruction |
| queued messages | Batch 4 | receiving end | `max_messages` (8) per end | on receive or end destruction |
| in-flight capabilities | Batch 3 `handles` | the receiving end's hold space | `max_handles` (8) per message | delivered, rolled back or destroyed |
| received handles | Batch 3 `handles` | receiving space | space budget | caller releases; failure keeps the message |
| blocked waiters | `accounting.zig` Ledger | calling space and global | 16 per space, 64 total (`Policy`) | every return path of `send`/`recv` |
| operation counters | `accounting.zig` | global | u64 counters incremented with plain `+= 1` (a Debug build traps on overflow; 2^64 events are not reachable in practice) | n/a |

Per-space queued-message and in-flight capability counts are enforced through the Batch 3/4 bounds above;
this layer keeps no shadow counters for them (see REPORT.md, category 2).

Rules: all arithmetic is checked; a release without a matching acquire is counted as `underflows` and
makes `audit()` report a violation; allocation failure on any path leaves state unchanged and returns
`QuotaExceeded` (-6). Tests: RES-01..12, ACC-01..04, STRESS-01..04. RES-05 compares real accounting
against a model over seeds 1..24; RES-09..11 inject allocation failure at every point of the syscall
paths.
