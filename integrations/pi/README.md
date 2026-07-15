# @phux/pi

Pi package for selecting and operating shared phux terminals. Installation,
compatibility, the exact tool and command surface, lifecycle behavior, and
safety boundaries live in the canonical
[Pi integration guide](../../docs/consumers/pi.md).

## Development and validation

Install the locked development dependencies from this directory:

```sh
npm ci
```

Run the deterministic gates, which do not call an LLM:

```sh
npm test
npm run typecheck
npm run build
npm run pack:check
npm run smoke:load
```

The real integration harness is opt-in:

```sh
PHUX_PI_REAL_SMOKE=1 npm run smoke:real
```

`pack:check`, `smoke:load`, and `smoke:real` own the package-local artifact and
harness checks. Consumer setup and operational interpretation remain in the
[Pi integration guide](../../docs/consumers/pi.md).
