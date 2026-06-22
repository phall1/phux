#!/bin/sh
set -eu

printf 'phux plugin demo\n'
printf 'core=stable terminal/session host\n'
printf 'plugin=agentic workflow package\n'
printf 'plugin_id=%s\n' "${PHUX_PLUGIN_ID:-}"
printf 'action_id=%s\n' "${PHUX_PLUGIN_ACTION_ID:-}"
printf 'root=%s\n' "${PHUX_PLUGIN_ROOT:-}"
