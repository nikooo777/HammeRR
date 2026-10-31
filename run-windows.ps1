[CmdletBinding()]
param(
    [switch]$Release,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$EditorArguments
)

$ErrorActionPreference = 'Stop'
$cargoCommand = Get-Command cargo -ErrorAction SilentlyContinue
$cargoPath = if ($cargoCommand) { $cargoCommand.Source } else { Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe' }
if (-not (Test-Path -LiteralPath $cargoPath)) {
    throw 'Install Rust with rustup, then run this script again.'
}
$buildConfiguration = if ($Release) { 'release' } else { 'debug' }
$buildArguments = @('build', '--locked', '-p', 'editor', '-p', 'compiler', '-p', 'native-runtime')
if ($Release) { $buildArguments += '--release' }
Push-Location -LiteralPath $PSScriptRoot
try {
    & $cargoPath @buildArguments
    if ($LASTEXITCODE -ne 0) { throw 'HammeRR build failed.' }
    $editorPath = Join-Path $PSScriptRoot "target\$buildConfiguration\editor.exe"
    & $editorPath @EditorArguments
    if ($LASTEXITCODE -ne 0) { throw "HammeRR exited with code $LASTEXITCODE." }
} finally {
    Pop-Location
}
