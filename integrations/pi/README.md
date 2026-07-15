# @phux/pi

Pi package for operating shared phux terminals. This first scaffold contains a
host-independent, typed adapter around the external `phux` CLI. Pi tools, TUI,
and lifecycle integration are intentionally deferred.

## Development

```sh
npm install
npm test
npm run typecheck
```

`phux` must be installed separately and available on `PATH`, or its path can be
passed as `new PhuxCli({ executable: "/path/to/phux" })`.
