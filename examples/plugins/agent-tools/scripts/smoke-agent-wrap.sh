#!/bin/sh
#
# smoke-agent-wrap.sh (phux-r82.11) — exercise phux-agent-wrap.sh with a
# stub `phux` and a fake agent, asserting the record-write path.
#
# Runs the wrapper against a stub `phux` binary that logs its argv and a
# fake agent that exits successfully, then checks that:
#   1. `phux agent set --name <name> --kind <kind>` ran at launch;
#   2. the fake agent ran;
#   3. `phux agent clear` ran on exit;
#   4. the wrapper forwarded the agent's exit status.
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

status=0
PHUX_AGENT_PHUX_BIN=$stub_phux \
  sh "$wrapper" --name claude --kind claude -- "$fake_agent" hello || status=$?

if [ "$status" -ne 7 ]; then
  printf 'FAIL: wrapper did not forward agent exit status (got %s, want 7)\n' "$status" >&2
  exit 1
fi

if [ ! -f "$agent_log" ] || ! grep -q 'ran hello' "$agent_log"; then
  printf 'FAIL: fake agent did not run\n' >&2
  exit 1
fi

# The set line must carry the exact flags, in a single invocation.
if ! grep -q 'agent	set	--name	claude	--kind	claude' "$argv_log"; then
  printf 'FAIL: agent set was not invoked with --name claude --kind claude\n' >&2
  printf 'argv log:\n' >&2
  cat "$argv_log" >&2
  exit 1
fi

if ! grep -q 'agent	clear' "$argv_log"; then
  printf 'FAIL: agent clear was not invoked on exit\n' >&2
  printf 'argv log:\n' >&2
  cat "$argv_log" >&2
  exit 1
fi

printf 'agent wrap smoke ok\n'
