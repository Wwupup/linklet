#!/usr/bin/env bash
# Assemble the release: one archive holding both platforms, one digest, and the notes.
#
# This is to the release job what `tools/verify.ps1` is to the gate job -- the logic lives in one
# file that CI calls, so that a person can run the same thing against a checkout instead of
# reading a workflow and hoping. The workflow is three steps: download the binaries, call this,
# attach what it produced.
#
#   tools/make_release.sh <version> <binaries-dir> <out-dir> <owner/repo>
#
# `<binaries-dir>` holds one directory per target triple, each with the two executables:
#
#   <binaries>/x86_64-pc-windows-msvc/linklet.exe       <binaries>/x86_64-unknown-linux-gnu/linklet
#   <binaries>/x86_64-pc-windows-msvc/linklet-agent.exe <binaries>/x86_64-unknown-linux-gnu/linklet-agent
#
# Which is exactly what `actions/download-artifact` produces when each platform uploads under its
# own triple -- so the directory names are not decoration, they are the handover format.
#
# # Why this runs on Linux, and the archive used to be assembled on Windows
#
# **Because a zip carries the Unix mode, and the tool that made the old archive did not write
# it.** Measured, both halves:
#
#   - `Compress-Archive` writes `external_attr = 0` on every entry. Unzipped on Linux, the files
#     come out mode **600** -- not executable, and not even readable by anyone else.
#   - A zip whose entries carry the mode comes back **755 / 644**, which is what `unzip` restores.
#
# So an archive made by `Compress-Archive` ships a Linux binary that has to be `chmod +x`ed before
# it will run, and one made by Info-ZIP `zip` does not. `zip` and `unzip` are both on the Ubuntu
# runner image; `docs/VERSIONING.md` records the pins and this dependency.
#
# **It is also why the checks below extract the archive rather than trusting the staging.** The
# mode is a property of the thing that comes out of the zip, so that is the thing to look at.
#
# # What it checks before it will produce anything
#
#   1. Both triples are present, each with both executables.
#   2. Every payload item is in the tag -- checked **before** anything is staged, because a package
#      that lost the skill would still publish and would only be missed by whoever tried to follow
#      it.
#   3. The finished archive contains every path it meant to, at the top level it meant to.
#   4. The Linux executables are executable **after extraction**.
#
# Exits non-zero at the first failure. Nothing is attached to a release here; this only writes
# `<out>/linklet-<version>.zip`, `<out>/SHA256SUMS` and `<out>/notes.md`.

set -euo pipefail

if [ "$#" -ne 4 ]; then
    echo "usage: $0 <version> <binaries-dir> <out-dir> <owner/repo>" >&2
    exit 2
fi

version="$1"
binaries="$2"
out="$3"
repo="$4"

windows=x86_64-pc-windows-msvc
linux=x86_64-unknown-linux-gnu

# The repository, from this script's own location, so it can be run from anywhere.
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Everything that goes into the archive that is **not** built: it is copied out of the tag. No
# `tools/`: the scripts that used to be shipped here kept an agent alive and drove a real machine,
# and both are gone -- the first because a script is not a service, and the second because a tool
# an agent drives does not need a script to drive it.
payload=(
    README.md
    CHANGELOG.md
    LICENSE
    docs/MCP.md
    docs/machine.md
    docs/smoke.md
    integrations
)

fail() {
    echo "make_release: $*" >&2
    exit 1
}

[ -n "$version" ] || fail "no version given"
[ -d "$binaries" ] || fail "no binaries directory at $binaries"
[ -d "$root/integrations" ] || fail "$root does not look like a linklet checkout"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
stage="$work/stage/linklet-$version"

# 1. Both platforms, both executables. A missing one is a build that did not happen, and an
#    archive that quietly holds one platform is the failure this file exists to make impossible.
for triple in "$windows" "$linux"; do
    case "$triple" in
        "$windows") names=(linklet.exe linklet-agent.exe) ;;
        *) names=(linklet linklet-agent) ;;
    esac
    for name in "${names[@]}"; do
        [ -f "$binaries/$triple/$name" ] ||
            fail "expected $binaries/$triple/$name -- both platforms have to be built before an \
archive is assembled, and this one is missing"
    done

    mkdir -p "$stage/bin/$triple"
    for name in "${names[@]}"; do
        cp "$binaries/$triple/$name" "$stage/bin/$triple/$name"
    done
done

# 2. The payload, before anything else is staged.
for item in "${payload[@]}"; do
    [ -e "$root/$item" ] || fail "the release needs $item, which is not in the tag"
done

# **The executable bit has to be on the file before the zip is written, because the zip is what
# records it.** The Linux binaries arrive from an artifact download whose round trip is not what is
# being tested -- what is tested is that the archive restores the bit, in the check below.
chmod +x "$stage/bin/$linux/linklet" "$stage/bin/$linux/linklet-agent"

# Everything else is a file from the tag, at the same place it has inside the repository.
for item in "${payload[@]}"; do
    if [ -d "$root/$item" ]; then
        mkdir -p "$stage/$item"
        cp -R "$root/$item/." "$stage/$item/"
    else
        mkdir -p "$stage/$(dirname "$item")"
        cp "$root/$item" "$stage/$item"
    fi
done

# 3. One archive, with the folder at its top level, so that unzipping into a shared directory does
#    not scatter a dozen entries among whatever else is there.
mkdir -p "$out"
out="$(cd "$out" && pwd)"
archive="$out/linklet-$version.zip"
rm -f "$archive"
( cd "$work/stage" && zip -q -r "$archive" "linklet-$version" )

# 4. Read the archive back and check what it actually holds, rather than trusting the staging
#    above.
expected=(
    "linklet-$version/bin/$windows/linklet.exe"
    "linklet-$version/bin/$windows/linklet-agent.exe"
    "linklet-$version/bin/$linux/linklet"
    "linklet-$version/bin/$linux/linklet-agent"
)
for item in "${payload[@]}"; do
    expected+=("linklet-$version/$item")
done

# `unzip -Z1` prints a directory with a trailing slash, so the listing is normalised before it is
# compared -- otherwise the payload's own directories read as missing, which is what this check
# said the first time it was run.
listing="$(unzip -Z1 "$archive" | sed 's|/$||')"
for path in "${expected[@]}"; do
    printf '%s\n' "$listing" | grep -qx -- "$path" ||
        fail "the archive does not hold $path"
done

# **The one check that decides whether this ran on the right platform.** A zip that did not carry
# the Unix mode gives back mode 600 here, which is a Linux binary nobody can run.
mkdir -p "$work/check"
( cd "$work/check" && unzip -q "$archive" "linklet-$version/bin/$linux/*" )
for name in linklet linklet-agent; do
    [ -x "$work/check/linklet-$version/bin/$linux/$name" ] ||
        fail "$name is not executable after extraction -- the archive did not carry the Unix \
mode, which is the one thing it has to get right for a Linux user"
done

# 5. The digest, beside the archive and never inside it: a digest inside the thing it verifies
#    cannot be used to verify it, and the archive is what a download can corrupt.
( cd "$out" && sha256sum "linklet-$version.zip" >SHA256SUMS )

# 6. The notes. A pointer and not a copy: `CHANGELOG.md` is the one place the answer to "what can I
#    do now that I could not do before" is written, and a release body that repeated it would be a
#    second copy to keep in step.
#
#    A quoted heredoc, because the text carries backticks that a bare heredoc would run as
#    commands; the two values that vary are substituted by name afterwards.
cat >"$out/notes.md" <<'NOTES'
linklet `@VERSION@`, built from the tag **on Windows and on Linux**. One archive, and one file to
verify it with.

`linklet-@VERSION@.zip` holds **one directory per platform**, named for the target triple, and the
rest of a deployment beside them:

```
linklet-@VERSION@/
  bin/x86_64-pc-windows-msvc/linklet.exe        the host tool, and the MCP server
  bin/x86_64-pc-windows-msvc/linklet-agent.exe  goes on each Windows machine being driven
  bin/x86_64-unknown-linux-gnu/linklet          the same tool, built on Linux
  bin/x86_64-unknown-linux-gnu/linklet-agent    the agent for a Linux machine
  integrations/                                 the client entry, and the skill
  README.md, CHANGELOG.md, LICENSE, docs/       what an operator reads
```

**Each binary is built by the platform it runs on** rather than cross-compiled, and both come from
the tag. The agent and the tool should come from the same release; **the agent may be ahead of the
tool and not behind it**, which is `docs/VERSIONING.md`'s rule and its reason.

**On Linux the two files are already executable**, and the archive records that. Some unzip tools
restore it and some do not; if `bin/x86_64-unknown-linux-gnu/linklet` arrives without the bit,
`chmod +x` is all it needs.

**Unzip it on the host.** Nothing here installs an agent on a target: the first copy is a file
copy, once, by hand, and `integrations/README.md` says what it consists of.

**What changed** is in `CHANGELOG.md` under `[@VERSION@]`:
@BLOB@/CHANGELOG.md

**What a version number means here**, which changes raise it, and the upgrade order:
@BLOB@/docs/VERSIONING.md

Verify the archive against `SHA256SUMS` before you unpack it.
NOTES

blob="https://github.com/$repo/blob/v$version"
sed -i "s/@VERSION@/$version/g; s|@BLOB@|$blob|g" "$out/notes.md"

echo "linklet-$version.zip:"
( cd "$stage" && find . -type f | sed "s|^\./|  linklet-$version/|" | sort )
echo
cat "$out/SHA256SUMS"
