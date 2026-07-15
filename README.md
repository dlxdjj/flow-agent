# flow-agent

Local-first attention surface for coding agents.

The repository is under active v1 development. M3 provides the fail-open Hook
bridge, persistent single-instance Runtime, authenticated localhost control
panel, fixed three-module web UI, safe Claude/Codex installer, first-run
onboarding, and structured diagnostics:

```bash
cargo run -p flow-agent -- serve --open
cargo run -p flow-agent -- install-hooks all
cargo run -p flow-agent -- doctor
cargo run -p flow-agent -- hook --provider claude < fixture.json
```

`serve` defaults to `--approval widget`. The explicit `prompt`, `allow`,
`deny`, and `pass-through` modes remain available for diagnostics and contract
testing.

Runtime data defaults to `~/.flow-agent`. Override it in tests or development
with `FLOW_AGENT_HOME=/path/to/data`.

`install-hooks` backs up existing configuration, preserves user and unknown
fields, writes through a lock and atomic rename, and installs a stable helper
at `~/.flow-agent/bin/flow-agent`. Codex requires a separate user-controlled
trust step: open Codex, run `/hooks`, review the exact Flow Agent commands, and
trust them. Flow Agent never edits Codex trust state or bypasses that review.

```bash
flow-agent install-hooks claude
flow-agent install-hooks codex
flow-agent install-hooks codex --enhanced-codex-activity
flow-agent uninstall-hooks all
flow-agent doctor --json
```

The onboarding UI uses the same installer implementation. It does not show
"connected" until a real provider event arrives after installation. Doctor and
automated tests do not modify the user's real Claude or Codex configuration.

## v1 plan

- [Full v1.1 implementation plan](docs/WIDGET_V1_PLAN.md)
- [Executable milestone acceptance](docs/V1_ACCEPTANCE.md)
- [Open Vibe Island / CodeIsland reference decisions](docs/REFERENCE_REVIEW.md)
- [M0 verification record](docs/M0_VERIFICATION.md)
- [M1 verification record](docs/M1_VERIFICATION.md)
- [M2 verification record](docs/M2_VERIFICATION.md)
- [M3 verification record](docs/M3_VERIFICATION.md)

## Local quality gate

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --offline -- -D warnings
cargo test --workspace --offline
cargo build --workspace --release --offline
./scripts/m0-e2e.sh
```

The common suite covers the provider path, SQLite Runtime, waiter, spool,
single-instance, restart, duplicate-request, authenticated API, UI contract,
safe install/uninstall, tri-state repair, onboarding, trust inspection, factual
task progress, and half-close behavior. The E2E suites verify provider
directives, widget control, pass-through, silent fail-open behavior when the
Runtime is absent, and the post-install real-event verification boundary.

## Privacy

flow-agent is local-first and does not include telemetry or a cloud backend.
Raw prompts, transcripts, source files, survey responses, and contact details
must not be committed to this repository.

## License

MIT
