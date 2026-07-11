#!/bin/sh
#
# smoke-agent-wrap.sh (phux-r82.11) — exercise phux-agent-wrap.sh with a
# stub `phux` and a fake agent, asserting the record-write path.
#
# Runs the wrapper against a stub `phux` binary that logs its argv and a
# fake agent that exits successfully, then checks that:
#   1. `phux agent set @<pane> --name <name> --kind <kind>` ran at launch;
#   2. the fake agent ran;
#   3. `phux agent clear @<pane>` ran on exit, pinned to the SAME pane;
#   4. the wrapper forwarded the agent's exit status;
#   5. with NO pane target, the wrapper writes NOTHING (never clobbers a
#      focused pane's record) yet still launches the agent.
#
# Needs no phux server and leaves no state behind.

set -eu

script_dir=$(CDPATH=; cd -- "$(dirname -- "$0")" && pwd)
wrapper=$script_dir/phux-agent-wrap.sh
tmp=${TMPDIR:-/tmp}/phux-agent-wrap-smoke.$$

cleanup() {
  rm -rf "$tmp"
}
trap cleanup EXIT HUP INT TERM

mkdir -p "$tmp"
argv_log=$tmp/phux-argv.log
agent_log=$tmp/agent.log

# Stub `phux`: append its full argv (tab-separated) to the log.
stub_phux=$tmp/phux
cat > "$stub_phux" <<EOF
#!/bin/sh
printf '%s\t' "\$@" >> "$argv_log"
printf '\n' >> "$argv_log"
EOF
chmod +x "$stub_phux"

# Fake agent: record that it ran, echo its own args, exit non-zero to
# prove the wrapper forwards the status.
fake_agent=$tmp/fake-agent
cat > "$fake_agent" <<EOF
#!/bin/sh
printf 'ran %s\n' "\$*" >> "$agent_log"
exit 7
EOF
chmod +x "$fake_agent"

# A pane target is required for the wrapper to write a record. Pin it via
# PHUX_TERMINAL_ID (the automatic path) so the wrapper self-targets `@3`.
status=0
PHUX_AGENT_PHUX_BIN=$stub_phux \
PHUX_TERMINAL_ID=3 \
  sh "$wrapper" --name claude --kind claude -- "$fake_agent" hello || status=$?

if [ "$status" -ne 7 ]; then
  printf 'FAIL: wrapper did not forward agent exit status (got %s, want 7)\n' "$status" >&2
  exit 1
fi

if [ ! -f "$agent_log" ] || ! grep -q 'ran hello' "$agent_log"; then
  printf 'FAIL: fake agent did not run\n' >&2
  exit 1
fi

# The set line must carry the pane target AND the exact flags, in a single
# invocation. `@3` derives from PHUX_TERMINAL_ID.
if ! grep -q 'agent	set	@3	--name	claude	--kind	claude' "$argv_log"; then
  printf 'FAIL: agent set was not invoked as: set @3 --name claude --kind claude\n' >&2
  printf 'argv log:\n' >&2
  cat "$argv_log" >&2
  exit 1
fi

# Clear must target the SAME pane the launch-time set used, never a bare
# (focused-pane) clear that could delete a sibling agent's record.
if ! grep -q 'agent	clear	@3' "$argv_log"; then
  printf 'FAIL: agent clear was not pinned to the launch pane (@3)\n' >&2
  printf 'argv log:\n' >&2
  cat "$argv_log" >&2
  exit 1
fi

# Safety case: with no resolvable pane target, the wrapper must NOT touch
# any record (a focused-pane guess would clobber a sibling), yet must still
# run the agent and forward its status.
notarget_log=$tmp/phux-argv-notarget.log
stub_phux_nt=$tmp/phux-nt
cat > "$stub_phux_nt" <<EOF
#!/bin/sh
printf '%s\t' "\$@" >> "$notarget_log"
printf '\n' >> "$notarget_log"
EOF
chmod +x "$stub_phux_nt"

nt_status=0
env -u PHUX_TERMINAL_ID -u PHUX_AGENT_TARGET \
  PHUX_AGENT_PHUX_BIN="$stub_phux_nt" \
  sh "$wrapper" --name claude --kind claude -- "$fake_agent" hi 2>/dev/null || nt_status=$?

if [ "$nt_status" -ne 7 ]; then
  printf 'FAIL: no-target wrapper did not forward agent exit status (got %s, want 7)\n' "$nt_status" >&2
  exit 1
fi

if [ -f "$notarget_log" ]; then
  printf 'FAIL: no-target wrapper wrote a record (would clobber a focused pane)\n' >&2
  printf 'argv log:\n' >&2
  cat "$notarget_log" >&2
  exit 1
fi

printf 'agent wrap smoke ok\n'
