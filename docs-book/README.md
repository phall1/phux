# phux

A **libghostty-backed terminal control plane** built on
[libghostty-vt](https://github.com/Uzaaft/libghostty-rs). A long-lived
server hosts terminals — spawned, observed, controlled, persisted,
addressable across hosts — and a tmux-shaped TUI rides on top as one
consumer among several.

The wire is asymmetric (ADR-0013): server→client *terminal content* is
**VT bytes** forwarded from the PTY (after per-client capability
rewriting), while client→server *input* is **structured key, mouse,
focus, and paste events** built from libghostty's own atoms. Both ends
run `libghostty_vt::Terminal`; the server's is canonical, the client's
is a local mirror used for rendering. Modern terminal protocols pass
end-to-end because the multiplexer never re-parses VT in the middle.

The protocol is layered (ADR-0015): an L1 substrate of Terminals that
every consumer speaks, an optional L2 Collection lifecycle bundle, and
an L3 metadata store on top. Sessions, windows, panes, splits — the
tmux vocabulary users expect — live in the reference TUI as conventions
over L3 metadata, not as wire concepts. Federation is in the addressing
scheme from day one (ADR-0007); identity is portable across hosts.

This book is the authoritative reference for the project:

- **[Vision](./VISION.md)** — the long arc. Read this first.
- **[Wire Protocol](./SPEC.md)** — the framing, message shapes, and
  semantics every phux server and client must implement.
- **[Architecture Decision Records](./ADR/README.md)** — the
  load-bearing decisions, with their rationale and tradeoffs, in
  chronological order.

The source of truth lives in the
[repo](https://github.com/phall1/phux); this site is a rendered view
assembled at build time.
