import assert from "node:assert/strict";
import test from "node:test";

import {
  parseLaunchResult,
  parseRenderedFrame,
  parseScreenState,
  parseWatchEvent,
  SchemaValidationError,
} from "../src/schemas.js";
import { truncateLines, truncateText } from "../src/truncate.js";

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
