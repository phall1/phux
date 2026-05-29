#!/usr/bin/env bash
#
# 03 — interactive input and blocking on conditions: `send-keys` + `wait`.
#
# `run` is for discrete commands with a clean "done" moment. For anything
# interactive — a REPL, a TUI, a backgrounded process, a program that
# prompts — you drop to `send-keys` (structured input) and `wait` (block
# until the screen meets a condition).
#
#   send-keys NAME KEY...   each arg is a named key (Enter, Tab, C-c, Up,
#                           M-x, ...) or a literal string sent char by char.
#   wait NAME --until TEXT  succeed when any visible line contains TEXT.
#   wait NAME --idle MS     succeed when the screen holds still for MS.
#   wait ... --timeout SECS give up after SECS (exit 124).
#
# Run it:   bash examples/agents/03-send-keys-and-wait.sh

source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
phux_start_server

# --- send-keys: type a command and run it ----------------------------------
# Start a backgrounded job. We use a marker that appears only in OUTPUT,
# never in the typed command, so `wait --until` can't match the echo.
section "send-keys: start a backgrounded job"
phux send-keys "$PHUX_SESSION" "(sleep 1; echo JOB_FINISHED) &" Enter

# --- wait --until: block until output appears ------------------------------
# `--until` matches ANY visible line, INCLUDING the shell's echo of the
# command you just typed. Match on text that appears only in output.
section "wait --until JOB_FINISHED (with a timeout safety net)"
if phux wait --until JOB_FINISHED --timeout 10 "$PHUX_SESSION"; then
    echo "  job finished; the marker is on screen"
fi

# --- wait --timeout: the miss path -----------------------------------------
# When the condition never occurs, `wait` exits 124 after --timeout. An
# agent uses this to bound a poll instead of hanging forever.
section "wait --until (deliberate miss -> exit 124)"
phux wait --until THIS_NEVER_APPEARS --timeout 2 "$PHUX_SESSION" \
    && echo "  (unreachable)" \
    || echo "  timed out as expected (exit $?)"

# --- wait --idle: block until the screen settles ---------------------------
# No specific string to match? Wait for the pane to stop changing. Good
# after kicking off output you can't predict the tail of.
section "wait --idle: block until the pane settles"
phux send-keys "$PHUX_SESSION" "for i in 1 2 3; do echo tick \$i; sleep 0.2; done" Enter
phux wait --idle 500 --timeout 10 "$PHUX_SESSION"
echo "  screen has settled; safe to read it"
phux snapshot "$PHUX_SESSION"

# --- talking to a REPL ------------------------------------------------------
# send-keys is how you converse with an interactive program. Here: start
# python, ask it something, wait for the answer, then exit it cleanly.
section "drive a REPL with send-keys + wait"
phux send-keys "$PHUX_SESSION" "python3 -q" Enter
phux wait --idle 750 --timeout 10 "$PHUX_SESSION"   # let the banner/prompt settle
phux send-keys "$PHUX_SESSION" "print(6 * 7)" Enter
phux wait --until 42 --timeout 10 "$PHUX_SESSION"
echo "  the REPL answered 42"
# Leave the REPL so the pane is back at a shell prompt. (`exit()` here only
# leaves python, not the pane's shell, so the session survives.)
phux send-keys "$PHUX_SESSION" "exit()" Enter
phux wait --idle 750 --timeout 10 "$PHUX_SESSION"
