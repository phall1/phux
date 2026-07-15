import assert from "node:assert/strict";
import test from "node:test";

import { parseScreenState, parseSessionList, SchemaValidationError } from "../src/schemas.js";
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
