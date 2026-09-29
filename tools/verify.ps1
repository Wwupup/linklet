#!/usr/bin/env pwsh
# The four gates, in one place, so that no reader has to be trusted to run them.
#
# This script exists because of a specific failure. The rules were written, the
# gates were documented in three files, and a commit still went in red: the
# implementation was checked out over by an older version and everything after it
# was built on a function that no longer existed. Nothing was wrong with the
# rules. Nothing ran them.
#
# So there is one entry point, and every caller refactors through it:
#
#   - a person before committing:   pwsh tools/verify.ps1
#   - CI:                           the workflow calls this exact script
#   - a future hook:                same line again
#
# Putting the commands in CI instead of here would be the mistake this file is
# meant to prevent: the same four commands in two places, drifting, with the copy
# that runs in CI being the one nobody tries locally.
#
# Exits non-zero at the first gate that fails. Deliberately not "run everything
# and report at the end": a later gate's output is noise while an earlier one is
# broken, and the reader has to work out which failure is upstream of which.

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Invoke-Gate {
    param(
        [Parameter(Mandatory)][string] $Name,
        [Parameter(Mandatory)][scriptblock] $Body
    )

    Write-Host "==> $Name" -ForegroundColor Cyan
    & $Body
    if ($LASTEXITCODE -ne 0) {
        Write-Host "FAILED: $Name" -ForegroundColor Red
        exit $LASTEXITCODE
    }
}

# 1. Formatting. Cheapest gate, and the one whose failures are pure noise: a diff
#    that shows every line changed because of whitespace hides the real change.
Invoke-Gate 'cargo fmt --check' {
    cargo fmt --all --check
}

# 2. Lints. `-D warnings` rather than a wall of yellow, because a warning that is
#    allowed to stay is a warning nobody reads.
Invoke-Gate 'cargo clippy -D warnings' {
    cargo clippy --workspace --all-targets -- -D warnings
}

# 3. Tests.
Invoke-Gate 'cargo test' {
    cargo test --workspace
}

# 4. Documentation links. `cargo doc` treats a broken intra-doc link as a warning
#    and carries on, so without this flag a link that stopped resolving stays
#    broken and stays invisible.
Invoke-Gate 'cargo doc -D warnings' {
    $env:RUSTDOCFLAGS = '-D warnings'
    try {
        cargo doc --workspace --no-deps
    }
    finally {
        Remove-Item Env:\RUSTDOCFLAGS -ErrorAction SilentlyContinue
    }
}

# 5. The layer rule, as an inventory rather than as a prohibition.
#
#    `tests/architecture.rs` already fails if the core gains a dependency, and
#    that test is the one that matters. This line is the other half: it prints
#    what the workspace depends on, so a new dependency anywhere is visible in
#    the output of a normal verification run instead of being noticed in a diff.
Invoke-Gate 'dependency inventory' {
    cargo metadata --format-version 1 --no-deps |
        ConvertFrom-Json |
        ForEach-Object { $_.packages } |
        ForEach-Object {
            $name = $_.name
            $deps = ($_.dependencies | ForEach-Object { $_.name }) -join ', '
            Write-Host ("    {0,-20} {1}" -f $name, $(if ($deps) { $deps } else { '(none)' }))
        }
}

Write-Host ''
Write-Host 'all gates passed' -ForegroundColor Green
exit 0
