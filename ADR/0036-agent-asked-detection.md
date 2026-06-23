---
audience: contributors
stability: stable
last-reviewed: 2026-06-22
---

# 0036 — Agent-asked detection sources

**TL;DR.** `AgentEvent::Asked` remains a server-observed event, but passive
per-agent guessing is not the authority. The shipped `phux-ask` terminal-title
sentinel is the v1 interoperable trigger; the next authority is an opt-in
agent hook contract that writes the same clean `Asked` payload. Screen-scrape
manifests stay a fallback only after a phux-owned empirical corpus exists.

Status: Accepted
Date: 2026-06-22

## Context

ADR-0035 allocated the `asked` event on the existing `EVENT` stream and
shipped a clean-room v1 trigger: an agent sets the terminal title to
`phux-ask[<id>]:<question>?s=opt1|opt2`. The server sees OSC 0 / OSC 2 through
libghostty-vt, parses that sentinel, coalesces repeated markers, and emits
`AgentEvent::Asked`.

That gets projection consumers unstuck, but it does not answer the wider
question in `phux-2sl6.2`: should phux infer "agent is asking" from each
agent's raw screen, rely on an explicit integration contract, or combine both?

The clean-room boundary is strict: external projects can inspire patterns, but
phux must not copy AGPL manifests, hook scripts, regexes, or source. The
detector contract has to be authored here.

The local spike found `claude`, `codex`, and `pi` installed and
version-probable on the development host, but did not claim a complete
blocked-state corpus. Launching those CLIs into real account flows is
environment- and credential-sensitive; reproducible capture is follow-up work,
not an ADR blocker.

## Decision

Use a layered source model:

1. **Authoritative: opt-in agent hook contract.** A first-party or configured
   agent integration should report a pending question directly to phux using
   the same fields as `AgentEvent::Asked`: `id`, `question`, `suggestions`, and
   optional `elapsed_seconds`. The hook owns identity and lifecycle; phux owns
   validation, coalescing, and event emission.
2. **Interoperable v1: `phux-ask` title sentinel.** Keep the shipped title
   sentinel as the lowest-friction path for any process that can emit OSC 0 /
   OSC 2. It is explicit, inspectable, and already covered by server and
   `phux watch --json` tests.
3. **Fallback: phux-owned screen evidence manifests.** A passive detector may
   exist later, but only from a phux-authored corpus captured from real
   `claude`, `codex`, `pi`, and other agents. Its role is advisory fallback
   when hooks and sentinels are absent; it must never outrank an explicit hook.
4. **Not a wire fork.** All sources converge on the existing `Asked` event.
   No new event kind, no tag renumbering, and no protocol minor bump are needed
   for source changes that preserve the payload.

## Consequences

- Projection consumers get one stable event regardless of how the server
  detected it.
- Plugin and config work can add agent integrations without ratatui or
  consumer-specific code entering the core substrate.
- Passive detection becomes an evidence problem, not a copied-manifest problem.
  A detector PR must include the captured local/CI corpus it claims to support.
- If libghostty-vt later exposes OSC 9 / OSC 777 or richer prompt state, that
  can become another source feeding the same event, below hooks and alongside
  explicit sentinels.

## Follow-up Work

- Define the opt-in hook payload and local command/API surface for agent
  integrations to report an ask.
- Build a disposable capture harness that launches available agent CLIs inside
  phux panes, records title, bell, visible grid rows, and stdout/stderr, and
  exits nonzero when empirical coverage is incomplete.
- Add a passive fallback detector only after the corpus proves stable signals
  for at least two agents.
