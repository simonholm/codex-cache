# codex-cache

Inspect, verify, and safely clean the Codex standalone release cache.

Codex standalone installs are kept under
`$CODEX_HOME/packages/standalone/releases` or
`$HOME/.codex/packages/standalone/releases`. Over time, inactive releases can
accumulate and use several GiB of disk space. `codex-cache` reports the
installed releases, validates the cache layout, and can delete inactive release
directories while preserving the active installation.

## Features

- `scan` — inspect the installed standalone cache
- `report` — summarize versions, cache size, and reclaim estimates
- `list` — list installed releases and their sizes
- `verify` — validate the cache layout and detect inconsistencies
- `clean --dry-run` — preview which releases would be removed
- `clean` — delete inactive releases selected by the keep policy

## Safety

Cleanup is conservative and bounded to the Codex standalone release cache:

- verification runs before real deletion, and cleanup aborts if verification
  fails
- the active release and the `current` symlink target are always protected
- deletion candidates must be inside the `releases/` directory
- `clean --dry-run` shows the exact directories and bytes before anything is
  removed

The keep policy controls which releases remain:

- `current` keeps only the active release and removes all inactive releases
- `current,previous` keeps the active release plus the newest inactive release

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
