# linklet

A small, honest tool for driving machines on a LAN, built to be called by an AI
agent rather than by a person reading a manual.

> **Status: M0-M7 done, both defects M10 found are fixed, the deploy loop can be closed
> from an agent, a target's files can be looked at without moving them, the machines on a
> network can be found without being told where they are, one command can be run across
> several of them. The agent is an ordinary process and stays offline after exit.** Five crates,
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
$ linklet pair --agent 10.0.0.5:8787 --code 083614
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
app.exe: 4194304 bytes, sha256 9f86d081884c7d65... (verified)
$ linklet pull --agent 10.0.0.5:8787 --from build.log --to build.log
build.log: 18244 bytes, sha256 2c26b46b68ffc68f... (verified)
```

File tools can use any ordinary path the agent account can access. Absolute paths are
used directly, including other Windows drives and ordinary UNC shares. Relative paths,
including `..`, start from the directory the agent was started in. There is no
`--root` option. To choose the base for relative paths, set the working directory before
starting the agent; it does not limit access to that directory.

The receiving side checks the digest before the file goes under its real name.
`(verified)` means that comparison passed; a mismatch refuses the transfer and prints
no success line. A second hash is unnecessary, especially for a log that is still changing.

Use `/` as a portable separator. Empty paths and NUL bytes are refused. On Windows,
device names, device namespaces, alternate streams, drive-relative paths such as
`C:file.txt`, and names ending in a dot or space remain unsupported. POSIX refuses
backslashes to catch Windows-shaped paths. File-type checks and transfer limits still
apply; `docs/transfer.md` T1 describes them. Like `exec`, file tools use the agent's
existing OS permissions and do not elevate them.

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

`exec` returns the last 1 MiB of raw bytes from each output stream, with total and
retained byte counts, encoding, truncation, and decoding-loss metadata. Larger
output is drained and discarded; truncation does not change the exit code.
Unicode is detected first; Windows command output falls back to its console
code page (system OEM without a console). File reads fall back to system ANSI.
Both binaries must use protocol 2; upgrade both ends together.

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
discovery feeds the commands it exists for. Results establish TCP reachability,
not authentication; use `probe` to verify an agent. Exit 1 means the scan was cut short and the
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

Exit 0 means every command exited 0. Any command failure, timeout, or failed call
returns 1 for the batch. Each target keeps its real exit code and failure reason.
Batches accept at most 256 targets, run four at once, and preserve input order.

`check` prints one line per target, in the order the targets were given: `live`,
`dead` or `unknown`, then the target as it was written, then the reason. `dead`
covers two situations and the reason is where they are told apart, because the
difference matters to whoever is reading:

| output | what it means |
|---|---|
| `the machine refused the connection` | the machine is up; nothing is listening on that port |
| `no answer within 5 s` | nothing came back at all -- off, filtered, or slow |

`exec` runs one command on one machine and reports its exit code, how long it
took, and what it printed. It needs a running target agent and a saved pairing credential.

### Exit codes

| code | meaning |
|---|---|
| 0 | the run completed and the answer is the whole truth -- everything alive, the command exited 0, or the listing was complete |
| 1 | the run completed and something is not: a target is down, the command failed, or a listing could not be read completely |
| 2 | the invocation was wrong |
| 3 | the run was refused before anything was looked at |

For `exec`, the command's own exit code is passed through when the command ran,
so `linklet exec ... && next` behaves the way the command would. A call that could
not be made gets 3. A remote command can also exit 3; the text distinguishes it. `ps` follows `check` rather than
`exec`: there is no command whose status could be passed through, and **1 means the
call was made and the answer is incomplete** -- a listing the machine could not
finish is a fact about the machine, and 3 would send the reader to the network.

The distinction between 1 and 3 is the one an agent needs: "the machines are down"
and "the tool could not start" send it to different places, and collapsing them
into one non-zero code loses exactly the information it came for.

All commands support `-h/--help` and both binaries support `--version`; these exit 0.
For `exec` and `spawn`, put Linklet options before the command or an explicit `--`.
Remote `--help` and other options pass through once the command starts.

Probe exit codes: 0 authenticated identity (name and version), 1 connection failed,
2 arguments or local token configuration, 3 authentication or protocol rejected,
4 timeout or invalid reply after connecting. Probe does not restart anything.

## Starting the target process

On Windows, open a terminal as the account whose permissions the agent should use:

```powershell
Set-Location C:\work
C:\linklet\linklet-agent.exe
```

On Linux:

```sh
cd /srv/work
/opt/linklet/linklet-agent
```

The startup directory is the base for relative paths. Absolute paths and parent
components use the process account's normal filesystem access. Choose an elevated
terminal only when the intended operations require that account's privileges.
The process is available while running and offline after it exits. No installation,
autostart, supervision, or automatic restart is performed. Allow the selected TCP
port (default 8787) through the target firewall.

First start generates and displays a permanent six-digit pairing code. Later starts
reuse it. The code and strong command credential live in the running account's
Windows `%LOCALAPPDATA%/linklet`, or Linux `$XDG_CONFIG_HOME/linklet` (default
`~/.config/linklet`), independent of the startup directory.

On each controller, pair once using the displayed code:

```sh
linklet pair --agent 10.0.0.5:8787 --code 083614
linklet probe --agent 10.0.0.5:8787
```

A new controller can pair unattended with the same code. Subsequent CLI and MCP
calls automatically load the credential saved for that host:port, including batches
with different targets. The address is still required; `discover` finds LAN listeners.

To revoke all controllers, stop the target and run `linklet-agent init`. This rotates
the code and credential, then starts normally. Initialization refuses while the saved
state belongs to a running agent. See [pairing](docs/pairing.md) for storage and protocol details.

## Calling it from an agent

`linklet mcp` speaks the Model Context Protocol on stdin and stdout, so an agent can
call this tool directly. No port and no configuration to keep in step: the client
starts the process and talks to it.

```json
{
  "command": "C:\\linklet\\linklet.exe",
  "args": ["mcp"]
}
```

Those two fields are the whole installation, and which file the entry goes in is the
client's business: `integrations/` has the entry to paste for DSH and the same two
fields for ZCode, with the skill that goes beside them. **`command` names the
executable and not a shell command**: an MCP client spawns it without a shell, which is
one thing a native `.exe` does not need a wrapper for.

Fourteen tools, one per intent: `pair`, `check`, `testbed`, `exec`, `ps`, `kill`, `spawn`,
`grep`, `tail`, `ls`, `push`, `pull`, `discover` and `probe`. They are the operations the commands above
perform, so a reader who knows one knows the other.

Call `pair` with `{"agent":"10.0.0.5:8787","code":"083614"}` once. Keep the
code as a string so leading zeros survive. The MCP server saves the long-term
credential for its OS account and never returns it as a tool result. The six-digit
code is the only credential tool argument.

`docs/MCP.md` is the surface as its maintainer sees it: why each tool is separate,
and the two places a caller goes wrong. `integrations/` is in this repository, and a
release is one archive -- `linklet-<version>.zip`, with the Windows and Linux
executables under `bin/<target-triple>/` and this installation material beside them --
so an installation does not need a clone and does not need a build for either system.
`docs/VERIFYING.md` says what builds it and what the archive can be trusted to be.

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

Core may depend on mature libraries for pure computation. It uses
`serde_json::Value` directly for JSON. `crates/linklet-core/tests/architecture.rs`
records each dependency and its use; review must still keep calls to filesystem,
network, process and environment APIs out of core. An empty dependency list would
not enforce that boundary, because `std` provides I/O too.

`linklet-core` declares the handshake and sealed-session contracts;
`linklet-adapters` implements them with `chacha20poly1305`, `hkdf`, `sha2` and
`x25519-dalek`. The adapter owns the operating system randomness they need.

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
