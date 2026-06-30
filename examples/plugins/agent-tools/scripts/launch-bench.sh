. "$(CDPATH=; cd -- "$(dirname -- "$0")" && pwd)/bench-common.sh"

mkdir -p "$bench_state_dir"
tmp=$bench_state.tmp
printf 'role\tsession\tstatus\ttarget\n' > "$tmp"

for role in $bench_roles; do
  session=$(role_session "$role")
  if phux_cmd new --json -s "$session" --cwd "$bench_workspace" >/dev/null; then
    status=running
  else
    status=exists-or-error
  fi
  printf '%s\t%s\t%s\t%s\n' "$role" "$session" "$status" "$session" >> "$tmp"
done

mv -f "$tmp" "$bench_state"
cat "$bench_state"
