# Installing linklet as an MCP server

Everything needed to drive linklet from an agent. Two pieces, and they do different
jobs:

| piece | what it adds | where it goes | required |
|---|---|---|---|
| MCP server | the tools themselves (`check`, `push`, `spawn`, `grep`, ...) | the client's server list -- `dsh/mcp-entry.yml` for DSH | yes -- without it the agent has no way to reach a target |
| skill | the procedure: the order of the calls, and the rules that produce misleading failures when ignored | `%USERPROFILE%\.dsh\skills\linklet\`, or the client's own skills directory | recommended |

They are complementary rather than alternatives. **The MCP server gives the agent
hands; the skill gives it the routine.** A model with the tools and no procedure will
push over a running executable, read a log before starting the program, or treat an
empty process listing as a clean machine -- each of which is a specific failure this
tool was built to make visible, and none of which the tool descriptions can prevent:
a description can say what a tool does and the order to call them in is a procedure.
`docs/MCP.md` records the experiment that found that boundary.

## Install the skill

The skill is a directory. Copy it to the client's skills directory, and the folder
name is the skill's name -- `linklet`.

```powershell
# DSH, user scope, from the unpacked release:
robocopy integrations\skills\linklet $env:USERPROFILE\.dsh\skills\linklet /E

# ZCode, user scope:
robocopy integrations\skills\linklet $env:USERPROFILE\.zcode\skills\linklet /E
```

A workspace-scope copy works the same way under `<repo>\.dsh\skills\linklet\` or
`<repo>\.zcode\skills\linklet\`. Skills are found by path, so a user-scope copy
shadows a workspace copy of the same name. Verify by asking for something the
description covers -- "deploy to the LAN machine and check the log" -- and if it does
not trigger, the usual cause is a description that does not name the situation.

## Configure the MCP server

Three fields, and every client wants the same three:

| field | value |
|---|---|
| `command` | the path to `linklet.exe`, and **not** a shell command |
| `args` | `["mcp"]` |
| `env` | `LINKLET_TOKEN_FILE`, naming a file whose first line is the shared secret |

**DSH.** Paste `dsh/mcp-entry.yml` into the top-level array of the profile's
`cordis.patch.yml` (`%USERPROFILE%\.dsh\profiles\<profile>\cordis.patch.yml`) and
replace the two paths in it. That file carries the notes about why the entry looks
the way it does.

**ZCode.** The same three fields go under `mcp.servers` in the client's config file
(`%USERPROFILE%\.zcode\cli\config.json`, or `<repo>\.zcode\config.json`), which
`lanlink`'s own `integrations/zcode/README.md` documents in full:

```json
{
  "mcp": {
    "servers": {
      "linklet": {
        "command": "C:\\linklet\\linklet.exe",
        "args": ["mcp"],
        "env": { "LINKLET_TOKEN_FILE": "C:\\linklet\\token.txt" }
      }
    }
  }
}
```

The ZCode shape is second-hand -- it is `lanlink`'s, not measured here -- and the DSH
entry is the one this repository has been set up with.

**Any other client** takes the same three fields. Two things to check when it does
not work: the path in `command` is the executable rather than a wrapper, and the file
the client reads is not one that gets committed, because a token file keeps the
secret out of it and `LINKLET_TOKEN` in the same place would not.

## The token

The first line of the token file is the secret; a byte-order mark and the line ending
are not part of it, so `Set-Content C:\linklet\token.txt -Value $secret` needs no
care. Both ends read `LINKLET_TOKEN_FILE`, so one file named on each side is the whole
configuration.

```powershell
# On the host, and on every target, with the same value:
Set-Content C:\linklet\token.txt -Value '<at least sixteen bytes>' -Encoding ascii
```

**Name one source.** With both `LINKLET_TOKEN_FILE` and `LINKLET_TOKEN` set, the
agent refuses to start and the host reports it and presents no token at all: the two
are two answers to one question, and whichever silently won would be the one the
operator did not believe was in force.

**Check it before wiring a client up.** `linklet probe --agent <host:port>` completes
a handshake and reads a reply, so it answers whether the secret and the agent agree
(exit 0) rather than whether something is listening. `check` cannot answer that: a
wedged agent keeps its listening socket open and reads as `live` -- `docs/smoke.md`
has the measurement.

## First, the agent has to be on the target

**No release and no tool here can put it there**, and that is a decision rather than a
gap: the first copy is a file copy, and doing it by hand once is how an operator finds
out what a deployment actually consists of. On the target, once:

```powershell
mkdir C:\linklet\transfers
linklet-agent.exe --port 8787 --root C:\linklet\transfers --log C:\linklet\agent.log
New-NetFirewallRule -DisplayName linklet-agent -Direction Inbound `
    -Protocol TCP -LocalPort 8787 -Action Allow
```

Then make it survive the console: `docs/smoke.md` has the `schtasks` recipe and
`tools/linklet-supervise.ps1`, which restarts an agent that died **and** one that
wedged. An agent started from a console dies with that console, and the port it was
listening on is then silently dropped rather than refused -- which reads exactly like
a firewall that was never opened.

## Reference

- `docs/MCP.md` -- the eleven tools, the two places a caller goes wrong, and the
  protocol decisions
- `skills/linklet/SKILL.md` -- the shipped skill, readable as plain documentation if
  you would rather not install it
- `docs/smoke.md` -- the target side in full: the firewall, the scheduler, the
  supervisor, and what was measured on a real machine
