# Device-free checks for the packaging version readers.
$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$keys = @('TRANSIT_VERSION')
$saved = @{}
foreach ($key in $keys) { $saved[$key] = [Environment]::GetEnvironmentVariable($key) }

function Assert-Equal($actual, $expected) {
  if ($actual -ne $expected) { throw "expected '$expected', got '$actual'" }
}

try {
  # Execute only the version-reader prefix; no build, signing, or executable launch.
  $packager = Get-Content -Raw -LiteralPath "$repoRoot/packaging/windows/package.ps1"
  $prefix = $packager.Substring(0, $packager.IndexOf('$Target = switch')).Replace('$PSScriptRoot', '(Join-Path $repoRoot "packaging/windows")')
  $env:TRANSIT_VERSION = ''
  $version = & {
    & ([scriptblock]::Create($prefix + [Environment]::NewLine + '$Version'))
  }
  $workspaceVersion = [regex]::Match((Get-Content -Raw -LiteralPath "$repoRoot/Cargo.toml"), '(?s)\[workspace.package\].*?version = "([^"]+)"').Groups[1].Value
  Assert-Equal $version $workspaceVersion
  $env:TRANSIT_VERSION = '2.3.4-beta.1'
  $version = & {
    & ([scriptblock]::Create($prefix + [Environment]::NewLine + '$Version'))
  }
  Assert-Equal $version '2.3.4-beta.1'
  Write-Output 'tooling PowerShell checks: ok'
} finally {
  foreach ($key in $keys) { [Environment]::SetEnvironmentVariable($key, $saved[$key]) }
}
