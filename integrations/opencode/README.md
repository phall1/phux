# @phux/opencode

Minimal public OpenCode plugin scaffold for the external `phux` terminal binary.
This package intentionally registers no tools or lifecycle hooks yet. Loading the
plugin does not start `phux`, contact an LLM, use OpenCode TUI internals, or pair
to a remote service.

## Public entrypoint

The package default export (also exported as `PhuxPlugin`) implements OpenCode's
public server plugin function and currently resolves to an empty hooks object:

```ts
import PhuxPlugin, { PhuxCli } from "@phux/opencode";

await PhuxPlugin(pluginInput); // {}
const phux = new PhuxCli();    // executes the external `phux` binary only when called
```

An OpenCode config can load the packed package by its npm name:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "plugin": ["@phux/opencode"]
}
```

For a local build, use the absolute `file:///.../dist/index.js` URL instead. Run
`npm run smoke:opencode` after `npm run build` to load that URL with `opencode
debug config` under temporary HOME/XDG directories. This smoke path resolves
configuration and loads plugins without sending a prompt or invoking an LLM.
`OPENCODE_BIN` may select a specific OpenCode executable.

## Official SDK evidence and pin

The development dependency is pinned exactly to `@opencode-ai/plugin` `1.18.1`.
The installed official package reports the same version in its `package.json`,
exports `./dist/index.js` with `./dist/index.d.ts` as its public root, and its
root declaration defines:

```ts
type Plugin = (input: PluginInput, options?: PluginOptions) => Promise<Hooks>;
```

It also defines `PluginModule` as a server plugin with optional `id` and no TUI
entrypoint. This scaffold uses only the root `Plugin` type. It does not import
the package's optional `./tui` export or any OpenCode internal module. Re-run
`npm view @opencode-ai/plugin version` and inspect
`node_modules/@opencode-ai/plugin/{package.json,dist/index.d.ts}` before changing
the pin or adopting more hooks.

## Shared adapter and package boundary

`PhuxCli` remains implemented once in `integrations/pi/src/adapter.ts` with its
runner, errors, and CLI schemas. The OpenCode source re-exports that adapter,
and `tsup` bundles the transitive host-independent source during build. Thus the
npm tarball is standalone: it has no sibling-path or `@phux/pi` runtime
requirement. `@opencode-ai/plugin` is type-only and remains a pinned development
dependency. The `phux` executable itself is deliberately external and is never
embedded.

The next integration step should construct `PhuxCli` from documented plugin
options and expose a small set of public OpenCode `tool`/lifecycle hooks. That
work must preserve subprocess cancellation and output limits from the shared
adapter; it is outside this scaffold.

## Development

```sh
npm ci
npm run typecheck
npm test
npm run smoke:opencode # requires an opencode executable; no LLM call
```

`npm test` builds, runs focused plugin/adapter tests, and installs the generated
tarball into a temporary consumer to verify it is standalone.
