import assert from "node:assert/strict";
import test from "node:test";

import PhuxPlugin, { PhuxCli, PhuxPlugin as NamedPhuxPlugin } from "../dist/index.js";

test("public plugin entrypoint loads without hooks, tools, or external work", async () => {
  assert.equal(PhuxPlugin, NamedPhuxPlugin);
  assert.deepEqual(await PhuxPlugin({}), {});
});

test("packed entrypoint exposes the shared PhuxCli adapter behavior", async () => {
  const requests = [];
  const cli = new PhuxCli({
    executable: "/opt/bin/phux",
    socket: "/tmp/phux.sock",
    runner: async (request) => {
      requests.push(request);
      return {
        termination: "completed",
        exitCode: 0,
        stdout: JSON.stringify({
          schema_version: 1,
          sessions: [{ name: "shared", windows: 1, attached: false }],
        }),
        stderr: "",
      };
    },
  });

  const result = await cli.ls();

  assert.deepEqual(result.sessions, [{ name: "shared", windows: 1, attached: false }]);
  assert.deepEqual(requests.map(({ executable, args }) => ({ executable, args })), [{
    executable: "/opt/bin/phux",
    args: ["ls", "--json", "--socket", "/tmp/phux.sock"],
  }]);
});
