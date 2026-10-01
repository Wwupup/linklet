# Versions, and what a release is

What a version number means here, which changes raise it, and what has to happen before one is
cut. The point of writing it down is that "should this be 0.3.0 or 0.3.1?" has an answer that
does not depend on who is asking.

**This tool is a set, not a library.** Five crates, one workspace version, and the two binaries
that matter -- `linklet` and `linklet-agent` -- are built from the same commit and are meant to
be deployed as a pair. Nothing here is published to a registry, and no crate is versioned
independently: a rule that let `linklet-core` be 0.4 while `linklet-agent` was 0.3 would create
a compatibility matrix for a tool that has exactly one deployment at a time.

## What the number means

`0.x.y`, and while the major is 0 the minor is the interesting one.

| change | version | example |
|---|---|---|
| a new command, a new tool, a new field a peer may ignore | **minor** | `0.2.0` added `push`, `ps`, `discover` |
| a fix that changes no interface | **patch** | an encoding reported wrongly |
| **a change to the wire that an old peer would answer wrongly** | **minor, and the protocol number goes up** | reusing a field, changing a field's meaning, changing the framing |
| a change that breaks the command line or the MCP surface | **minor** | `--agent` no longer accepting a list |

**Adding an operation does not raise the protocol number.** An agent that does not know an `op`
refuses it by name, listing the ones it does know -- that is an answer, and an old agent
answering some requests correctly is a working deployment. Raising the number is for the
changes where a peer would *do something wrong while believing it understood*.

## The protocol number, and the one rule

`wire::PROTOCOL_VERSION` is a small whole number in the same workspace as everything else, and
it travels in the handshake. The rule is:

> **Only ever add. Never reuse a field, never change what a field means, and never change the
> framing -- unless the protocol number goes up in the same commit.**

Where it comes from, for present-day numbers:

- **Protocol 1 is every build before the field existed.** That is why the first number is 1 and
  not 0: a handshake with no `protocol` key is a peer that predates the key, and
  `wire::OLDEST_PROTOCOL` is what absence means. Calling it 0 would put every already-deployed
  agent outside the range of things the scheme can talk about.
- **Absence is not an error.** An old agent sends a perfectly good handshake that is short one
  key, and it is read as the oldest protocol rather than refused as malformed. See
  `crates/linklet-client/tests/version_skew.rs`, which pins both this and the refusal.

**A mismatch is refused at the handshake**, with both numbers named and a direction: which end
is older, and therefore which one to replace. It is refused rather than tolerated because a
host that cannot use what it is talking to should say so once, at the start, rather than let a
caller discover it from whichever operation happens to need a newer feature. `docs/decisions.md`
D2 records what the release before this one cost, which was nothing at all.

**The upgrade order is therefore: the agent first.** A new agent serves an old host, so having
the target ahead of the tool is the state that always works; a host ahead of its agents is the
state that gets refused, with a sentence saying so.

## Cutting a release

**CI runs `tools/verify.ps1` on every push, and that is all it can run.**
`.github/workflows/verify.yml` calls `tools/verify.ps1` on Windows. What no workflow can do
is step 2 below, because it needs a machine on a network -- so this list is still a list a
person works through, with one step already done for them.

Tagging runs `.github/workflows/release.yml`, which builds the binaries, writes a
`SHA256SUMS` beside them, and attaches both to the release. **It refuses a tag that disagrees
with the version in `Cargo.toml`**, which is the failure this project has already had once:
a release was called 0.2.0 while every binary still reported 0.1.0.

**Every step is a person running one command.**

1. `pwsh tools/verify.ps1` -- every gate. Nothing is committed red and nothing is released
   red; this is the same command that guards a commit, and the same one CI runs.
2. **Drive a real machine.** `pwsh tools/smoke.ps1 -Target <host:port>`, plus the commands the
   release added, by hand. `docs/smoke.md` is what that claim covers and what it does not. **A
   release that was never run against a second machine is a release whose every claim is
   untested outside this host** -- that is a decision a person makes, not one this document makes
   for them.
3. **Write the version into `CHANGELOG.md`.** Move what is under `[Unreleased]` into a new
   section named for the version, state the date, and leave `[Unreleased]` empty above it.
   `[0.2.0]` is the worked example: what a user can do now that they could not before, plus the
   fixes, plus **what is still open**.
4. **Bump the version in `Cargo.toml`** -- one edit, in `[workspace.package]`. Every crate
   inherits it with `version = { workspace = true }`, and `SERVER_VERSION` derives from it, so
   there is no second place to forget. **`crates/linklet-core/tests/architecture.rs` fails if
   any crate pins its own version**, because the first attempt at this release bumped the
   workspace and changed nothing: all five crates carried `version = "0.1.0"` of their own, so
   the MCP server went on introducing itself as 0.1.0 and the agent went on reporting 0.1.0 to
   every host. One number in six places is five places to forget, and the forgotten one is the
   one that reaches a shipped binary.
5. `pwsh tools/verify.ps1` again, because a version bump is a change.
6. **Commit, then tag: `git tag -a v0.2.0 -m "..."`.** The tag is what makes a release
   findable. Nothing in this repository writes version numbers into strings for the tag to
   disagree with; the tag is the record of which commit was released.
7. **Build and keep the binaries.** `cargo build --release`, and the two `.exe` files go
   wherever they are distributed from. `git` does not hold build output and must not --
   `docs/rationale.md` says why. The tag is the source, and the binaries are a function of it.
8. **Read the action pins.** For each `uses:` in `.github/workflows/`, fetch that action's
   `action.yml` and check its `runs.using`. `node24` is current and `node20` is a deprecation
   warning waiting to arrive as an email. No local tool reports this -- see below.

## The workflows, and what can be checked before pushing

`tools/verify.ps1` runs `actionlint` when it is on `PATH` and **says so when it is not**: it is
not a Rust tool, and nobody should have to install it to build this. To get it:

```powershell
# The Windows binary, digest-verified against the release's own checksums.txt.
Invoke-WebRequest https://github.com/rhysd/actionlint/releases/download/v1.7.12/actionlint_1.7.12_windows_amd64.zip -OutFile "$env:TEMP\actionlint.zip"
Expand-Archive "$env:TEMP\actionlint.zip" -DestinationPath "$env:USERPROFILE\.local\bin" -Force
actionlint --version
```

**What it catches**: a misspelled `github.*` property, an action whose inputs do not exist, a
`run:` body whose shell script does not parse, a `needs:` that names no job.

**What it does not catch, and this was measured against this repository's own workflows**: it
knows about action runtimes and flags `actions/checkout@v3` as *"too old to run on GitHub
Actions"*, but it did **not** flag `@v4` when GitHub began retiring Node 20 -- its staleness
threshold lags theirs. That failure arrived as an email after a push, which is why step 8 above
exists.

So the pins are guarded two ways and neither of them is the linter:

1. **`crates/linklet-core/tests/ci_workflow.rs` fails on a branch pin.** `@master` moves under a
   green build; `@vN` or a SHA does not. `dtolnay/rust-toolchain@master` was in the first
   version of these workflows and is now `@v1`.
2. **The release checklist reads the pins** (step 8). The runtime is GitHub's and not this
   machine's, so there is nothing local to ask; `action.yml` is the answer and it is one line
   to read.

A major tag is the deliberate choice over a commit SHA: a SHA is the most reproducible and the
least maintainable, and the maintainability half is what bit.

## Rolling back

**Check out the old tag and build it.** That is the whole procedure, and it works because the
tag is a commit and the binaries are a function of it. There is no migration to reverse and no
state on the target that a version writes:

- The agent keeps a log file and a transfer root. Both are the operator's and neither has a
  format a version owns.
- Nothing on the target records which tool talked to it.

A target running a newer agent than the tool is the safe direction, per the upgrade order above,
so a rollback of the *tool* alone leaves the deployment working.

## What this deliberately does not have

- **No published crates.** Nothing here is a library anyone depends on, and publishing five
  crates would create a compatibility surface for a tool with one deployment at a time.
- **No release matrix.** The support statement is one sentence: **the agent and the tool should
  be from the same release, and the agent may be ahead.** A matrix of "host 0.3 supports agent
  0.2 and above" needs back-versioned tests that this repository has no machine to run.
- **No conventional-commits automation.** `docs/COMMITS.md` fixes the commit format and
  `crates/linklet-core/tests/commit_message.rs` enforces it, so a generated changelog is
  possible. It is refused on purpose: a generated changelog tells a user which internal function
  moved, and `CHANGELOG.md` exists to answer one question -- what can I do now that I could not
  do before.
