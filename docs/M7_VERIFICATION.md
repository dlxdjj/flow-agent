# M7 verification record - dynamic quota and truthful timing

Status: complete on `agent/v1-full`.

This record was split retrospectively from the previously published combined
`V1_1_FUNCTIONAL_CORRECTIONS.md` record. It maps committed behavior and existing
evidence to M7 without claiming a separate historical frozen candidate.

## Delivered contract

- Quota renders every structurally valid Provider window instead of forcing a
  fixed Claude/Codex slot model.
- Codex primary, secondary, monthly, weekly, daily, or future named windows are
  displayed only when the bounded adapter validates their numeric structure.
- A successful quota sample remains visible after 30 minutes with its real
  capture time and “last valid sample” semantics.
- Missing or incompatible data remains unavailable; age never fabricates a
  percentage or a refresh.
- A newly created/changed Claude quota cache invalidates an earlier unavailable
  snapshot immediately.
- Existing Claude status-line configuration is never silently overwritten;
  explicit wrapper mode preserves visible output and restores the complete
  original object on uninstall.
- Agent time uses total current-turn time as the primary value and current
  phase time as secondary context.

## Evidence mapping

- Dynamic and stale-value quota parsing: `crates/quota/src/lib.rs` and its
  fixture-backed tests.
- Status-line wrapper/restore: `crates/installer/src/lib.rs` and
  `crates/installer/tests/m4_statusline.rs`.
- Immediate cache refresh and quota snapshot API: `crates/server/src/server.rs`.
- Turn and phase timestamps: `crates/runtime/src/storage.rs` and
  `crates/runtime/tests/m1_runtime.rs`.
- UI windows, capture time, last-valid state, and timer rendering:
  `web/app.js`.
- Combined exact-tree gate record: `V1_1_FUNCTIONAL_CORRECTIONS.md`.

Primary implementation commits: `6b7c465` and `120e89d`.

## Acceptance result

The recorded quota, Runtime, installer, server, workspace, release, and
two-minute resource gates passed. Claude Desktop still cannot create Claude
Code terminal `statusLine` samples; this optional quota-source limitation does
not block Hook installation or Agent event/control support.
