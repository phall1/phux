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

require() {
  label=$1
  value=$2
  file=$3
  if [ -z "$value" ]; then
    printf 'missing %s in %s\n' "$label" "$file" >&2
    exit 1
  fi
}

count=0
for file in "$root"/integrations/*.toml; do
  [ -e "$file" ] || continue
  require schema_version "$(top_key schema_version "$file")" "$file"
  require id "$(top_key id "$file")" "$file"
  require display_name "$(top_key display_name "$file")" "$file"
  require kind "$(top_key kind "$file")" "$file"
  require detect.mode "$(section_key detect mode "$file")" "$file"
  require detect.command "$(section_key detect command "$file")" "$file"
  require launch.command "$(section_key launch command "$file")" "$file"
  count=$((count + 1))
done

if [ "$count" -eq 0 ]; then
  printf 'no integration templates found under %s/integrations\n' "$root" >&2
  exit 1
fi

printf 'validated %s integration templates\n' "$count"
