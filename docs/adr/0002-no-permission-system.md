# 2. Do not implement a permission system

Date: 2026-11-08

## Status

Accepted

## Context

The `write`, `edit`, and `bash` tools change the user's machine. Many harnesses put a permission system in front of them, such as approval prompts, allow and deny lists, or path restrictions.

pi-rust is kept minimal ([ADR 1](0001-keep-the-harness-minimal.md)), and the loop works without a permission system.

## Decision

pi-rust does not implement a permission system. Tools run as soon as the model calls them, with the user's full privileges and no confirmation.

Each tool call and its output are printed to the terminal.

## Consequences

- The model can read, change or delete anything the user can, run any command and use the network. This includes following instructions planted in files or command output (prompt injection).
- The user is responsible for where pi-rust runs, for example in a container or VM when working with untrusted code.
- If ADR 1 is revisited, this ADR should be too.
