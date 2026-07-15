#!/usr/bin/env python3
"""MCP-flavoured equivalent of orchestrate-placed-fleet.

Requires a running phux server, registered integrations, and phux-mcp on PATH.
Each watcher uses its own stdio adapter because one MCP stdio connection serves
requests serially. All watches carry a finite timeout and event cap.
"""

from __future__ import annotations

import json
import os
import subprocess
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import Any

MCP = os.environ.get("PHUX_MCP", "phux-mcp")
SESSION = os.environ.get("PHUX_FLEET_SESSION", "agent-fleet-mcp-demo")
BUILDER = os.environ.get("PHUX_BUILDER_INTEGRATION", "codex")
REVIEWER = os.environ.get("PHUX_REVIEWER_INTEGRATION", "claude")
WORKDIR = os.environ.get("PHUX_FLEET_CWD", str(Path.cwd()))
WATCH_SECONDS = int(os.environ.get("PHUX_WATCH_SECONDS", "15"))

if WATCH_SECONDS <= 0:
    raise SystemExit("PHUX_WATCH_SECONDS must be positive")


class McpClient:
    def __init__(self) -> None:
        self.proc = subprocess.Popen(
            [MCP],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=None,
            text=True,
            bufsize=1,
        )
        self.next_id = 1
        self._request(
            "initialize",
            {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "phux-fleet-example", "version": "1"},
            },
        )
        self._write(
            {
                "jsonrpc": "2.0",
                "method": "notifications/initialized",
                "params": {},
            }
        )

    def _write(self, message: dict[str, Any]) -> None:
        assert self.proc.stdin is not None
        self.proc.stdin.write(json.dumps(message) + "\n")
        self.proc.stdin.flush()

    def _request(self, method: str, params: dict[str, Any]) -> dict[str, Any]:
        request_id = self.next_id
        self.next_id += 1
        self._write(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "method": method,
                "params": params,
            }
        )
        assert self.proc.stdout is not None
        raw = self.proc.stdout.readline()
        if not raw:
            raise RuntimeError("phux-mcp closed stdout")
        response = json.loads(raw)
        if response.get("id") != request_id:
            raise RuntimeError(f"unexpected MCP response id: {response}")
        if "error" in response:
            raise RuntimeError(response["error"])
        return response["result"]

    def call(self, name: str, arguments: dict[str, Any]) -> Any:
        result = self._request(
            "tools/call", {"name": name, "arguments": arguments}
        )
        if result.get("isError"):
            raise RuntimeError(result["content"][0]["text"])
        text = result["content"][0]["text"]
        return json.loads(text)

    def close(self) -> None:
        if self.proc.stdin is not None:
            self.proc.stdin.close()
        try:
            self.proc.wait(timeout=2)
        except subprocess.TimeoutExpired:
            self.proc.terminate()
            self.proc.wait(timeout=2)

    def __enter__(self) -> "McpClient":
        return self

    def __exit__(self, *_: object) -> None:
        self.close()


def pane(result: dict[str, Any]) -> str:
    return f"@{result['terminal_id']}"


def bounded_watch(target: str) -> list[dict[str, Any]]:
    # A separate adapter permits these calls to block concurrently.
    with McpClient() as watcher:
        batch = watcher.call(
            "phux_watch",
            {
                "target": target,
                "timeout_secs": WATCH_SECONDS,
                "max_events": 64,
            },
        )
        return batch["events"]


def main() -> None:
    with McpClient() as mcp:
        seed_result = mcp.call(
            "phux_new", {"name": SESSION, "cwd": WORKDIR}
        )
        seed = f"@{seed_result['terminal_id']}"

        builder = pane(
            mcp.call(
                "phux_launch",
                {
                    "integration": BUILDER,
                    "target": seed,
                    "split": "vertical",
                    "ratio": 0.55,
                    "cwd": WORKDIR,
                },
            )
        )
        reviewer = pane(
            mcp.call(
                "phux_launch",
                {
                    "integration": REVIEWER,
                    "target": builder,
                    "split": "horizontal",
                    "ratio": 0.5,
                    "cwd": WORKDIR,
                },
            )
        )
        coordinator = pane(
            mcp.call(
                "phux_spawn",
                {
                    "target": seed,
                    "split": "horizontal",
                    "ratio": 0.7,
                    "cwd": WORKDIR,
                    "command": [
                        "sh",
                        "-lc",
                        'printf "coordinator ready\\n"; exec sleep 3600',
                    ],
                },
            )
        )

        mcp.call(
            "phux_move_pane",
            {
                "source": reviewer,
                "target": coordinator,
                "direction": "vertical",
                "ratio": 0.5,
            },
        )
        mcp.call(
            "phux_swap_pane", {"first": builder, "second": reviewer}
        )

    with ThreadPoolExecutor(max_workers=2) as pool:
        batches = list(pool.map(bounded_watch, [builder, reviewer]))

    asks: dict[tuple[Any, Any], dict[str, Any]] = {}
    for event in (event for batch in batches for event in batch):
        if event.get("event") == "asked":
            asks[(event.get("terminal"), event.get("id"))] = event

    if asks:
        print("Blocked asks:")
        for event in asks.values():
            suggestions = event.get("suggestions") or []
            suffix = f" (suggestions: {', '.join(suggestions)})" if suggestions else ""
            print(f"  {event.get('terminal')}: {event.get('question', '')}{suffix}")
    else:
        print("No blocked asks arrived during the bounded MCP watches.")

    print(f"Fleet remains running in session: {SESSION}")
    print("Human: attach, use C-a q for next attention and C-a Q to return.")
    print("The MCP orchestration did not move focus.")


if __name__ == "__main__":
    main()
