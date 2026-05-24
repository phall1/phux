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
- **A reimplementation of copy mode.** We expose grid state. Modern
  terminals (Ghostty, kitty, wezterm) handle selection well; we do not
  compete.
- **Homegrown crypto.** SSH and Unix socket perms are the model.
- **"Just supporting tmux's behavior here for compatibility."** We are
  not tmux. We will be better in places and different in others, and we
  document the differences.

If your change conflicts with these, open a [Discussion] before a PR.

[Discussion]: https://github.com/phall1/phux/discussions

## Reviewing your own work before opening a PR

- Did the public API change? Rustdoc updated?
- Did wire bytes change? `SPEC.md` updated? Version bumped?
- Could this be tested with `proptest`? Probably should be.
- Is there a simpler shape with fewer abstractions? Prefer it.
- Could a future contributor read this code cold and understand it?

If you can answer "yes" to all of those, ship it.
