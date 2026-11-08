# CLAUDE.md — dlux project plan

**dlux** — *"Download everything. Beautifully."*
A modern download manager for [aria2](https://aria2.github.io/), written in Rust, with a fully
mouse-driven terminal UI and a web UI. Open source, MIT licensed.

This file is the persistent plan for the project. Read it at the start of every session and keep
the **Milestone status** section up to date.

## Repository layout

```
Cargo.toml              # workspace root (shared versions, lints, release profile)
crates/
  dlux-core/            # library: aria2 JSON-RPC client, aria2 process mgmt, config,
                        # app state, yt-dlp manager, binary manager
  dlux/                 # the `dlux` binary: CLI, TUI front end, web server front end
web/                    # (milestone 7) Svelte/React + Vite + Tailwind frontend, embedded via rust-embed
```

- Binary/command: `dlux`. Binary crate: `dlux` (name was free on crates.io). Library crate: `dlux-core`.
- `dlux` (no args) → TUI. `dlux serve --port 8080 --bind 127.0.0.1` → web server.

## Architecture

- Stack: ratatui + crossterm (mouse capture on), tokio, reqwest (rustls), serde/serde_json,
  anyhow (binary) / thiserror (library errors), axum for the web server, clap for the CLI.
- aria2 is driven over JSON-RPC (default `http://localhost:6800/jsonrpc`, optional rpc-secret,
  sent as `"token:<secret>"` first param). Numbers in aria2 responses are strings — parse them.
- Startup: try to connect to a running aria2. If none answers and the RPC URL is local, spawn
  `aria2c --enable-rpc` as a child process with a random secret (passed via a 0600 conf file, not
  argv), `--stop-with-process=<our pid>` as a safety net, and shut it down on exit (SIGTERM on
  Unix — aria2 saves its session and exits in ~1 s, while the `aria2.shutdown` RPC lingers ~4 s;
  RPC shutdown on Windows; kill as a last resort).
- Module split in `dlux-core`: `rpc` (client + types), `aria2` (connect/spawn), `config`, `paths`,
  `fmt`, `state` (app state, filters, speed history, poller), later `ytdlp`, `binaries`.
  In `dlux`: `cli`, `status`, `tui` (`app` state/input, `ui` rendering, `theme`, `terminal`), later
  `web`.
- MSRV: Rust 1.88 (required by ratatui 0.30). Edition 2024.
- Both front ends consume the same core: a polling loop builds a `Snapshot` (global stat + active +
  waiting + stopped via `system.multicall`) once per second.

## Config

`~/.config/dlux/config.toml` (XDG on Unix, platform config dir on Windows). Missing file → defaults.
Keys: `[aria2] rpc_url, rpc_secret, auto_spawn, aria2c_path, extra_args`, `[downloads] dir,
max_concurrent`, `[ui] theme`, `[web] token`. Data dir: `~/.local/share/dlux/` (bin/, aria2 session,
logs).

## TUI design (Tokyo Night palette)

- Rounded borders everywhere. Header: small dlux logo + global ↓/↑ speed pills.
- Speed graph panel (last 60 s, braille/half-block line chart).
- Pill-style filter tabs with counts: Active, Waiting, Done, Errors.
- Download cards: name, thin smooth progress bar (green fast, amber slow, gray paused, red error),
  size, speed, ETA, percent. yt-dlp jobs get a small "video" tag.
- Details pane for the selection: connections, seeds, directory, files, GID.
- Bottom toolbar of clickable buttons (+ Add, Pause, Resume, Remove) and a key-hint line.
- Responsive for small and large terminals.

### Mouse (must be complete)
Click tabs/cards/toolbar/dialog buttons; wheel scroll; draggable scrollbar; right-click context
menu on a card (pause, resume, remove, open folder, copy URL); hover highlight; double-click opens
details. Keyboard too: j/k, a, p, r, d, tab, ?, q.

## Features

- Add dialog: URL(s), magnet links, .torrent/.metalink path; choose directory.
- Pause/resume/remove, pause all/resume all, purge finished. Global and per-download speed limits.

## yt-dlp integration

- Non-direct-file URL → Add dialog switches to "Video" mode; `yt-dlp -J <url>` → clickable
  format list (1080p, 720p, audio only…). Download with `yt-dlp --downloader aria2c -f <fmt>`,
  parse progress output into a card with a video tag.
- Binaries: use yt-dlp/ffmpeg from PATH; otherwise download standalone yt-dlp (GitHub releases) and
  a static ffmpeg into `~/.local/share/dlux/bin/` on first need, with a progress card.
- "Update yt-dlp" button (`yt-dlp -U`). Friendly error messages on failure.

## Web UI

- axum: REST API + WebSocket pushing live updates every second.
- Frontend: lightweight Svelte or React + Vite + Tailwind, built to static files and embedded with
  rust-embed (single self-contained executable). Same look/features as the TUI, responsive,
  drag-and-drop .torrent/URLs, PWA manifest.
- Security: bind 127.0.0.1 by default; require a token/password when bound elsewhere; never expose
  the aria2 RPC secret to the browser.

## Packaging & workflow

- Release profile: lto, strip, codegen-units=1, panic=abort (terminal restored via panic hook).
- cargo-deb metadata, systemd user unit for `dlux serve`.
- GitHub Actions: CI (fmt, clippy, test) on PRs; release workflow for Linux x86_64/aarch64 + Windows.
- Publish to crates.io. README (screenshots, install, keybinding + mouse table), CONTRIBUTING.md,
  CODE_OF_CONDUCT.md, issue/PR templates.
- Conventional commits (`feat:`, `fix:`, `docs:`, `chore:`). One branch + one PR per milestone,
  named `feat/<topic>` (e.g. `feat/tui-layout`), squash merge, linear history. Changelog/version bumps via release-please or git-cliff; tags `v0.1.0`.

## Quality bar

- Clean module boundaries (see above). Restore terminal on panic and Ctrl-C.
- Unit tests for RPC parsing and yt-dlp progress parsing.
- Before committing: `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.

## Milestones (one PR each; stop after each for review)

1. [x] CLAUDE.md, workspace skeleton, aria2 RPC client, aria2 auto-spawn
2. [x] TUI layout, theme, live list, speed graph
3. [x] Complete mouse support and keyboard shortcuts
4. [ ] Add dialog (URLs, magnets, torrents) and download controls
5. [ ] yt-dlp and ffmpeg integration with the format picker
6. [ ] Web backend (REST + WebSocket)
7. [ ] Web frontend, embedded in the binary
8. [ ] Packaging, CI/CD, README, release v0.1.0

## Milestone status / notes

- **M1**: workspace with `dlux-core` + `dlux`. `dlux status` prints aria2 version, global stats and
  downloads (smoke test for the RPC client). `dlux` (TUI) and `dlux serve` are placeholders until
  M2/M6. Integration tests in `crates/dlux-core/tests/` spawn a real `aria2c` and are skipped when it
  is not installed. Merged as PR #1.
- **M2** (branch `feat/tui-layout`): `dlux` opens the TUI. Header (logo, aria2 status, ↓/↑ pills),
  60 s braille speed graph (nice 1/1.5/2/3/5/7.5 scale in binary units), pill filter tabs with
  counts, 3-row download cards (half-cell smooth progress bar coloured by state), details pane,
  toolbar buttons (render only; enabled/disabled by selection), key-hint line. Responsive: details
  beside the list at ≥110 cols, toggled with Enter below/instead of the list on narrow terminals;
  graph dropped under 24 rows; compact header under 16 rows. Truecolor detection with xterm-256
  fallback; `ui.theme` = `tokyo-night` | `tokyo-night-storm`. Keys working in M2: q/Esc/Ctrl-C,
  tab/shift-tab/1-4, j/k/arrows/PgUp/PgDn/g/G, Enter. Mouse capture is on and `App::hits` records
  tab/card/button/scrollbar rects for M3. TUI logs go to `~/.local/share/dlux/dlux.log`.
  Screenshots in `docs/screenshots/`.
- **M3** (branch `feat/m3-mouse-keyboard`): every input resolves to a `tui::app::Command`
  (keys, toolbar, context menu, clickable hints), run against the selection; side effects are
  queued as `Action`s and performed by `tui::actions::Executor` on background tasks (aria2 calls,
  `xdg-open`/`open`/`explorer`, clipboard via OSC 52 + wl-copy/xclip/xsel/pbcopy/clip), with
  results shown as a toolbar toast. `tui::input` routes events dialog → menu → main screen.
  Mouse: click tabs/cards/buttons/hints, double-click (450 ms) opens details, right-click context
  menu (Pause, Resume, Remove, Open folder, Copy URL), wheel scrolls the list (or switches tabs
  over the tab row), draggable scrollbar, hover highlights (`Theme::bg_hover`). Wheel/drag scroll
  the view without moving the selection (`App::follow`). Keys added: a, p/r/space, d/Del, o, c,
  m/Menu/Shift-F10, ?/F1; Esc closes popups first. Remove asks for confirmation unless the
  download already stopped (then `removeDownloadResult`). Core: `spawn_poller` now returns a
  `PollerHandle` with `refresh()`; out-of-schedule polls arrive as `PollEvent::Refresh` and do not
  add speed samples. `a` shows a "coming next milestone" toast until M4.
