## What and why

<!-- What does this change, and why? Link the issue: "Fixes #123". -->

## How it was tested

<!-- Tests added, and what you tried by hand (TUI, web UI, which terminal/browser). -->

## Checklist

- [ ] The title is a conventional commit (`feat: …`, `fix: …`) — it becomes the squash commit
- [ ] `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] `npm --prefix web run build` has no Svelte warnings (if the web UI changed)
- [ ] The TUI and the web UI both got the change (or it only makes sense in one)
- [ ] README / CLAUDE.md updated if behaviour or keys changed
