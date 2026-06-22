#!/bin/sh
set -eu

root=${PHUX_PLUGIN_ROOT:-$(CDPATH=; cd -- "$(dirname -- "$0")/.." && pwd)}

section_key() {
  section=$1
  key=$2
  file=$3
  awk -F '=' -v section="[$section]" -v key="$key" '
    $0 == section { in_section = 1; next }
    /^\[/ { in_section = 0 }
    in_section && $1 ~ "^[[:space:]]*" key "[[:space:]]*$" {
      value = $2
      sub(/^[[:space:]]*"/, "", value)
      sub(/"[[:space:]]*$/, "", value)
      print value
      exit
    }
  ' "$file"
}

top_key() {
  key=$1
  file=$2
  awk -F '=' -v key="$key" '
    $1 ~ "^[[:space:]]*" key "[[:space:]]*$" {
      value = $2
      sub(/^[[:space:]]*"/, "", value)
      sub(/"[[:space:]]*$/, "", value)
      print value
      exit
    }
  ' "$file"
}

command_exists() {
  command_name=$1
  search_path=${PHUX_AGENT_TOOLS_PATH:-$PATH}
  old_ifs=$IFS
  IFS=:
  for dir in $search_path; do
    if [ -x "$dir/$command_name" ]; then
      IFS=$old_ifs
      return 0
    fi
  done
  IFS=$old_ifs
  return 1
}

if [ "${PHUX_AGENT_TOOLS_DETECT:-0}" != "1" ]; then
  printf 'agent detection disabled\n'
  printf 'set PHUX_AGENT_TOOLS_DETECT=1 to probe opt-in integration templates\n'
  exit 0
fi

printf 'id\tdisplay_name\tcommand\tstate\n'
for file in "$root"/integrations/*.toml; do
  [ -e "$file" ] || continue
  id=$(top_key id "$file")
  name=$(top_key display_name "$file")
  command_name=$(section_key detect command "$file")
  state=missing
  if command_exists "$command_name"; then
    state=available
  fi
  printf '%s\t%s\t%s\t%s\n' "$id" "$name" "$command_name" "$state"
done
