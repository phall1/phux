import assert from "node:assert/strict";
import test from "node:test";

import {
  parseInsertPaneResult,
  parseLaunchResult,
  parseMovePaneResult,
  parseRenderedFrame,
  parseScreenState,
  parseSessionList,
  parseSwapPaneResult,
  parseWatchEvent,
  SchemaValidationError,
} from "../src/schemas.js";
import { truncateLines, truncateText } from "../src/truncate.js";

test("session-list parser accepts v2 terminal inventory and normalizes v1", () => {
  assert.deepEqual(parseSessionList({ schema_version: 1, sessions: [] }), {
    schema_version: 1,
    sessions: [],
    terminals: [],
  });
  assert.deepEqual(parseSessionList({
    schema_version: 2,
    sessions: [{ name: "work", windows: 1, attached: false }],
    terminals: ["@3", "devbox/@7"],
  }), {
    schema_version: 2,
    sessions: [{ name: "work", windows: 1, attached: false }],
    terminals: ["@3", "devbox/@7"],
  });
  assert.throws(
    () => parseSessionList({ schema_version: 2, sessions: [] }),
    SchemaValidationError,
  );
});

test("screen parser validates dimensions and normalizes additive fields", () => {
  const screen = parseScreenState({
    schema_version: 1,
    pane: 2,
    cols: 80,
    rows: 1,
    cursor: { x: 0, y: 0, visible: true },
    lines: ["$"],
  });
  assert.deepEqual(screen.scrollback, []);
  assert.throws(() => parseScreenState({ ...screen, rows: 2 }), SchemaValidationError);
});

test("new machine parsers reject incompatible versions and malformed event payloads", () => {
  assert.throws(() => parseLaunchResult({
    schema_version: 2, terminal_id: 1, integration: "codex", plugin: "agents", argv: ["codex"],
  }), SchemaValidationError);
  assert.throws(() => parseWatchEvent({ event: "asked", terminal: "@1", id: "q" }), SchemaValidationError);
  assert.throws(() => parseRenderedFrame({
    schema_version: 1, cols: 2, rows: 1, cursor: null, cells: [],
  }), /exactly 2 entries/);
});

test("spatial parsers require canonical operations, fields, directions, and ratios", () => {
  assert.equal(parseInsertPaneResult({
    schema_version: 1, operation: "insert-pane", session_id: 1,
    target_terminal_id: 3, new_terminal_id: 4, direction: "vertical", ratio: 0.4,
  }).new_terminal_id, 4);
  assert.equal(parseMovePaneResult({
    schema_version: 1, operation: "move-pane", session_id: 1,
    source_terminal_id: 4, target_terminal_id: 3, direction: "horizontal", ratio: 0.6,
  }).source_terminal_id, 4);
  assert.equal(parseSwapPaneResult({
    schema_version: 1, operation: "swap-pane", session_id: 1,
    first_terminal_id: 3, second_terminal_id: 4,
  }).second_terminal_id, 4);
  assert.throws(() => parseInsertPaneResult({
    schema_version: 1, operation: "move-pane", session_id: 1,
    target_terminal_id: 3, new_terminal_id: 4, direction: "vertical", ratio: 0.4,
  }), SchemaValidationError);
  assert.throws(() => parseMovePaneResult({
    schema_version: 1, operation: "move-pane", session_id: 1,
    source_terminal_id: 4, target_terminal_id: 3, direction: "diagonal", ratio: 1,
  }), SchemaValidationError);
});

test("truncation helpers bound output and retain newest lines", () => {
  const text = truncateText("abcdefghij", 5);
  assert.equal(Array.from(text.text).length, 5);
  assert.equal(text.truncated, true);
  assert.equal(text.omittedChars, 5);

  const marked = truncateText("x".repeat(100), 50);
  const match = /\.\.\. (\d+) characters omitted \.\.\./.exec(marked.text);
  assert.notEqual(match, null);
  const markerLength = Array.from(match?.[0] ?? "").length + 2; // surrounding newlines
  assert.equal(marked.omittedChars, 100 - (50 - markerLength));
  assert.equal(Number(match?.[1]), marked.omittedChars);

  assert.deepEqual(truncateLines(["old", "middle", "new"], 2), {
    lines: ["middle", "new"],
    truncated: true,
    omittedLines: 1,
  });
});
