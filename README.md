# codex-cache

Inspect and safely manage the Codex standalone release cache.

## Features

- `scan` — inspect the installed standalone cache
- `report` — summarize versions, cache size, and reclaim estimates
- `list` — list installed releases and their sizes
- `verify` — validate the cache layout and detect inconsistencies
- `clean --dry-run` — preview which releases would be removed

## Safety

Cleanup is currently limited to dry-run planning.
No deletion functionality has been implemented yet.

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
codex-cache clean --dry-run --keep current,previous
```

## Status

Inspection, verification, and cleanup planning are implemented.
Safe deletion will be added in a future release.
