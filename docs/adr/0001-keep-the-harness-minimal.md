# 1. Keep the harness minimal

Date: 2026-11-08

## Status

Accepted

## Context

pi-rust is a proof of concept. It sends a conversation to the Messages API, streams the reply, runs the tools the model asks for, and loops until the model answers without a tool call.

A proof of concept only needs to show that this loop works. Anything beyond that is out of scope.

## Decision

pi-rust stays minimal. Concretely:

- **Four tools:** `read`, `write`, `edit`, and `bash`.
- **One model, no flags.** The model, token limit, and system prompt are constants in `src/main.rs`.
- **No configuration.** No config files and no environment variables. Credentials come from the existing Claude Code keychain entry.
- **Few dependencies.** Only `reqwest`, `serde_json`, and `tokio`.
- **Plain terminal I/O.** Line-based input and streamed output. No TUI.
- **Memory-only sessions.** The conversation isn't saved to disk.

A feature is added only if the loop can't work without it.

## Consequences

- Features users may expect from other harnesses aren't there, such as resuming sessions, compacting long conversations, choosing models, multi-line input and permissions ([ADR 2](0002-no-permission-system.md)).
- Correctness fixes are still in scope.
- If pi-rust grows beyond a proof of concept, this ADR should be revisited.
