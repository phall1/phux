# Contributing to phux

phux is an experiment in building the terminal multiplexer that would
exist if libghostty had been available in 2007. We are picky about
contributions — not to be precious, but because every multiplexer that
came before ours was strangled by accumulated cruft, and we are deeply
allergic.

The yardstick is the [smol manifesto](https://smol.tauri.app/): solve a
well-defined problem; behave the way users expect; be maintainable by
one person; compose with other tools; be finishable. If a proposal moves
phux away from any of those, it's the wrong proposal.

## Bar for any change

A PR must pass:

```sh
just ci
```

That target runs (and you must, locally, before pushing):

1. `cargo fmt --all --check` — formatting is mechanical, not a style debate.
2. `cargo clippy --all-targets --all-features -- -D warnings` — clippy is
   pedantic on purpose. If a lint is wrong for our case, allow it with a
   comment explaining why.
3. `cargo nextest run --workspace --all-features` — all tests pass.
4. `cargo deny check` — licenses, advisories, unauthorized sources.

## Additional expectations

- **Test what you change.** Protocol changes need `proptest` roundtrip
  cases and `insta` snapshots. State-machine changes need explicit
  transition tests. Bug fixes get a regression test, named after the
  issue.
- **Update [`SPEC.md`](./SPEC.md) when the wire changes.** The spec is
  normative — code conforms to it, not the other way around. Bump the
  protocol version per the rules in `SPEC.md` §6.
- **Write an ADR for any decision that closes off a design space.** See
  [`ADR/README.md`](./ADR/README.md). You do not need an ADR for a bug
  fix; you do for "should this be in `core` or `server`?"
- **Public APIs are documented.** Workspace lints warn on missing docs
  for library crates. The binary crate is exempt.
- **`unsafe` requires justification.** Every `unsafe` block carries a
  `// SAFETY: …` comment naming the invariant it relies on. We prefer
  zero `unsafe` and lint for it (`#![forbid(unsafe_code)]` is the default
  for new modules unless explicitly opted out).
- **No new dependencies without a paragraph of justification in the PR.**
  Every dep is a long-term maintenance cost. Inline what you can.

## Things we will not accept

Asking saves us both time:

- **An embedded scripting language.** Commands are typed IPC messages.
  If you want logic, write a script and shell out.
- **A plugin system on day one.** Hooks are typed events. We may design a
  proper plugin contract later, after we know what is actually pluggable.
- **A homegrown selection engine.** When we add copy-mode (see issue
  `phux-abi`), we bridge to libghostty-vt's selection APIs (Ghostty PR
  \#12794) rather than reimplement word/line/output boundaries.
- **Homegrown crypto.** SSH and Unix socket perms are the model.
- **"Just supporting tmux's behavior here for compatibility."** We are
  not tmux. We will be better in places and different in others, and we
  document the differences.

If your change conflicts with these, open a [Discussion] before a PR.

[Discussion]: https://github.com/phall1/phux/discussions

## Git workflow

- **Linear history is the default.** Prefer fast-forward merges or
  rebases. Do NOT create merge commits with `--no-ff` on `main` — the
  log must stay linear and bisect-friendly. For a multi-branch
  integration, the canonical sequence is:
  ```bash
  for branch in <ordered list>; do
      git rebase main "$branch"      # replay onto current main
      git checkout main
      git merge --ff-only "$branch"
  done
  ```
- **One commit per task.** Squash WIP commits before merge. The
  commit message tells the story of the change, not the keystrokes
  that produced it.
- **Never `--no-verify`.** Pre-commit hooks are load-bearing. If a
  hook fails, fix the root cause.

## Multi-agent fan-out

When fanning out parallel agent work (e.g. four agents in wave 1 of
the protocol epic):

1. **Pre-create explicit worktrees** before launching agents:
   ```bash
   git worktree add /tmp/phux-<wave>-<task> -b <branch-name> main
   ```
   Do NOT rely on the Claude Code Agent tool's `isolation: worktree`
   flag for parallel launches — in wave 1 only 2 of 4 agents got real
   worktrees (race condition); the other 2 shared the main checkout.
   Self-managed worktrees are race-free.
2. **Pre-scaffold shared files** (e.g. `mod.rs`, `lib.rs`) so each
   agent owns disjoint files. This is how wave 1 avoided merge
   conflicts on `crates/phux-protocol/src/input/mod.rs`.
3. **Each agent's prompt MUST start with** a `cd /tmp/phux-...; pwd`
   to verify they're in their worktree, and an instruction to produce
   **one squashed commit** on their branch.
4. **Integration uses rebase + ff-only merge** per the Git workflow
   section above.
5. **Clean up after merge**:
   ```bash
   git worktree remove /tmp/phux-<wave>-<task>
   git branch -d <branch-name>
   ```

## Observability: tokio-console

`phux-server` has an opt-in `tokio-console` cargo feature that attaches
the [tokio-console](https://github.com/tokio-rs/console) debugger to a
running server — handy for inspecting broadcast lag, task stalls, and
poll counts in the actor system. Requires Tokio built with
`--cfg tokio_unstable`:

```sh
RUSTFLAGS='--cfg tokio_unstable' cargo run --features phux-server/tokio-console -- server
# in another shell:
cargo install --locked tokio-console
tokio-console   # connects to 127.0.0.1:6669 by default
```

## Heap profiling: dhat

The `phux` binary has an opt-in `dhat-heap` cargo feature that swaps in
the [dhat](https://docs.rs/dhat) allocator and installs a heap profiler
for the lifetime of `main()`. On clean shutdown, a `dhat-heap.json`
report is written to the current working directory:

```sh
cargo run --features dhat-heap -- server
# Ctrl-C the server to flush; then open dhat-heap.json at:
#   https://nnethercote.github.io/dh_view/dh_view.html
```

The instrumented allocator is significantly slower than the system
allocator — use for profiling only, never for production builds.

## Reviewing your own work before opening a PR

- Did the public API change? Rustdoc updated?
- Did wire bytes change? `SPEC.md` updated? Version bumped?
- Could this be tested with `proptest`? Probably should be.
- Is there a simpler shape with fewer abstractions? Prefer it.
- Could a future contributor read this code cold and understand it?

If you can answer "yes" to all of those, ship it.
