# M4 verification record

Status: passed on 2026-07-16. The final automated gate passed on 2026-07-15,
and the user explicitly accepted the exact isolated 1600x600 main quota view
and settings view on 2026-07-16. M4 is eligible for its milestone commit and
GitHub push after the final staged-tree checks below pass.

M4 delivers honest Claude/Codex quota adapters, optional Claude status-line
installation, notification and mute rules, retention, complete local JSON
export, and destructive clear. Gemini remains an optional P1 provider and is
intentionally not shipped in v1; it did not delay either P0 provider.

## Current contract and reference review

- Current [Claude Code status-line documentation](https://code.claude.com/docs/en/statusline)
  confirms `rate_limits.five_hour` and `rate_limits.seven_day`, their
  `used_percentage` and `resets_at` fields, optional presence, and stdin/stdout
  execution model. The live development CLI remains Claude Code `2.1.210`.
- The current local Codex CLI is `0.144.4`. A keys-only inspection of sanitized
  rollout records confirmed `session_meta.payload.cli_version` and
  `event_msg.payload.type=token_count` with `rate_limits.primary/secondary`.
  The adapter only parses those records, scans a bounded tail, and never
  projects response items or conversation text.
- The official [Codex app-server rate-limit contract](https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md#7-rate-limits-chatgpt)
  was reviewed as current upstream evidence. M4 still follows the v1.1 plan's
  experimental read-only rollout adapter rather than starting an account API.
- Open Vibe Island revision `6e5e7a6a5b5097ee627a7d4dea6226c128747a71`
  and CodeIsland revision `3e2aec7fa87c56b0f5129d7ba11d0dc3699dd500`
  remain architecture and failure-mode references only. No GPL source was
  copied. The accepted lesson is preserved: an existing custom status line is
  never overwritten, and a missing bridge/cache is a visible unavailable
  state.

## Quota truth boundary

- Claude quota capture persists only schema version, provider/source,
  capture time, window, used percentage, and reset time. Tests prove that
  session ID, cwd, transcript path, prompts, and unrelated status-line fields
  never reach the cache.
- Installing the optional Claude bridge uses the existing lock, backup,
  semantic JSON merge, same-directory temporary file, fsync, atomic rename,
  symlink refusal, stable binary, and mode preservation. A custom `statusLine`
  returns a conflict before any binary, backup, or config mutation.
- Codex accepts only the fixture-gated `0.144.4` rollout family. It reads at
  most 128 KiB for session metadata and a 2 MiB file tail for token-count
  records. Unknown versions, changed schema, missing/null limits, missing
  files, parse errors, and data older than 30 minutes render without a
  percentage.
- Available rows show remaining percentage derived from the provider's used
  percentage. Missing and stale rows have neither `usedPct` nor
  `remainingPct`; the UI cannot construct a zero or estimated bar from them.

## Settings and local data

- Notification modes are `banner`, `list`, or `ignore` for approval, question,
  error, and completion. Sound and per-Provider mute rules apply only to the
  page notification; approval cards remain visible and controllable.
- The Codex tool-activity toggle semantically reinstalls the managed Hook only
  when Codex is already connected and then explicitly tells the user to review
  `/hooks` trust again.
- Retention accepts only 30, 90, or 365 days and prunes only events older than
  the selected boundary. Sanitized attention and command history follows the
  v1.1 persistence contract.
- Export enumerates every local application table into JSON and adds only a
  schema version and export timestamp. It performs no network operation.
- Destructive clear requires the exact string `DELETE`, closes SQLite, removes
  `data.sqlite`, WAL, SHM, cache, and spool, then creates a fresh private
  database so the running Runtime remains usable. Hook configuration, stable
  integration intent, backups, and binaries are preserved.

## Automated results

| Gate | Result |
| --- | --- |
| `cargo clippy --workspace --all-targets --offline -- -D warnings` | PASS, zero warnings |
| `cargo test --workspace --offline` | PASS, 91 tests |
| `cargo test -p flow-agent-quota --offline` | PASS, 5 schema/freshness/privacy/bounded-scan cases |
| `cargo test -p flow-agent-installer --offline` | PASS, including 3 status-line safety cases |
| `cargo test -p flow-agent-runtime --offline` | PASS, including 3 data transaction cases |
| `cargo test -p flow-agent-server --offline` | PASS, authenticated M4 API plus M2 regressions |
| `cargo test -p flow-agent --test m4_cli --offline` | PASS, 3 single-binary cases |
| `cargo build --workspace --release --offline` | PASS |
| `node --check web/app.js` | PASS |
| `./scripts/m0-e2e.sh` | PASS, Claude allow, Codex deny, pass-through, missing-runtime fail-open |
| `git diff --check` | PASS |

The browser-control plugin failed twice before page initialization with the external
error `Cannot redefine property: process`, the same failure recorded for M2 and
M3. Per the browser skill, no standalone automation was substituted. An
isolated Runtime under `/tmp/fa-m4-visual-*` opened the exact page in the
default browser for human review. The user accepted both the quota view and the
settings overlay at 1600x600 on 2026-07-16.

## Remaining gate

The short format/diff and sensitive-token checks are rerun against the exact
staged tree immediately before the M4 commit. M5 performance,
security-hardening, 48-hour soak, packaging, and release evidence remain out
of scope here.
