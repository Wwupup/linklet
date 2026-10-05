# The workflows

What runs on GitHub, what it is allowed to claim, and what has to be checked by hand. Three
documents already say *what is true* about this project -- `README.md`, `docs/ROADMAP.md` and
`docs/INDEX.md` -- and none of them says how the thing is actually built and published. That is
this file, and it is the one a person reads before changing `.github/workflows/`.

It is written the way everything else here is written: each decision with the cost of the
alternative, because most of them have already been paid for once.

**There are two workflows, and they do different jobs.** `verify.yml` runs the gates on every push
and answers "is this commit good". `release.yml` runs on a tag and answers "what does a user
download". Rule 3 of `AGENTS.md` -- the definition of done -- is `tools/verify.ps1`, and **both
workflows call that script rather than restating its four commands**. The script's own header
explains why that matters more than the four lines it saves.

## `verify.yml` -- every push

One job, **two runners**, `windows-latest` and `ubuntu-latest`, one step that runs
`pwsh tools/verify.ps1`.

- **Both platforms because the adapter layer has two halves.** A Windows run exercises
  `tasklist`, `taskkill`, `ipconfig` and `route`; a Linux run exercises `/proc`, `ip` and `sh`.
  Neither is visible from the other, and `docs/testing.md` records what the second platform has
  already found: a path that escaped the transfer root on a POSIX filesystem, and a clippy failure
  that had never been looked for there.
- **A matrix over one job, not two jobs with the same steps**, for the reason the script exists:
  one place the gates are written down.
- **`fail-fast: false`.** The two legs fail for different reasons, and cancelling one destroys the
  answer about the other.
- **`pwsh`, not `bash`.** PowerShell 7 is preinstalled on the Ubuntu image, so the same script runs
  on both. A second shell script for one platform would be that two-lists mistake again.
- **`RUSTFLAGS: -D warnings`** at the workflow level as well as `-D warnings` on clippy, so cargo is
  as strict about its own deprecations as the linter is about the code.
- **`timeout-minutes: 20`.** The suite takes about ten minutes locally and every test spawns a real
  agent process; twenty is where something is wrong rather than slow.

## `release.yml` -- a tag

Three jobs, because a release is one file and that file cannot be built in one place.

| job | where | what it does |
|---|---|---|
| `build (x86_64-pc-windows-msvc)` | `windows-latest` | the tag/manifest check, `tools/verify.ps1`, `cargo build --release --locked`, upload the two `.exe` |
| `build (x86_64-unknown-linux-gnu)` | `ubuntu-latest` | the same, uploading the two binaries |
| `assemble and attach` | `ubuntu-latest` | download both artifacts, `tools/make_release.sh`, `gh release create` |

**Each binary is built by the platform it runs on.** Cross-compiling one from the other would mean
shipping something that was never run on the operating system it is for, and the gates that ran on
that leg would be about the other machine.

**The assembling job runs on Linux, and the reason is not symmetry.** A zip records the Unix mode
of every entry. `Compress-Archive` writes `external_attr = 0` for every one of them; measured, it
leaves a Linux binary on disk at mode **600** -- not executable, and not readable by anyone else.
Info-ZIP `zip` writes the mode, so a zip made by `zip` comes back **755 / 644**. The archive is
therefore assembled where that tool is, and `tools/make_release.sh` extracts its own output and
checks the bit rather than trusting the staging step. `crates/linklet-core/tests/ci_workflow.rs`
fails if `Compress-Archive` comes back or if the assembling job stops running on the image whose
`zip` does this.

**`tools/make_release.sh` is to that job what `tools/verify.ps1` is to the gate job**: the logic in
one file, runnable by a person against a checkout, called by CI rather than restated in YAML. Run it
by hand with

```sh
bash tools/make_release.sh <version> <binaries-dir> <out-dir> <owner/repo>
```

where `<binaries-dir>` holds one directory per target triple -- which is exactly what
`actions/download-artifact` produces, so the handover format and the person's local layout are the
same thing.

**What the script checks before it will produce anything**, each because the alternative is
publishing something quietly wrong:

1. Both triples are present, each with both executables. An archive that holds one platform is the
   failure the whole arrangement exists to prevent.
2. Every payload item is in the tag -- checked **before** anything is staged, because a package that
   lost the skill would still publish and would only be missed by whoever tried to follow it.
3. The finished archive holds every path it meant to, at the top level it meant to.
4. The Linux executables are **executable after extraction**.

**One archive, `linklet-<version>.zip`**, holding `bin/<target-triple>/` for both platforms plus the
payload that is copied out of the tag: `README.md`, `CHANGELOG.md`, `LICENSE`, `docs/MCP.md`,
`docs/machine.md`, `docs/smoke.md` and `integrations/`. `SHA256SUMS` sits beside the archive and
never inside it -- a digest inside the thing it verifies cannot be used to verify it. Release notes
are generated from a template in the script and **point at** `CHANGELOG.md` rather than copying it,
because a second copy is a second thing to keep in step.

**The payload list lives in the script and not in the workflow**, and a test fails if that stops
being true. It used to be written in `release.yml`; two lists of what a release carries is two lists
that can drift, and a release is the worst place to find out which one won.

**`--verify-tag`** on `gh release create`, so the job cannot invent a tag that was not pushed.

## What no workflow can do

**It cannot decide that a release is ready.** Steps 1 to 5 of `docs/VERSIONING.md` are a person
running the gates and driving a real machine, and the second one needs a target on a network. A
runner has none. `docs/smoke.md` is that layer: seven claims, made by hand or by an agent.

**It cannot install anything.** The archive is files. Putting an agent on a target is a file copy a
person makes once.

**It cannot check its own pins.** One class of failure is invisible locally: an action whose runtime
GitHub has deprecated. `actionlint` flags `actions/checkout@v3` and did **not** flag `@v4` when
Node 20 began being retired -- its staleness threshold lags GitHub's -- so that arrives as an email
after a push. The pins are guarded two ways instead:

1. `crates/linklet-core/tests/ci_workflow.rs` fails on a branch pin. `@master` moves under a green
   build; `@vN` or a SHA does not.
2. `docs/VERSIONING.md` step 8: read each action's own `action.yml` before a release and check its
   `runs.using`. **`node24` is current**; anything older is a deprecation warning waiting to arrive.

**This machine cannot see GitHub's own answers at all.** There is no `act` and no Docker here, so a
workflow can be linted and its shell parsed locally and nothing more -- `tools/verify.ps1` runs
`actionlint` when it is on `PATH`, and that is the limit. Everything else is learned from a push.
`docs/machine.md` has the rest of what this machine does to a command.

## Changing a workflow

1. `tools/verify.ps1`, which lints the workflows when `actionlint` is installed.
2. `cargo test -p linklet-core --test ci_workflow`, which holds the claims above.
3. If a new action is added: read its `action.yml` and check `runs.using`.
4. **A workflow change cannot be tested by reading it.** Push it and read the run -- that is what
   `docs/VERSIONING.md`'s list of what to check before pushing is for.
