# @phux/pi

Pi package for selecting and operating terminals owned by an external phux
server. The extension supplies Pi-native tools, target selection and handoff
commands, branch-aware target persistence, and best-effort lifecycle metadata.
The canonical consumer behavior and safety boundaries are documented in the
[Pi integration guide](../../docs/consumers/pi.md).

## Requirements

- Node.js 20 or newer.
- Pi compatible with the package peer APIs (development is pinned to Pi
  `0.80.7`).
- `phux >= 0.1.0` installed separately and available on `PATH`.

The package does not bundle phux, embed a terminal, or support remote pairing.
It is not currently published to npm. The repository root is not a Pi package.

## Install from this checkout

Run this from the repository root:

```sh
pi install ./integrations/pi
```

Pi records the package directory in its settings. Start Pi normally after the
install. If phux uses a non-default local socket, export `PHUX_SOCKET` before
starting Pi.

For an artifact-shaped local install, build a tarball and give that file to Pi:

```sh
cd integrations/pi
npm ci
npm pack
mkdir -p phux-pi-packed
tar -xzf phux-pi-0.1.0.tgz -C phux-pi-packed
pi install ./phux-pi-packed/package
```

This is local packed-tarball use, not an npm publication claim. `npm pack` runs
the TypeScript build before creating the archive; Pi installs the extracted
package directory rather than loading the `.tgz` itself.

## Verify the setup

```sh
phux --version
pi list
```

Inside Pi, `/phux` selects a default pane, `/phux-status` checks it, and
`/phux-attach` prints a local human attach argv without executing it. See the
[consumer guide](../../docs/consumers/pi.md) for the exact tool/command surface,
target staleness behavior, lifecycle ownership, and concurrency warnings.

## Development and validation

Install the locked development dependencies:

```sh
npm ci
```

The deterministic gates do not call an LLM:

```sh
npm test              # unit tests, archive contents, packed extension load
npm run typecheck
npm run build
npm run pack:check    # inspect a temporary npm archive
npm run smoke:load    # install that archive into an isolated Pi dir and query RPC
```

`smoke:load` isolates `PI_CODING_AGENT_DIR`, Pi sessions, `PHUX_SOCKET`, and XDG
directories. It asks Pi RPC for registered commands and exits without sending a
model prompt.

The real phux smoke is opt-in:

```sh
PHUX_PI_REAL_SMOKE=1 npm run smoke:real
```

It starts a foreground server on a temporary socket with temporary XDG
directories, creates a session, runs a marker command, snapshots the pane,
prints the safe human attach argv, and stops that server. It never contacts or
starts the user's default phux server. Set `PHUX=/absolute/path/to/phux` to test
a non-default binary.

The public adapter can also select an executable explicitly:

```ts
import { PhuxCli } from "@phux/pi";

const phux = new PhuxCli({ executable: "/absolute/path/to/phux" });
```

That constructor option is for library consumers; the installed extension uses
`phux` from `PATH`.
