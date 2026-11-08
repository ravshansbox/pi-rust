# pi-rust

A minimal coding agent for the terminal, written in Rust. It is a proof of concept.

## Requirements

- macOS (credentials are stored in the keychain and sign-in opens the browser with `open`)
- Rust (edition 2024)
- A Claude account

## Run

```sh
cargo run
```

On first run, pi-rust opens a browser to sign in to Claude. Paste the code it shows back into the terminal. The credentials are saved in the macOS keychain under `pi-rust` and refreshed when they expire.

Type a prompt after `>` and press Enter. Press Ctrl+D to quit.

The model is `claude-opus-5-5` by default. To use another model, pass `--model`:

```sh
cargo run -- --model claude-sonnet-5-5
```

## Tools

The model can use these tools:

- `read`: read a file
- `write`: write a file
- `edit`: edit a file
- `bash`: run a shell command
- `web_fetch`: fetch a web page
- `web_search`: search the web

Tools run without asking for confirmation. The model can do anything you can.

## Test

```sh
cargo test
```

## Decisions

See [docs/adr](docs/adr).
