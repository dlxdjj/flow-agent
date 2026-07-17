# M6 verification record - live sessions and Attention linkage

Status: complete on `agent/v1-full`.

This record was split retrospectively from the previously published combined
`V1_1_FUNCTIONAL_CORRECTIONS.md` record. It maps the committed implementation
and existing test evidence to the M6 boundary; it does not invent a separate
historical candidate or test run.

## Delivered contract

- The main task list contains sessions that are active, have unresolved
  Attention, or were active within the last 30 minutes.
- An old session remains visible while it owns an unresolved item.
- Selecting “在 Agent 任务中查看” selects, pins, highlights, focuses, and
  scrolls the matching session into view.
- Session activity distinguishes thinking, tool execution, waiting, completed,
  failed, and idle only when corresponding facts exist.
- Timers update text without rebuilding the complete task row.
- Claude and Codex use locally served image marks instead of letter monograms.

## Evidence mapping

- Runtime filtering, attention ownership, activity timestamps, and migration:
  `crates/runtime/src/storage.rs` and `crates/runtime/tests/m1_runtime.rs`.
- Authenticated snapshot/filter behavior:
  `crates/server/src/server.rs` and `crates/server/tests/m2_api.rs`.
- Selection, pinning, highlighting, scrolling, icon, and text-only timer
  behavior: `web/app.js`, `web/app.css`, and `web/index.html`.
- Provider icons and licensing: `web/assets/` and
  `THIRD_PARTY_NOTICES.md`.
- Combined exact-tree gates and resource evidence:
  `V1_1_FUNCTIONAL_CORRECTIONS.md`.

Primary implementation commits: `6b7c465` and `120e89d`.

## Acceptance result

The recorded full workspace tests, zero-warning Clippy, release build,
JavaScript syntax check, event-to-WebSocket performance check, five-round
Provider replay, two-minute resource gate, exact-candidate installation, and
local visual acceptance passed. The separate 48-hour final release soak remains
outside M6 and is still pending.
