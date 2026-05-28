#!/usr/bin/env bash
# check-docs.sh
#
# Doc-system gates for the phux repository. Enforces the contract laid
# out in docs/CONVENTIONS.md (the "discipline" layer of the doc tree).
#
# Gates:
#   - frontmatter-present : every checked .md has a YAML header with
#                           audience/stability/last-reviewed
#   - frontmatter-valid   : that header's values match the controlled
#                           vocabulary
#   - tldr-present        : first non-frontmatter / non-H1 paragraph
#                           begins with "**TL;DR.**"
#   - dead-link           : every relative `[text](path.md...)` link
#                           resolves to a real file
#   - adr-status          : every ADR's `Status:` line is one of the
#                           four blessed forms
#   - spec-version-sync   : docs/spec/CHANGELOG.md head version agrees
#                           with phux-protocol's PROTOCOL_VERSION
#                           (skipped while the SPEC split is in flight)
#
# Usage:
#   bash scripts/check-docs.sh             # run all gates
#   bash scripts/check-docs.sh --help
#   bash scripts/check-docs.sh --list
#   bash scripts/check-docs.sh --only=tldr-present
#
# Exit codes:
#   0   no violations
#   1   one or more violations
#   2   script invoked incorrectly

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

ALL_GATES=(
    frontmatter-present
    frontmatter-valid
    tldr-present
    dead-link
    adr-status
    spec-version-sync
)

# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

ONLY=""

usage() {
    cat <<'EOF'
check-docs.sh — mechanical enforcement for docs/CONVENTIONS.md

USAGE:
    bash scripts/check-docs.sh [--help] [--list] [--only=<gate>]

OPTIONS:
    --help          show this message and exit
    --list          print the gates this script implements and exit
    --only=<gate>   run only the named gate

Gates write violations to stderr prefixed with `[<gate-name>]`. The
script prints a `checked N files, M violations` summary on stdout and
exits 0 if M == 0, else 1.

See docs/CONVENTIONS.md for the contract these gates enforce.
EOF
}

list_gates() {
    echo "Gates implemented:"
    for g in "${ALL_GATES[@]}"; do
        echo "  - $g"
    done
}

for arg in "$@"; do
    case "$arg" in
        --help|-h)
            usage
            exit 0
            ;;
        --list)
            list_gates
            exit 0
            ;;
        --only=*)
            ONLY="${arg#--only=}"
            ;;
        *)
            echo "error: unknown argument: $arg" >&2
            usage >&2
            exit 2
            ;;
    esac
done

if [[ -n "$ONLY" ]]; then
    found=0
    for g in "${ALL_GATES[@]}"; do
        if [[ "$g" == "$ONLY" ]]; then
            found=1
            break
        fi
    done
    if [[ "$found" -eq 0 ]]; then
        echo "error: --only=$ONLY does not match a known gate" >&2
        list_gates >&2
        exit 2
    fi
fi

should_run() {
    local gate="$1"
    [[ -z "$ONLY" || "$ONLY" == "$gate" ]]
}

# ---------------------------------------------------------------------------
# File discovery
# ---------------------------------------------------------------------------

# Build the list of .md files we care about. Exclusions, per CONVENTIONS.md:
#   - LICENSE* (not markdown anyway, but defensive)
#   - .beads/, .direnv/, .git/, target/, research/archive/
#   - crates/*/tests/*/README.md (test fixtures, not part of the doc system)
# Inclusions:
#   - everything under docs/ and ADR/
#   - top-level .md (README, AGENTS, CLAUDE, CONTRIBUTING, ARCHITECTURE,
#     SPEC, DESIGN, VISION)
#   - research/ (excluding research/archive/) — `stability: scratch`
#     lives here and CONVENTIONS.md says frontmatter is still required.

collect_files() {
    # Top-level .md files.
    find "$ROOT" -maxdepth 1 -type f -name '*.md' \
        ! -iname 'LICENSE*' \
        -print
    # docs/ tree.
    if [[ -d "$ROOT/docs" ]]; then
        find "$ROOT/docs" -type f -name '*.md' -print
    fi
    # ADR/ tree.
    if [[ -d "$ROOT/ADR" ]]; then
        find "$ROOT/ADR" -type f -name '*.md' -print
    fi
    # research/, minus archive/.
    if [[ -d "$ROOT/research" ]]; then
        find "$ROOT/research" -type f -name '*.md' \
            -not -path "$ROOT/research/archive/*" \
            -print
    fi
}

FILES=()
while IFS= read -r f; do
    [[ -n "$f" ]] && FILES+=("$f")
done < <(collect_files | LC_ALL=C sort -u)

# ---------------------------------------------------------------------------
# Violation bookkeeping
# ---------------------------------------------------------------------------

VIOLATIONS=0

violate() {
    # violate <gate> <file> <message...>
    local gate="$1"
    local file="$2"
    shift 2
    local rel="${file#$ROOT/}"
    echo "[$gate] $rel: $*" >&2
    VIOLATIONS=$((VIOLATIONS + 1))
}

# ---------------------------------------------------------------------------
# Frontmatter parsing helpers
# ---------------------------------------------------------------------------

# Echoes the line number (1-based) of the closing `---` if the file opens
# with a YAML frontmatter block within the first 20 lines, else nothing.
frontmatter_close_line() {
    local file="$1"
    awk 'NR == 1 { if ($0 != "---") exit 0 }
         NR > 1 && NR <= 20 { if ($0 == "---") { print NR; exit 0 } }
         NR > 20 { exit 0 }' "$file"
}

# Echoes the value (everything after `key:`) for a frontmatter key, trimmed.
# Only looks inside the frontmatter block (lines 2..close-1).
# (awk's `close` is a builtin, so the variable is named `close_line`.)
frontmatter_value() {
    local file="$1"
    local key="$2"
    local close_line="$3"
    awk -v key="$key" -v close_line="$close_line" '
        NR >= 2 && NR < close_line {
            if (match($0, "^[[:space:]]*" key ":[[:space:]]*")) {
                v = substr($0, RLENGTH + 1)
                gsub(/[[:space:]]+$/, "", v)
                print v
                exit
            }
        }
    ' "$file"
}

# Is this the repo-root README.md? It is exempt from YAML frontmatter
# (would render visibly on GitHub as the project landing page) and
# from the TL;DR gate (the README's whole job is to be the landing
# page, not to summarize itself). Instead, it declares the same
# metadata via an opening HTML comment block within the first 20
# lines. See docs/CONVENTIONS.md.
is_root_readme() {
    [[ "$1" == "$ROOT/README.md" ]]
}

# For the README only: echoes the closing-comment line if an HTML
# comment block opens on line 1 and closes within the first 20 lines.
readme_html_close_line() {
    local file="$1"
    awk 'NR == 1 { if ($0 != "<!--") exit 0 }
         NR > 1 && NR <= 20 { if ($0 ~ /-->[[:space:]]*$/) { print NR; exit 0 } }
         NR > 20 { exit 0 }' "$file"
}

# ---------------------------------------------------------------------------
# Gate 1: frontmatter-present
# ---------------------------------------------------------------------------

gate_frontmatter_present() {
    local file close
    for file in "${FILES[@]}"; do
        # README.md exception: HTML-comment metadata in place of YAML.
        if is_root_readme "$file"; then
            close="$(readme_html_close_line "$file" || true)"
            if [[ -z "$close" ]]; then
                violate frontmatter-present "$file" \
                    "missing or malformed HTML-comment metadata (need '<!--' on line 1 and '-->' within the first 20 lines; README is exempt from YAML frontmatter — see docs/CONVENTIONS.md)"
                continue
            fi
            local rmissing=()
            for key in audience stability last-reviewed; do
                if ! awk -v key="$key" -v close_line="$close" '
                        NR >= 2 && NR < close_line {
                            if (match($0, "^[[:space:]]*" key ":[[:space:]]*[^[:space:]]")) { found = 1; exit }
                        }
                        END { exit (found ? 0 : 1) }
                    ' "$file"; then
                    rmissing+=("$key")
                fi
            done
            if (( ${#rmissing[@]} > 0 )); then
                violate frontmatter-present "$file" \
                    "HTML-comment metadata missing key(s): ${rmissing[*]}"
            fi
            continue
        fi

        close="$(frontmatter_close_line "$file" || true)"
        if [[ -z "$close" ]]; then
            violate frontmatter-present "$file" \
                "missing or malformed YAML frontmatter (need '---' on line 1 and a closing '---' within the first 20 lines)"
            continue
        fi
        local missing=()
        for key in audience stability last-reviewed; do
            local val
            val="$(frontmatter_value "$file" "$key" "$close" || true)"
            if [[ -z "$val" ]]; then
                missing+=("$key")
            fi
        done
        if (( ${#missing[@]} > 0 )); then
            violate frontmatter-present "$file" \
                "frontmatter missing key(s): ${missing[*]}"
        fi
    done
}

# ---------------------------------------------------------------------------
# Gate 2: frontmatter-valid
# ---------------------------------------------------------------------------

# Allowed sets.
AUDIENCE_ALLOWED='^(humans|agents|consumers|contributors)$'
STABILITY_ALLOWED='^(stable|evolving|scratch)$'
DATE_RE='^[0-9]{4}-[0-9]{2}-[0-9]{2}$'

gate_frontmatter_valid() {
    local file close
    for file in "${FILES[@]}"; do
        close="$(frontmatter_close_line "$file" || true)"
        # If there's no frontmatter, gate 1 already complained; skip here.
        [[ -z "$close" ]] && continue

        local audience stability reviewed
        audience="$(frontmatter_value "$file" "audience" "$close" || true)"
        stability="$(frontmatter_value "$file" "stability" "$close" || true)"
        reviewed="$(frontmatter_value "$file" "last-reviewed" "$close" || true)"

        # audience: comma-separated list of allowed words.
        if [[ -n "$audience" ]]; then
            local cleaned
            cleaned="$(echo "$audience" | tr ',' '\n' | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')"
            local bad=""
            while IFS= read -r token; do
                [[ -z "$token" ]] && continue
                if ! [[ "$token" =~ $AUDIENCE_ALLOWED ]]; then
                    bad+="${bad:+, }$token"
                fi
            done <<< "$cleaned"
            if [[ -n "$bad" ]]; then
                violate frontmatter-valid "$file" \
                    "audience: invalid value(s): $bad (allowed: humans, agents, consumers, contributors)"
            fi
        fi

        # stability: single word from the allowed set.
        if [[ -n "$stability" ]]; then
            if ! [[ "$stability" =~ $STABILITY_ALLOWED ]]; then
                violate frontmatter-valid "$file" \
                    "stability: invalid value '$stability' (allowed: stable, evolving, scratch)"
            fi
        fi

        # last-reviewed: ISO date.
        if [[ -n "$reviewed" ]]; then
            if ! [[ "$reviewed" =~ $DATE_RE ]]; then
                violate frontmatter-valid "$file" \
                    "last-reviewed: '$reviewed' is not a YYYY-MM-DD date"
            fi
        fi
    done
}

# ---------------------------------------------------------------------------
# Gate 3: tldr-present
# ---------------------------------------------------------------------------

# Per CONVENTIONS.md: the first non-blank, non-frontmatter, non-H1 line
# of content must start with `**TL;DR.**`. We tolerate any number of
# blank lines between the H1 and the TL;DR, and we treat the absence of
# a frontmatter block as "skip" — gate 1 already flagged that case.

gate_tldr_present() {
    local file close
    for file in "${FILES[@]}"; do
        # README.md is exempt — its whole job is to be the landing page;
        # a TL;DR paragraph would degrade that. See docs/CONVENTIONS.md.
        if is_root_readme "$file"; then
            continue
        fi
        close="$(frontmatter_close_line "$file" || true)"
        local start=1
        if [[ -n "$close" ]]; then
            start=$((close + 1))
        fi
        local result
        result="$(awk -v start="$start" '
            NR < start { next }
            {
                # strip CR
                sub(/\r$/, "")
                # skip blank lines
                if ($0 ~ /^[[:space:]]*$/) next
                # skip a single H1 (first non-blank H1 only)
                if (!seen_h1 && $0 ~ /^#[[:space:]]/) {
                    seen_h1 = 1
                    next
                }
                # first non-blank, non-H1 line of content.
                if ($0 ~ /^\*\*TL;DR\.\*\*/) {
                    print "ok"
                } else {
                    print "bad:" $0
                }
                printed = 1
                exit
            }
            END {
                if (!printed) print "empty"
            }
        ' "$file" || true)"
        case "$result" in
            ok) ;;
            bad:*)
                violate tldr-present "$file" \
                    "first content line is not a '**TL;DR.**' paragraph (saw: ${result#bad:})"
                ;;
            empty|"")
                violate tldr-present "$file" \
                    "no content found after frontmatter (need an H1 and a '**TL;DR.**' paragraph)"
                ;;
        esac
    done
}

# ---------------------------------------------------------------------------
# Gate 4: dead-link
# ---------------------------------------------------------------------------

# Match `[text](path)` where `path` does not start with a scheme
# (http://, https://, mailto:, #...) and is not absolute (`/...`).
# Anchors (`#...`) on the end are stripped before resolving.

gate_dead_link() {
    local file
    for file in "${FILES[@]}"; do
        local dir
        dir="$(dirname "$file")"
        # Extract every `](...)` target. We use grep -oE then awk-clean.
        # The regex purposely keeps it simple: balanced parens inside the
        # URL aren't supported (markdown's spec discourages them anyway).
        while IFS= read -r link; do
            [[ -z "$link" ]] && continue
            # Skip absolute URLs.
            case "$link" in
                http://*|https://*|mailto:*|ftp://*|tel:*|ws://*|wss://*)
                    continue ;;
                '#'*)
                    # Pure in-page anchor — nothing to resolve.
                    continue ;;
                /*)
                    # Repo-absolute or filesystem-absolute. Resolve from
                    # repo root rather than treating it as filesystem-/.
                    : ;;
            esac

            # Strip anchor and optional query.
            local target="${link%%#*}"
            target="${target%%\?*}"
            [[ -z "$target" ]] && continue

            local resolved
            case "$target" in
                /*)
                    resolved="$ROOT$target"
                    ;;
                *)
                    resolved="$dir/$target"
                    ;;
            esac

            if [[ ! -e "$resolved" ]]; then
                violate dead-link "$file" \
                    "broken relative link: $link"
            fi
        done < <(grep -oE '\]\([^)]+\)' "$file" 2>/dev/null \
                  | sed -E 's/^\]\(//; s/\)$//' \
                  | awk '{print $1}')
        # ^ awk {print $1} strips any optional title `](path "title")`.
    done
}

# ---------------------------------------------------------------------------
# Gate 5: adr-status
# ---------------------------------------------------------------------------

# Allowed statuses (anchored, exact):
#   Status: Accepted
#   Status: Accepted (forward-compat)
#   Status: Superseded by ADR-NNNN
#   Status: Deprecated

gate_adr_status() {
    local file
    if [[ ! -d "$ROOT/ADR" ]]; then
        return
    fi
    while IFS= read -r file; do
        # Skip the ADR README.
        if [[ "$(basename "$file")" == "README.md" ]]; then
            continue
        fi
        local close
        close="$(frontmatter_close_line "$file" || true)"
        local start=1
        [[ -n "$close" ]] && start=$((close + 1))

        # First `Status:` line outside frontmatter.
        local status_line
        status_line="$(awk -v start="$start" '
            NR < start { next }
            /^Status:/ { sub(/\r$/, ""); print; exit }
        ' "$file" || true)"

        if [[ -z "$status_line" ]]; then
            violate adr-status "$file" "no 'Status:' line found"
            continue
        fi

        case "$status_line" in
            "Status: Accepted"|\
            "Status: Accepted (forward-compat)"|\
            "Status: Deprecated")
                ;;
            "Status: Superseded by ADR-"[0-9][0-9][0-9][0-9])
                ;;
            *)
                violate adr-status "$file" \
                    "non-vocabulary status: '$status_line' (allowed: 'Status: Accepted', 'Status: Accepted (forward-compat)', 'Status: Superseded by ADR-NNNN', 'Status: Deprecated')"
                ;;
        esac
    done < <(find "$ROOT/ADR" -type f -name '*.md' | LC_ALL=C sort)
}

# ---------------------------------------------------------------------------
# Gate 6: spec-version-sync
# ---------------------------------------------------------------------------

# Compare the head version listed in docs/spec/CHANGELOG.md (first table
# row that starts with `| <version> |`) against the PROTOCOL_VERSION
# constant in crates/phux-protocol/src/lib.rs.
#
# CONVENTIONS.md notes the SPEC split is in flight; docs/spec/CHANGELOG.md
# does not yet exist. While that's true, this gate emits a single NOTE
# line and does nothing else — it activates automatically once the file
# appears.

gate_spec_version_sync() {
    local changelog="$ROOT/docs/spec/CHANGELOG.md"
    if [[ ! -f "$changelog" ]]; then
        echo "[spec-version-sync] NOTE: $changelog does not exist yet; gate is dormant until the SPEC split lands." >&2
        return
    fi

    # First table row: a line beginning with `| ` followed by a non-pipe
    # version token, then ` |`. We tolerate leading whitespace.
    local head_version
    head_version="$(awk '
        /^[[:space:]]*\|[[:space:]]*[^|[:space:]]+[[:space:]]*\|/ {
            # Strip leading `|` and surrounding whitespace.
            line = $0
            sub(/^[[:space:]]*\|[[:space:]]*/, "", line)
            # Token is everything up to the next `|`.
            n = index(line, "|")
            if (n > 0) {
                v = substr(line, 1, n - 1)
                sub(/[[:space:]]+$/, "", v)
                # Skip header / separator rows like `| Version |` or `|---|`.
                if (v ~ /^[-: ]+$/) next
                if (tolower(v) == "version") next
                print v
                exit
            }
        }
    ' "$changelog")"

    if [[ -z "$head_version" ]]; then
        violate spec-version-sync "$changelog" \
            "could not parse a head version from the first table row"
        return
    fi

    # Pull PROTOCOL_VERSION out of phux-protocol's lib.rs.
    local lib="$ROOT/crates/phux-protocol/src/lib.rs"
    if [[ ! -f "$lib" ]]; then
        violate spec-version-sync "$lib" \
            "crates/phux-protocol/src/lib.rs not found; cannot verify PROTOCOL_VERSION"
        return
    fi

    local major minor patch
    major="$(awk '/PROTOCOL_VERSION/{flag=1} flag && /major:/{gsub(/[^0-9]/,"",$0); print; exit}' "$lib")"
    minor="$(awk '/PROTOCOL_VERSION/{flag=1} flag && /minor:/{gsub(/[^0-9]/,"",$0); print; exit}' "$lib")"
    patch="$(awk '/PROTOCOL_VERSION/{flag=1} flag && /patch:/{gsub(/[^0-9]/,"",$0); print; exit}' "$lib")"

    if [[ -z "$major" || -z "$minor" || -z "$patch" ]]; then
        violate spec-version-sync "$lib" \
            "could not parse PROTOCOL_VERSION fields (major/minor/patch)"
        return
    fi

    local code_version="${major}.${minor}.${patch}"

    # The changelog's version may carry a pre-release suffix (e.g.
    # `0.2.0-draft.2`); the code constant won't. Compare the
    # `MAJOR.MINOR.PATCH` head only.
    local changelog_core="${head_version%%-*}"

    if [[ "$changelog_core" != "$code_version" ]]; then
        violate spec-version-sync "$changelog" \
            "head version '$head_version' (core '$changelog_core') disagrees with PROTOCOL_VERSION '$code_version' in crates/phux-protocol/src/lib.rs"
    fi
}

# ---------------------------------------------------------------------------
# Run
# ---------------------------------------------------------------------------

run_gate() {
    local gate="$1"
    should_run "$gate" || return 0
    case "$gate" in
        frontmatter-present) gate_frontmatter_present ;;
        frontmatter-valid)   gate_frontmatter_valid   ;;
        tldr-present)        gate_tldr_present        ;;
        dead-link)           gate_dead_link           ;;
        adr-status)          gate_adr_status          ;;
        spec-version-sync)   gate_spec_version_sync   ;;
        *) echo "internal error: unknown gate '$gate'" >&2; exit 2 ;;
    esac
}

for gate in "${ALL_GATES[@]}"; do
    run_gate "$gate"
done

echo "checked ${#FILES[@]} files, ${VIOLATIONS} violations"

if (( VIOLATIONS > 0 )); then
    exit 1
fi
exit 0
