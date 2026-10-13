# Protocol 3 operation evaluation

## Conditions

The deterministic scenarios in `crates/linklet-cli/tests/execution.rs` use real
CLI/MCP subprocesses and authenticated loopback agents. A compiled Rust helper
exercises literal arguments, closed stdin, per-call directories, process exits,
timeouts and concurrent stdout/stderr. Definitions come from the actual tools
list, and replies are validated with the jsonschema test dependency.

These tests measure protocol behavior and fixed call counts. No new experiment
with an independently running language model was performed. They do not establish
a model accuracy rate, token-cost improvement or LAN behavior. The historical
readability experiment has different conditions and is not a protocol 3 baseline.

## Representative local measurements

Observed runs used Windows under its ordinary account and WSL Ubuntu as `ww`,
with development binaries and one tool call per scenario:

| Scenario | Tool calls | Windows bytes | WSL bytes | Incorrect status assertions |
|---|---:|---:|---:|---:|
| Authenticated context | 1 | 791 | 723 | 0 |
| Start and observe a finite helper | 1 | 907 | 745 | 0 |
| First log page | 1 | 1287 | 1086 | 0 |
| Incremental page with no new bytes | 1 | 1215 | 1049 | 0 |
| Empty process query | 1 | 685 | 683 | 0 |
| Stop an absent fixture name | 1 | 308 | 308 | 0 |
| Two-target dual-stream output | 1 | 34251 | 34181 | 0 |

The published 14-tool definitions measured 42396 compact JSON bytes. Result
measurements include status and metadata but exclude the outer JSON-RPC envelope.
The dual-stream case writes 17 MiB plus a marker to each stream on each target;
its content is exactly 32768 bytes, including the short text summary. Serialized
results may exceed 32 KiB because the budget covers content rather than metadata
or JSON escaping. Paths, PIDs, counts and timings make exact sizes vary by run.

Pair returns the authenticated context in its encrypted completion, eliminating
a follow-up probe solely to retrieve that context. Spawn returns its initial log
tail and exit observation in one call. A later application readiness check remains
a separate requirement when readiness matters. These are interface facts, not
measured improvements in a model's planning behavior.

## Verification coverage

The suite covers invalid modes, quoted/empty/Unicode arguments, concurrent cwd,
Windows batch rejection, per-target failures and uncertainty, dropped upload
confirmation, output budgets, partial process information, log pagination,
rotation, truncation, boundary edits, split BOMs and partial characters.

Permission tests observe the effective account and read a mode-000 file on Linux.
The ordinary account must receive permission_denied; the root process must read
it. Neither result is inferred from sudo configuration.

Windows and WSL verification records are summarized in docs/VERIFYING.md. To
reproduce measurement lines, build the workspace and run:

```sh
cargo test -p linklet-cli --test execution -- --nocapture
cargo test -p linklet-adapters --test process_permissions -- --nocapture
```
