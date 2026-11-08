# dlux

**Download everything. Beautifully.**

dlux is a modern download manager for [aria2](https://aria2.github.io/), written in Rust, with a
fully mouse-driven terminal UI and a web UI.

> 🚧 Early development. The aria2 RPC client and aria2 auto-spawn are in place; the TUI, web UI and
> yt-dlp integration are on the way. See [CLAUDE.md](CLAUDE.md) for the full plan and milestones.

## Try it

Requires Rust 1.88+ and `aria2c` on your `PATH`.

```sh
cargo run -- status            # connect to aria2 (or start one) and list downloads
cargo run -- status --json     # the same as JSON
cargo run -- config init       # write ~/.config/dlux/config.toml with defaults
```

If no aria2 is listening at the configured RPC URL (default `http://localhost:6800/jsonrpc`), dlux
starts a private `aria2c` with a random RPC secret and stops it again on exit.

## License

[MIT](LICENSE)
