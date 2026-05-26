# phux

A terminal multiplexer built on
[libghostty-vt](https://github.com/Uzaaft/libghostty-rs).

phux is in the shape of tmux — a long-lived server, attaching clients, sessions
of windows of panes — but the wire is asymmetric (ADR-0013): server→client
*pane content* is **VT bytes** forwarded from the PTY (after per-client
capability rewriting), while client→server *input* is **structured key, mouse,
focus, and paste events** built from libghostty's own atoms. Both ends run
`libghostty_vt::Terminal`; the server's is canonical, the client's is a local
mirror used for rendering. Modern keyboard protocols pass cleanly through to
inner programs because the multiplexer never re-parses VT to do them. The
outer composition lives in phux; the terminal emulation is delegated to
libghostty.

This book is the authoritative reference for the protocol and architecture:

- **[Wire Protocol](./SPEC.md)** — the framing, message shapes, and semantics
  every phux client and server must implement.
- **[Architecture Decision Records](./ADR/README.md)** — the load-bearing
  decisions, with their rationale and tradeoffs, in chronological order.

The source of truth for these documents lives at
[`SPEC.md`](https://github.com/phall1/phux/blob/main/SPEC.md) and
[`ADR/`](https://github.com/phall1/phux/tree/main/ADR) in the repo; this site
is a rendered view assembled at build time.
