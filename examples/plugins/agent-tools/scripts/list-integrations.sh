#!/bin/sh
set -eu

root=${PHUX_PLUGIN_ROOT:-$(CDPATH=; cd -- "$(dirname -- "$0")/.." && pwd)}

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

printf 'id\tdisplay_name\tkind\tdetect_mode\tdetect_command\n'
for file in "$root"/integrations/*.toml; do
  [ -e "$file" ] || continue
  printf '%s\t%s\t%s\t%s\t%s\n' \
    "$(top_key id "$file")" \
    "$(top_key display_name "$file")" \
    "$(top_key kind "$file")" \
    "$(section_key detect mode "$file")" \
    "$(section_key detect command "$file")"
done
