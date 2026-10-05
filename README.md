# linklet

A small, honest tool for driving machines on a LAN, built to be called by an AI
agent rather than by a person reading a manual.

> **Status: M0-M7 done, both defects M10 found are fixed, the deploy loop can be closed
> from an agent, a target's files can be looked at without moving them, the machines on a
> network can be found without being told where they are, one command can be run across
> several of them, and an agent that dies or wedges is restarted.** Five crates, 544 tests,
> one command that runs every gate. A host can check reachability, run a command on a target
> through a sealed channel, read what it did, see what is running there, start something that
> outlives the call, stop it again, list and search a directory on the target, and move one
> file in either direction. See `docs/ROADMAP.md` for what is next and what was parked, and
> `docs/decisions.md` for the choices that are not obvious from the code.

## What it does

```console
$ linklet check 10.0.0.5:8787,10.0.0.6:8787
live 10.0.0.5:8787 connected
dead 10.0.0.6:8787 the machine refused the connection
$ echo $?
1
```

```console
$ export LINKLET_TOKEN=$(some-secret-of-at-least-16-bytes)
$ linklet exec --agent 10.0.0.5:8787 "build.cmd --release"
exit 0
took 2411 ms
stdout:
built 3 targets
$ echo $?
0
```

```console
$ linklet ps --agent 10.0.0.5:8787 --name app.exe
5144 app.exe
1 of 214 match, filter name=app.exe
$ echo $?
0
```

```console
$ linklet push --agent 10.0.0.5:8787 --from dist/app.exe --to app.exe
app.exe: 4194304 bytes, sha256 9f86d081884c7d65...
$ linklet pull --agent 10.0.0.5:8787 --from build.log --to build.log
build.log: 18244 bytes, sha256 2c26b46b68ffc68f...
```

A transfer goes to the directory the agent was started in, or the one it was given with
`--root`, and nowhere else -- in either direction. The digest on the line is the one the
receiving side computed, so a caller can check it against the file it sent or the file
it now has.

**A path that would leave that directory is refused by name, and what counts as leaving it
is a fact about the target's filesystem.** Use `/` as the separator: it is the one that means
the same on both platforms, where a backslash is a separator on Windows and an ordinary
character elsewhere -- so on a Linux target a backslash is refused rather than quietly
written as part of a filename. Names Windows cannot hold are ordinary on Linux (`NUL`,
`a:stream`, a trailing dot) and are accepted there; the ones Windows would resolve to a
*different file* -- a stream, a device, a name whose trailing dot it strips -- are refused
there. `docs/transfer.md` T1 has the table and the reason each rule exists.

`ps` prints one line per process, `pid name`, then a summary. **The summary is the
point**: `0 of 214 match, filter name=app.exe` cannot be read as a clean machine, and an
agent that treats an empty list as one will deploy over a running binary. For the same
reason an incomplete listing -- one that could not read the machine, or could not check
a field the caller filtered on -- exits 1 and says so in a note.

```console
$ linklet kill --agent 10.0.0.5:8787 --pid 5144
killed 1 of 1
5144 app.exe
$ linklet kill --agent 10.0.0.5:8787 --name app.exe --yes
killed 2 of 2
5144 app.exe
5150 app-helper.exe
```

`kill` takes a pid, an exact name, or a substring. **A name needs `--yes`** because it can
match more than one process, and a pid never does because a number is one process. It will
not stop the agent that is serving it, and that refusal comes from the target rather than
from here: filtering it out locally would report success on everything else while the one
process the caller named kept running. `killed 0 of 1` is exit 1 -- the machine did not do
what was asked.

```console
$ linklet spawn --agent 10.0.0.5:8787 --output logs\app.log "app.exe --serve"
started 5144
$ linklet ps --agent 10.0.0.5:8787 --name app.exe
5144 app.exe
1 of 214 match, filter name=app.exe
```

`spawn` is not `exec` with a flag. **`exec` waits** for the command and returns its output,
which is right for a build step and wrong for a program meant to keep running: the request,
the connection and the agent's pipes are held until it exits. `spawn` starts the program with
**its own output file** and returns a pid at once -- and `started` is the word on the line
because that is all this side knows. Whether it is still there is the `ps` you run next,
which is the order the deploy loop actually goes in: start, look, stop if you have to.

```console
$ linklet grep --agent 10.0.0.5:8787 --from build.log --pattern ERROR --last --context 1
2 matches in build.log, read as utf-8, 18244 bytes
  compiling
41: ERROR: expected ';' at line 12
  warning: unused import
$ linklet tail --agent 10.0.0.5:8787 --from build.log --lines 3
more than 3 lines in build.log, read as utf-8, stopped early, 18244 bytes
42: done
```

`grep` and `tail` read a file **on the target and return only the answer**, which is the
difference between them and a `pull`: a two-gigabyte log whose last ERROR is the question
does not have to cross the network to answer it. The first line of both is a summary, and
it is always printed, because **a file that could not be read must never look like a file
with no matches**:

```console
$ linklet grep --agent 10.0.0.5:8787 --from gone.log --pattern ERROR
could not search gone.log: cannot read gone.log: the system cannot find the file specified
$ echo $?
1
```

Exit 1 means the answer is incomplete -- the search stopped early, the file was cut short at
the byte ceiling, or it could not be read at all. 0 means the whole file was looked at. The
encoding is in the summary too, so a reader shown mojibake knows which rule produced it: on
Windows the machine's code page, and elsewhere ISO-8859-1, which is one character per byte
and loses nothing. **A label is never a guess about the bytes** -- it names the rule that was
applied, and the two machines apply different ones.

**The pattern is a substring.** `ERROR|FATAL` does not work; `--pattern ERROR` does.
`docs/ROADMAP.md` records that as a difference from the reference implementation rather
than as an equivalent.

```console
$ linklet ls --agent 10.0.0.5:8787 --from .
3 of 3 entries in .
archive/
logs/
build.log 18244 bytes
```

`ls` answers the question every other command assumes: **is this path there, and what is
beside it.** It prints `name/` for a directory and `name size` for a file, then the same
kind of summary `ps` prints. An empty directory says `0 of 0 entries`; a path that is not
there says `could not list` and exits 1 -- the two are the same list and opposite facts, and
only one of them means the machine has no logs.

```console
$ linklet discover --port 8790
1 of 1530 addresses answered on port 8790
192.168.100.2
networks: 192.168.100.1/255.255.255.0 192.168.3.157/255.255.255.0 172.18.112.1/255.255.240.0
skipped: 192.168.100.1 (this machine's own address)
skipped: 192.168.3.1 (the default gateway, which was not asked to be part of this)
note: the scan was cut short at its ceiling, so addresses beyond it were not tried
$ linklet discover --port 8790 --targets
192.168.100.2:8790
```

`discover` finds the machines on the networks this host is on, without being told where they
are. It prints the summary first and the addresses after it, because a scan has a ceiling
and **"nothing answered" and "I tried a thousand of this network's four thousand addresses"
are different facts**. `--networks` prints the networks without scanning; `--targets` prints
the addresses as a `host:port` list, which is what every other command already takes -- so
discovery feeds the commands it exists for. Exit 1 means the scan was cut short and the
answer is therefore incomplete.

It scans about 1600 addresses at most, never this machine's own address and never the
default gateway, and it names both in the output rather than silently skipping them.

```console
$ linklet exec --agents 10.0.0.5:8787,10.0.0.6:8787,10.0.0.7:8787 "build.cmd --release"
1 of 3 ran
ran 10.0.0.5:8787
  exit 0
  took 2411 ms
  stdout:
  built 3 targets
unreachable 10.0.0.6:8787
refused 10.0.0.7:8787
```

`exec --agents` runs one command across several machines. It is a **separate flag from
`--agent`** because the exit codes differ: one target's status is passed through, and a run
over several has no single status to pass through. Each machine gets a block labelled with its
target, in the order you listed them however they finish, and one machine failing does not stop
the others. **`refused` and `unreachable` are different words** -- one sends you to the build,
the other to the network.

Exit 0 means every agent ran the command. A command that ran and exited 7 is still exit 0:
that is the command's business, and whether the agents answered is this tool's.

`check` prints one line per target, in the order the targets were given: `live`,
`dead` or `unknown`, then the target as it was written, then the reason. `dead`
covers two situations and the reason is where they are told apart, because the
difference matters to whoever is reading:

| output | what it means |
|---|---|
| `the machine refused the connection` | the machine is up; nothing is listening on that port |
| `no answer within 5 s` | nothing came back at all -- off, filtered, or slow |

`exec` runs one command on one machine and reports its exit code, how long it
took, and what it printed. It needs an agent on the target and a shared token.

### Exit codes

| code | meaning |
|---|---|
| 0 | the run completed and the answer is the whole truth -- everything alive, the command exited 0, or the listing was complete |
| 1 | the run completed and something is not: a target is down, the command failed, or a listing could not be read completely |
| 2 | the invocation was wrong |
| 3 | the run was refused before anything was looked at |

For `exec`, the command's own exit code is passed through when the command ran,
so `linklet exec ... && next` behaves the way the command would. A call that could
not be made gets 3, which no command can produce. `ps` follows `check` rather than
`exec`: there is no command whose status could be passed through, and **1 means the
call was made and the answer is incomplete** -- a listing the machine could not
finish is a fact about the machine, and 3 would send the reader to the network.

The distinction between 1 and 3 is the one an agent needs: "the machines are down"
and "the tool could not start" send it to different places, and collapsing them
into one non-zero code loses exactly the information it came for.

## Calling it from an agent

`linklet mcp` speaks the Model Context Protocol on stdin and stdout, so an agent can
call this tool directly. No port and no configuration to keep in step: the client
starts the process and talks to it.

```json
{
  "command": "C:\\linklet\\linklet.exe",
  "args": ["mcp"],
  "env": { "LINKLET_TOKEN_FILE": "C:\\linklet\\token.txt" }
}
```

Those three fields are the whole installation, and which file the entry goes in is the
client's business: `integrations/` has the entry to paste for DSH and the same three
fields for ZCode, with the skill that goes beside them. **`command` names the
executable and not a shell command**: an MCP client spawns it without a shell, which is
one thing a native `.exe` does not need a wrapper for.

Eleven tools, one per intent: `check`, `testbed`, `exec`, `ps`, `kill`, `spawn`,
`grep`, `tail`, `ls`, `push` and `pull`. They are the operations the commands above
perform, so a reader who knows one knows the other.

**The token comes from the environment, or from the file that variable names, and
never from a tool argument** -- an argument would put the secret in the
conversation. `LINKLET_TOKEN_FILE` is the field to prefer here, because
`LINKLET_TOKEN` in the same place writes the secret into a file that gets copied,
shared and committed. `linklet probe --agent <host:port>` is the check to run before
wiring a client up: it completes a handshake, so it answers whether the token and the
agent agree.

`docs/MCP.md` is the surface as its maintainer sees it: why each tool is separate,
and the two places a caller goes wrong. `integrations/` is in this repository, and a
release is one archive -- `linklet-<version>.zip`, with the executables under `bin/`
and this installation material beside them -- so an installation does not need a
clone.

## The problem

Driving a remote machine during debugging means a long chain of small,
error-prone steps: work out which machines are reachable, copy a build over, start
it, notice it did not start, look at a log, kill what is left, copy the log back.
Every one of those steps is a place where a guess is made and never checked, and
the failure that results is quiet.

This tool makes each step explicit: it takes a description of targets, returns
typed answers, and refuses to answer a question it did not actually ask the
machine.

## What it does not do

A boundary is part of the design, and an unstated boundary gets crossed by
accident. This tool:

- does not install an agent on a target by itself (the first copy has to be a file
  copy; there is nothing to talk to yet)
- does not keep a database, a service, or a daemon on the host
- **drives Windows and Linux targets, in both directions, verified between two real
  machines.** Every command works on either: the protocol, the sealed channel, both transfer
  directions, `exec`, `ps`, `kill`, `spawn`, `ls`, `grep`, `tail`, `check`, `probe`,
  `discover` and `testbed` -- a Windows host driving a Linux agent and a Linux host driving a
  Windows one, including the deploy loop (look, start, confirm it stayed up, stop it, confirm
  it is gone). **The one real difference is the quality of one answer**: a file that is not
  UTF-8 is read on Windows with the machine's code page, which recovers the characters, and
  elsewhere with ISO-8859-1, which is total and lossless but does not -- so the mojibake there
  is labelled rather than decoded. `docs/ROADMAP.md` M11 says why- does not guess: when it cannot determine something, it returns `unknown` with
  the reason, never a plausible default
- **has no service on the target, and no supervisor either.** An agent that dies stays
  dead, and one that has wedged is not noticed, until somebody asks: `linklet probe`
  answers whether an agent is *working* rather than only *listening*, and the caller that
  drives this tool is the thing that runs it. Making the agent a service is the platform's
  job -- `docs/smoke.md` has the scheduled-task form and what it does and does not cover
- **has no identities.** The token authenticates the channel and says nothing about
  *which* caller it is, so there is no per-caller revocation and no audit trail
- **has no cipher agility.** One curve, one cipher, one key derivation, chosen at
  build time
- **has no CI on a second machine.** `.github/workflows/verify.yml` runs the same four
  gates on Windows and on Linux for every push -- one job, two runners -- which is what
  `tools/verify.ps1` was written as the single entry point for. **Nothing automated has
  ever run against another machine** -- that needs one, so it cannot be a gate.
  `docs/testing.md` is the argument and `docs/smoke.md` is what that layer claims

## The one architectural rule

**The core is pure. The edges are thin.**

```
linklet-cli        argv in, text out, process exit code          (thin)
linklet-adapters   the network, the OS, the crypto               (thin)
linklet-core       decisions: parsing, validation, policy        (pure, no I/O)
      ^
linklet-agent      the target side     linklet-client   the host side
```

The rule pays for itself in exactly one way, and it is the one that matters:
**every decision in `core` can be tested in microseconds with no network, no temp
files, and no cleanup.** When a test needs a real machine to run, it stops being
run, and then the behaviour it covered rots.

The rule was originally written as "core depends on no crate", and that was the
wrong wording -- it is satisfied by a rule rather than a reason, so it stayed in
force past the point where its reason applied, and a SHA-256 came to be written by
hand because of it. The rule is now what it always meant: **core depends on no
crate that does I/O**, enforced by a named allowlist in
`crates/linklet-core/tests/architecture.rs` where every entry carries a sentence
saying why. `docs/decisions.md` has the whole of it.

That is also how the cryptography is arranged. `linklet-core` declares what a
sealed conversation is (`src/channel.rs`); `linklet-adapters` implements it with
`chacha20poly1305`, `hkdf`, `sha2` and `x25519-dalek`. The core still cannot open a
socket or a cipher, and the arithmetic is in crates other people have attacked.

### What the channel does

A sealed call is **a handshake and then the message it protects**, on one connection:
the hello, then the command sealed under the session it produced. The handshake cannot
protect itself -- the initiator cannot derive a key until it has the responder's public
key -- so the exchange comes first and the command follows it. The alternative, one
round trip with the command sealed under the token alone, leaves the command readable by
anyone who later learns the token, which is the wrong half to protect.

A transfer is the same beginning and then a stream: a manifest carrying the size and the
digest, then one sealed chunk per mebibyte. `docs/transfer.md` is the design and the
thirteen ways it goes wrong.

Both sides generate an X25519 key pair per handshake and **discard the private
half**, so a session recorded today cannot be read by anyone who learns the token
tomorrow. The token still authenticates the exchange: an attacker who substitutes
their own public key can complete a handshake and still cannot open a byte,
because the session they build is not the one either end built.
`crates/linklet-adapters/tests/handshake.rs` demonstrates that rather than
asserting it.

## Working on it

```sh
pwsh tools/verify.ps1           # fmt, clippy, test, rustdoc, dependency inventory
```

One entry point, because a gate nobody runs is a gate that does not exist. It runs
`cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, `cargo doc` with
`-D warnings`, and a dependency inventory that prints which crate depends on what.

Read `AGENTS.md` before your first commit -- it is the rules only, one screen long.
`docs/INDEX.md` is the map of everything else.

### Where things are

| | |
|---|---|
| `crates/linklet-core/` | the decisions. 22 test files, all pure |
| `crates/linklet-adapters/` | sockets and crypto: the only place either happens |
| `crates/linklet-agent/` | the target side: one binary, binds a port, runs commands |
| `crates/linklet-client/` | the host side of the protocol |
| `crates/linklet-cli/` | argv in, text out, exit code |
| `integrations/` | installing the MCP server in a client, and the skill an agent drives a machine by |
| `docs/` | see `docs/INDEX.md`, which says what kind of document each one is |
