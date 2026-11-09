# Contributing to dlux

Thanks for helping! Bug reports, ideas and pull requests are all welcome. By taking part you agree
to the [code of conduct](CODE_OF_CONDUCT.md).

## Getting set up

You need Rust 1.88 or newer, `aria2c` on your `PATH`, and Node.js 22+ for the web UI.

```sh
git clone https://github.com/SaraVix1/DLuxDownloadManager
cd DLuxDownloadManager
npm --prefix web ci && npm --prefix web run build   # build the web UI (embedded into dlux)
cargo run                                           # the TUI
cargo run -- serve                                  # the web UI on http://127.0.0.1:8080
```

Without the web build, `cargo build` still works: dlux embeds a small placeholder page and prints
a warning. For frontend work, run `cargo run -- serve` and, in another terminal,
`npm --prefix web run dev`: Vite serves the UI with hot reload and proxies `/api` to dlux.

## Where things live

| Path | What |
|------|------|
| `crates/dlux-core` | The library: aria2 RPC client, starting aria2, config, app state, yt-dlp/ffmpeg, the `Service` both front ends call |
| `crates/dlux/src/tui` | The terminal UI (ratatui) |
| `crates/dlux/src/web` | The web server (axum): REST API, WebSocket, request guard, embedded assets |
| `web/` | The web UI (Svelte 5, Vite, Tailwind 4) |
| `packaging/` | systemd unit (the `.deb` metadata is in `crates/dlux/Cargo.toml`) |

New features usually start as a method on `dlux_core::service::Service`, so the TUI and the web
UI behave the same.

## Before you open a pull request

```sh
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test                        # integration tests need aria2c; they're skipped without it
npm --prefix web run build        # must finish without Svelte warnings
```

CI runs the same checks on Linux and Windows, plus a compile check with Rust 1.88.

- Add tests for parsing and logic (RPC responses, yt-dlp output, input handling…). The TUI is
  tested by rendering into a test buffer; the web API by running the real router against a
  private aria2.
- Keep the TUI and the web UI in step: a feature in one usually belongs in the other.
- Mouse and keyboard both matter: anything clickable should have a key, and the other way round.

## Commits and pull requests

- [Conventional commits](https://www.conventionalcommits.org/): `feat:`, `fix:`, `docs:`,
  `refactor:`, `chore:`… The changelog is generated from them with
  [git-cliff](https://git-cliff.org/) (`cliff.toml`).
- One topic per pull request, on a `feat/<topic>` or `fix/<topic>` branch. Pull requests are
  squash-merged, so the PR title becomes the commit message — make it a good conventional one.

## Releasing (maintainers)

1. Bump `version` in the root `Cargo.toml`, then `cargo check` to update `Cargo.lock`.
2. `git cliff --tag vX.Y.Z -o CHANGELOG.md`, review it, and commit as `chore(release): vX.Y.Z`.
3. After that is merged: `git tag vX.Y.Z && git push origin vX.Y.Z`. The release workflow builds the
   binaries and `.deb` packages, creates the GitHub release and publishes to crates.io (if the
   `CARGO_REGISTRY_TOKEN` secret is set).
