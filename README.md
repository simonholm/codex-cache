# codex-cache

Inspect, verify, and safely clean Codex release caches.

Codex keeps releases in two package stores under `$CODEX_HOME/packages` (or
`$HOME/.codex/packages`): `standalone/releases` and
`app-server-daemon/releases`. Each package has its own `current` symlink.
Over time, inactive releases can accumulate and use several GiB of disk space.
`codex-cache` reports installed releases, validates each package layout, and
can delete inactive release directories while preserving each active release.
An absent package store is skipped when the other is present.

## Features

- `scan` — inspect the installed package caches
- `report` — summarize versions, cache size, and reclaim estimates
- `list` — list installed releases and their sizes
- `verify` — validate the cache layout and detect inconsistencies
- `clean --dry-run` — preview which releases would be removed
- `clean` — delete inactive releases selected by the keep policy

## Safety

Cleanup is conservative and bounded to these two Codex release caches:

- verification runs before real deletion, and cleanup aborts if verification
  fails
- each package's active release and `current` symlink target are protected
- deletion candidates must be inside their package's `releases/` directory
- `clean --dry-run` shows the exact directories and bytes before anything is
  removed

The keep policy applies independently to each package:

- `current` keeps only each active release and removes inactive releases
- `current,previous` keeps each active release plus its newest inactive release

## Build

```bash
cargo build --release
```

## Example

```bash
codex-cache report
codex-cache list
codex-cache verify
codex-cache clean --dry-run --keep current
codex-cache clean --keep current
```

For a more conservative cleanup, keep the newest inactive release as a rollback
candidate:

```bash
codex-cache clean --dry-run --keep current,previous
codex-cache clean --keep current,previous
```
