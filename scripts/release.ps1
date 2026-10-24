<#
.SYNOPSIS
  Cut a release: bump the version, test, commit, tag and push (the push starts the release workflow).

.EXAMPLE
  scripts/release.ps1 -Version 0.2.0
  scripts/release.ps1 -Version 0.2.0 -Yes     # no confirmation before pushing
#>
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [switch]$Yes
)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path -Parent $PSScriptRoot)

foreach ($d in "$env:USERPROFILE\.cargo\bin", 'C:\Program Files\Git\cmd', 'C:\Program Files\GitHub CLI') {
    if ((Test-Path $d) -and ($env:Path -notlike "*$d*")) { $env:Path = "$d;$env:Path" }
}

function Run([string]$exe, [string[]]$a) {
    $old = $ErrorActionPreference; $ErrorActionPreference = 'Continue'   # native stderr is not an error
    & $exe @a
    $code = $LASTEXITCODE; $ErrorActionPreference = $old
    if ($code -ne 0) { throw "$exe $($a -join ' ') failed (exit $code)" }
}

if ($Version -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$') { throw "Version must look like 1.2.3 or 1.2.3-rc1, got '$Version'" }
$tag = "v$Version"

$branch = (git rev-parse --abbrev-ref HEAD).Trim()
if ($branch -ne 'main') { throw "Must be on main (currently on '$branch')." }
if (git status --porcelain) { throw 'Working tree is not clean; commit or stash first.' }
if (git tag --list $tag) { throw "Tag $tag already exists locally." }
Run git @('fetch', 'origin', '--tags', '--quiet')
if (git ls-remote --tags origin "refs/tags/$tag") { throw "Tag $tag already exists on origin." }
$behind = (git rev-list --count 'HEAD..origin/main').Trim()
if ($behind -ne '0') { throw "main is $behind commit(s) behind origin/main; pull first." }

# CHANGELOG must have a section for this version ("## 0.2.0" or "## [0.2.0]", optional date after).
$esc = [regex]::Escape($Version)
if (-not (Select-String -Path CHANGELOG.md -Pattern "^##\s+\[?v?$esc\]?(\s|$)" -Quiet)) {
    throw "CHANGELOG.md has no '## $Version' section."
}

# Bump [workspace.package] version.
$cargo = Get-Content Cargo.toml -Raw
$re = '(?ms)(\[workspace\.package\].*?^version\s*=\s*")[^"]+(")'
if ($cargo -notmatch $re) { throw 'Could not find version in [workspace.package] of Cargo.toml.' }
$new = [regex]::Replace($cargo, $re, "`${1}$Version`${2}")
if ($new -ne $cargo) {
    [IO.File]::WriteAllText((Join-Path $PWD 'Cargo.toml'), $new, (New-Object Text.UTF8Encoding $false))
}
Run cargo @('check', '--workspace')   # refreshes Cargo.lock
Run cargo @('test', '--workspace', '--locked')

git add Cargo.toml Cargo.lock
if (git status --porcelain) {
    Run git @('commit', '-m', "Release $tag", '-m', 'Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>')
} else {
    Write-Host "Version is already $Version; nothing to commit."
}
Run git @('tag', '-a', $tag, '-m', "Release $tag")

if (-not $Yes) {
    $answer = Read-Host "Push main and $tag to origin now (this starts the release workflow)? [y/N]"
    if ($answer -notmatch '^(y|yes)$') {
        Write-Host "Not pushed. Local commit and tag $tag are in place. To push later: git push origin main $tag"
        Write-Host "To undo instead: git tag -d $tag; git reset --hard HEAD~1  (only if a release commit was made)"
        exit 1
    }
}
Run git @('push', 'origin', 'main')
Run git @('push', 'origin', $tag)
Write-Host "Pushed $tag. Watch the release: gh run watch"
