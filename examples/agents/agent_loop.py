#!/usr/bin/env python3
"""The read+act+wait loop, as an agent would wire it in code.

A self-contained illustration of driving phux purely via its CLI: every
phux interaction is a subprocess call, and the JSON verbs (`ls --json`,
`snapshot --json`, `run --json`) are parsed straight into dicts. There is
no phux client library here on purpose -- the CLI *is* the agent surface
(ADR-0022). Any language that can spawn a process and parse JSON can drive
phux this way.

Run it:   python3 examples/agents/agent_loop.py

It stands up a throwaway server on a private socket so it never touches
the user's real one-per-user server. A production agent skips that and
just runs `phux <verb>` with no --socket.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path


def find_phux() -> str:
    """Locate a phux binary: $PHUX, then PATH, then this repo's debug build."""
    if env := os.environ.get("PHUX"):
        return env
    if found := shutil.which("phux"):
        return found
    repo_root = Path(__file__).resolve().parents[2]
    candidate = repo_root / "target" / "debug" / "phux"
    if not candidate.exists():
        print("agent_loop: building phux (one-time, may be slow)...", file=sys.stderr)
        subprocess.run(
            ["nix", "develop", "-c", "cargo", "build", "-p", "phux"],
            cwd=repo_root,
            check=True,
        )
    return str(candidate)


class Phux:
    """A thin CLI wrapper: each method is one `phux <verb>` invocation.

    `--socket` is a per-subcommand flag and must precede a verb's trailing
    positional args, so it is inserted right after the verb here.
    """

    def __init__(self, binary: str, socket: str, session: str) -> None:
        self.binary = binary
        self.socket = socket
        self.session = session

    def _run(self, verb: str, *args: str, check: bool = True) -> subprocess.CompletedProcess:
        cmd = [self.binary, verb, "--socket", self.socket, *args]
        return subprocess.run(cmd, capture_output=True, text=True, check=check)

    def ls(self) -> list[dict]:
        """Enumerate sessions (the `ls --json` contract)."""
        out = self._run("ls", "--json").stdout
        return json.loads(out)["sessions"]

    def snapshot(self) -> dict:
        """Read the focused pane as structured data (side-effect-free)."""
        out = self._run("snapshot", self.session, "--json").stdout
        return json.loads(out)

    def send_keys(self, *keys: str) -> None:
        """Send named keys and/or literal strings to the focused pane."""
        self._run("send-keys", self.session, *keys)

    def run(self, command: str, timeout_s: int = 30) -> dict:
        """Run a discrete command; return {command, exit_code, output, ...}.

        `run` mirrors the child's exit code into its own, so check=False
        here: a non-zero command is data, not a Python exception.
        """
        proc = self._run(
            "run", "--json", "--timeout", str(timeout_s), self.session, command,
            check=False,
        )
        return json.loads(proc.stdout)

    def wait_until(self, text: str, timeout_s: int = 10) -> bool:
        """Block until a visible line contains `text`. True if met, False on timeout.

        `--until` matches the echo of the command you typed too, so wait on
        text that appears only in OUTPUT.
        """
        proc = self._run(
            "wait", "--until", text, "--timeout", str(timeout_s), self.session,
            check=False,
        )
        return proc.returncode == 0  # 124 == timed out

    def wait_idle(self, idle_ms: int = 500, timeout_s: int = 10) -> bool:
        """Block until the screen holds still for `idle_ms`."""
        proc = self._run(
            "wait", "--idle", str(idle_ms), "--timeout", str(timeout_s), self.session,
            check=False,
        )
        return proc.returncode == 0


def main() -> int:
    binary = find_phux()
    tmpdir = tempfile.mkdtemp(prefix="phux-agent-loop.")
    socket = str(Path(tmpdir) / "phux.sock")
    session = "demo"

    # Stand up the throwaway server and wait for its socket to bind.
    server = subprocess.Popen(
        [binary, "server", "--session", session, "--socket", socket],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        for _ in range(200):
            if Path(socket).exists():
                break
            time.sleep(0.025)
        else:
            print("agent_loop: server did not bind", file=sys.stderr)
            return 1

        phux = Phux(binary, socket, session)

        # 1. READ: what sessions exist?
        print("sessions:", [s["name"] for s in phux.ls()])

        # 2. ACT/READ via run: a discrete command, exit code as data.
        result = phux.run("echo hello; echo world")
        print(f"run exit={result['exit_code']} output={result['output']!r}")

        # 3. The interactive loop: drive a prompt, answer it, confirm.
        #
        # Two deliberate choices (see 04-read-act-wait-loop.sh for the
        # rationale): wrap the program in `sh -c` so it is portable across
        # login shells, and wait on a value the program COMPUTES at runtime
        # (`result=49`) -- which appears only in output, never in the
        # echoed keystroke or the command source.
        prog = "sh -c 'printf \"Pick a number: \"; read n; echo \"result=$((n * n))\"'"
        phux.send_keys(prog, "Enter")

        if not phux.wait_until("Pick a number:"):
            print("agent_loop: prompt never appeared", file=sys.stderr)
            return 1

        # READ the waiting prompt before deciding.
        scr = phux.snapshot()
        prompt = next((l for l in scr["lines"] if "Pick a number:" in l), "")
        print(f"prompt on screen: {prompt.strip()!r}")

        # ACT: answer, then WAIT on the computed result (not our echoed key).
        phux.send_keys("7", "Enter")
        if phux.wait_until("result=49"):
            print("loop complete: program computed 7*7=49")
        else:
            print("agent_loop: program did not compute the result", file=sys.stderr)
            return 1

        phux.wait_idle()
        return 0
    finally:
        server.terminate()
        shutil.rmtree(tmpdir, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
