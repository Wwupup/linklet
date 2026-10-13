# Permanent pairing

Run `linklet-agent` on the target. First start creates a uniformly random six-digit
code and an independent 256-bit command token. Later starts reuse both. A new
controller can pair unattended with that permanent code; no confirmation, expiry,
one-time use or service installation is involved.

On each controller, run `linklet pair --agent host:8787 --code 083614`, or call
MCP `pair` with `{"agent":"host:8787","code":"083614"}`. Replace these example
digits with those printed on the target. Exactly six ASCII digits are required;
MCP requires a string to preserve leading zeros. The result names the target and
version, and reports local persistence without exposing either secret.

## Storage and reset

Both sides use the running account application directory:

- Windows: `%LOCALAPPDATA%/linklet`.
- Linux: `$XDG_CONFIG_HOME/linklet`, or `~/.config/linklet` when unset.

The target stores `agent.json`. Controllers store one `peer-<address-hash>.json`
per host:port. Address keys are case-insensitive; aliases and changed ports are
separate entries and require re-pairing. Working directory and executable location
do not select the store. Windows uses a protected DACL for the current user; Linux
directories are 0700 and newly written files are 0600. Replacement is atomic.
Corrupt state is an error, never an implicit rotation.

Only one managed target process may own an account store. Its `agent.lock` is held
by the OS for the process lifetime. Stop the process, then run `linklet-agent init`
to rotate both secrets and start normally. The old code and all old controller
credentials stop working. Initialization refuses while the store is owned by a
running agent. It does not stop processes or modify service/startup configuration.
Deleting files under a running process is not a supported reset method.

Explicit target `--token`, `--token-file`, `LINKLET_TOKEN` or `LINKLET_TOKEN_FILE`
disables managed pairing. `init` refuses explicit token configuration. On a
controller, an explicit environment token or token file overrides saved credentials.
Invalid explicit configuration never falls back. Clear these variables for the
automatic flow. CLI and MCP share lookup, including batches with different secrets.

## Exchange and bounds

The permanent code authorizes new controllers with the target process account
permissions. It is intentionally accepted in MCP arguments and may remain in client
history. Keep it private; rotate with `init` when needed. No per-controller identity,
selective revocation, rendezvous or NAT traversal is added. The address is required;
`discover` finds TCP listeners, without claiming authentication.

Pairing uses [opaque-ke](https://docs.rs/opaque-ke/4.0.1/opaque_ke/) with Ristretto255,
TripleDH/SHA-512 and Argon2id (the library default parameters). Registration runs
locally on startup from the saved code; there is no network registration endpoint.
Each attempt uses fresh randomness, context `linklet pairing opaque-v1`, and record
identifier `linklet-agent`. After both proofs succeed, the session key feeds the
existing directional HKDF/ChaCha20-Poly1305 channel with a distinct pairing salt.
Only that channel carries the strong command token. A captured handshake is not an
offline code-testing oracle. Normal command handshakes still use the strong token.

One agent-wide budget admits five immediate attempts, then replenishes one per
minute. All IPs and concurrent attempts share it. Successful and abandoned attempts
consume it before cryptography. Refusals give a cooldown; no human unlock is needed.
The budget is in memory and starts fresh after a process restart. Previously paired
command calls are not throttled. Six digits remain a small secret space; this
limits online guessing but is not internet-scale authentication.

Each pairing connection has two inbound messages, keeping the existing budget:
a hello containing `pairing: "opaque-v1"`, protocol and hex OPAQUE request; then a
hello containing the final client proof. The target replies with its OPAQUE response,
then sealed credentials. Each hex message is bounded to 2048 characters before
cryptographic parsing; existing frame and IO limits apply. No command is dispatched
on a pairing connection.

This is additive to protocol 2; existing fields keep their meaning. Earlier agents
reject the unfamiliar hello. Install current binaries on both ends to use pairing.
It belongs to the next minor release, with no release or deployment performed here.

Pair exits: 0 paired and saved, 1 connection failed, 2 arguments or local
configuration/storage error, 3 authentication/refusal, 4 invalid reply or transport
failure after connecting. MCP marks every failed pair as `isError`.
