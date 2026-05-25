# phux

A terminal multiplexer built on
[libghostty-vt](https://github.com/ghostty-org/libghostty-vt-rs).

phux is in the shape of tmux — a long-lived server, attaching clients, sessions
of windows of panes — but the wire protocol carries **structured cell-level
diffs** (not VT byte streams) and **semantic key events** (so modern keyboard
protocols pass cleanly through to inner programs). The outer composition lives
in phux; the terminal emulation is delegated to libghostty.

This book is the authoritative reference for the protocol and architecture:

- **[Wire Protocol](./SPEC.md)** — the framing, message shapes, and semantics
  every phux client and server must implement.
- **[Architecture Decision Records](./ADR/README.md)** — the load-bearing
  decisions, with their rationale and tradeoffs, in chronological order.

The source of truth for these documents lives at
[`SPEC.md`](https://github.com/phall1/phux/blob/main/SPEC.md) and
[`ADR/`](https://github.com/phall1/phux/tree/main/ADR) in the repo; this site
is a rendered view assembled at build time.
