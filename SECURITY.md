# Security

flow-agent processes local coding-agent events and permission decisions. Do
not include secrets, source files, raw transcripts, or personal survey data in
bug reports.

Until a private reporting channel is published, please contact the maintainer
directly rather than opening a public issue for suspected vulnerabilities.

The v1 security baseline is:

- local-only transports;
- fail-open hooks when the runtime is unavailable;
- user-private runtime directories and sockets;
- bounded hook payloads;
- no telemetry or cloud service.
