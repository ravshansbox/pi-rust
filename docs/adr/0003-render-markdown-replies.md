# 3. Render markdown replies

Date: 2026-11-08

## Status

Accepted

## Context

pi-rust is minimal ([ADR 1](0001-keep-the-harness-minimal.md)), but replies are markdown and hard to read as raw text in a terminal.

## Decision

pi-rust renders markdown replies with ANSI styles, using pulldown-cmark without default features. This is an exception to ADR 1.

Replies are rendered one block at a time, at each blank line outside a code fence, so they still stream. Tables are left as raw text. When stdout is not a terminal, replies are printed as raw markdown.

## Consequences

pi-rust depends on pulldown-cmark. Text appears one block at a time instead of as it arrives.
