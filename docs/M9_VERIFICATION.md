# M9 verification record - Provider title consistency

Status: complete on `agent/v1-full`.

M9 was implemented in the v1.1 functional-correction candidate and previously
documented inside `V1_1_FUNCTIONAL_CORRECTIONS.md` and
`V1_ACCEPTANCE.md`. This file provides the missing milestone-specific index to
that evidence.

## Delivered contract

- The main title is the Provider conversation title when a verified local
  source exists.
- Claude accepts official `session_title` and bounded local custom/AI title
  records with freshness/priority rules.
- Codex accepts the latest bounded local `thread_name` for the matching
  session/thread.
- The second line is the normalized, maximum-64-character current question
  without a synthetic prefix.
- The third line contains only the current model when supplied.
- Project, Provider, title provenance, usage, usernames, paths, and transcript
  text are not mixed into the three-line title stack.
- Recent local title changes refresh without Runtime restart.
- SQLite schema migration preserves existing sessions and title provenance.

## Evidence mapping

- Bounded title resolution and priority: `crates/runtime/src/title.rs`.
- Migration, privacy, refresh, and snapshot tests:
  `crates/runtime/src/storage.rs` and
  `crates/runtime/tests/m1_runtime.rs`.
- Official Hook field parsing and Provider contracts:
  `crates/core/src/lib.rs`, `crates/core/tests/m0_contract.rs`, and
  `crates/providers/tests/provider_contract.rs`.
- Browser title-stack rendering: `web/app.js`.
- Exact historical gate and local acceptance evidence:
  `V1_1_FUNCTIONAL_CORRECTIONS.md` and `V1_ACCEPTANCE.md`.

Primary implementation commit: `120e89d`.

## Acceptance result

The historical record reports format, zero-warning Clippy, the workspace
suite, release build, five-round Claude/Codex control replay, two-minute
resource gate, exact local installation, live title rendering, browser-console
checks, Codex re-trust, user acceptance, commit, and push as passed. The
continuous 48-hour final release soak remains a separate incomplete M5 gate.
