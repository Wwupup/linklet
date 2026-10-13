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

## Where these files are

A release is one archive, `linklet-<version>.zip`, and this file is inside it. Unzip
it anywhere and the paths below are relative to the folder it creates:

```
linklet-<version>/
  bin/x86_64-pc-windows-msvc/linklet.exe          the host tool, and the MCP server a client is pointed at
  bin/x86_64-pc-windows-msvc/linklet-agent.exe    goes on each Windows machine being driven
  bin/x86_64-unknown-linux-gnu/linklet            the same tool for a Linux host
  bin/x86_64-unknown-linux-gnu/linklet-agent      the agent for a Linux machine
  integrations/                                   this file, the client entry, and the skill
  README.md, CHANGELOG.md, LICENSE, docs/
```

**Both platforms are in the one archive**, one directory per target triple, so `bin/` is not
ambiguous and a host and the agent it drives do not have to come from different downloads. **A
build is used on the platform it was built for and on the other one it is not**: the two are
different executables, and neither runs on the other's system.

The executables are self-contained -- no runtime, no installer -- so the directory for one
platform can be copied to a target or put on `PATH` on its own. On Linux the two files are
already executable, which the archive records. Everything else is documentation and
configuration, and none of it needs to travel to the machines being driven.

## Install the skill

The skill is a directory. Copy it to the client's skills directory, and the folder
name is the skill's name -- `linklet`.

```powershell
# DSH, user scope, from the unpacked archive:
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

Two fields are required; credentials are remembered after pairing:

| field | value |
|---|---|
| `command` | the path to `linklet.exe`, and **not** a shell command |
| `args` | `["mcp"]` |

**DSH.** Paste `dsh/mcp-entry.yml` into the top-level array of the profile's
`cordis.patch.yml` (`%USERPROFILE%\.dsh\profiles\<profile>\cordis.patch.yml`) and
replace the executable path in it. That file carries the notes about why the entry looks
the way it does.

**ZCode.** The same fields go under `mcp.servers` in the client's config file
(`%USERPROFILE%\.zcode\cli\config.json`, or `<repo>\.zcode\config.json`), which
`lanlink`'s own `integrations/zcode/README.md` documents in full:

```json
{
  "mcp": {
    "servers": {
      "linklet": {
        "command": "C:\\linklet\\linklet.exe",
        "args": ["mcp"]
      }
    }
  }
}
```

The ZCode shape is second-hand -- it is `lanlink`'s, not measured here -- and the DSH
entry is the one this repository has been set up with.

**Any other client** takes the same two fields. Check that `command` names the
executable directly and that the MCP server runs under the account used to pair.

## Pair once

Start the target, then call MCP `pair` with `agent` and its displayed six-digit
`code`, for example `{"agent":"10.0.0.5:8787","code":"083614"}`. Use the actual
displayed code as a string. Even a new controller can pair unattended. The MCP
server saves the strong credential under its OS account; later operations and CLI
calls use it automatically. `probe` then confirms authenticated name and version.

Long-term tokens are never MCP arguments or results. See `docs/pairing.md` for
reset and storage.

## First, the agent has to be on the target

**No release and no tool here can put it there**, and that is a decision rather than a
gap: the first copy is a file copy, and doing it by hand once is how an operator finds
out what a deployment actually consists of. On the target, configure the firewall from an elevated terminal, then start the
agent from the account whose permissions it should use:

```powershell
New-NetFirewallRule -DisplayName linklet-agent -Direction Inbound `
    -Protocol TCP -LocalPort 8787 -Action Allow
New-Item -ItemType Directory -Force C:\linklet\transfers
Set-Location C:\linklet\transfers
C:\linklet\linklet-agent.exe
```

**The log is kept for you**, at `logs/agent.log` beside `linklet-agent.exe`, and it
rolls over at a mebibyte keeping three older files. Nothing has to be passed for that;
`--log <file>` moves it and `--no-log` turns it off, and the log is where the request
that never finished is named -- see `docs/smoke.md`.

The agent generates and saves credentials on first start
and prints a permanent six-digit code. Later starts reuse it. Stop the process and
run `linklet-agent init` to rotate both secrets and revoke all paired controllers.
Initialization refuses while the same account store is still in use. File tools
use the startup directory for relative paths and the process account permissions.

Keep the ordinary agent process running in the intended account and startup
directory. Exiting it makes the target offline. This workflow installs no service,
autostart task, supervisor, or automatic restart. Authenticate with probe before
using it; the tool returns the remote name and version on success. Upgrade both
binaries together for protocol 3.

## Reference

- `docs/MCP.md` -- the fourteen tools, the two places a caller goes wrong, and the
  protocol decisions
- `skills/linklet/SKILL.md` -- the shipped skill, readable as plain documentation if
  you would rather not install it
- `docs/smoke.md` -- the target side in full: the firewall, the process, the log,
  and what was measured on a real machine
