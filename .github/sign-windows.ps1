[CmdletBinding()]
param([Parameter(Mandatory = $true)][string]$Directory)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if (-not $env:WINDOWS_SIGNING_PFX_BASE64 -or -not $env:WINDOWS_SIGNING_PFX_PASSWORD) {
    throw 'Both Windows signing secrets are required.'
}
$executables = @(Get-ChildItem -LiteralPath $Directory -Filter '*.exe' -File)
if ($executables.Count -eq 0) { throw 'No Windows executables to sign.' }
$signTool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" |
    Sort-Object { [version]$_.Directory.Parent.Name } -Descending |
    Select-Object -First 1
if (-not $signTool) { throw 'Windows SDK SignTool is missing.' }

$pfxPath = Join-Path $env:RUNNER_TEMP 'hammerr-signing.pfx'
$publicPath = Join-Path $Directory 'HammeRR-signing.cer'
$trustStore = 'Cert:\LocalMachine\Root'
$imported = @()
$trusted = $null
$password = ConvertTo-SecureString $env:WINDOWS_SIGNING_PFX_PASSWORD -AsPlainText -Force
try {
    Write-Host 'Import signing identity'
    [IO.File]::WriteAllBytes($pfxPath, [Convert]::FromBase64String($env:WINDOWS_SIGNING_PFX_BASE64))
    $imported = @(Import-PfxCertificate -FilePath $pfxPath -Password $password -CertStoreLocation 'Cert:\CurrentUser\My')
    $certificates = @($imported | Where-Object HasPrivateKey)
    if ($certificates.Count -ne 1) { throw 'The PFX must contain exactly one signing identity.' }
    $certificate = $certificates[0]
    $now = Get-Date
    if ($now -lt $certificate.NotBefore -or $now -ge $certificate.NotAfter) {
        throw 'The signing certificate is not currently valid.'
    }
    Export-Certificate -Cert $certificate -FilePath $publicPath -Type CERT | Out-Null
    Write-Host 'Trust signing certificate on the runner'
    $trusted = Import-Certificate -FilePath $publicPath -CertStoreLocation $trustStore
    Remove-Item -LiteralPath $pfxPath -Force
    $env:WINDOWS_SIGNING_PFX_BASE64 = $null
    $env:WINDOWS_SIGNING_PFX_PASSWORD = $null

    Write-Host "Sign and timestamp $($executables.Count) executables"
    & $signTool.FullName sign /s My /sha1 $certificate.Thumbprint /fd SHA256 /tr http://timestamp.digicert.com /td SHA256 $executables.FullName
    if ($LASTEXITCODE -ne 0) { throw 'Windows signing or timestamping failed.' }
    Write-Host 'Verify signed executables'
    & $signTool.FullName verify /pa /all /tw $executables.FullName
    if ($LASTEXITCODE -ne 0) { throw 'Windows signature verification failed.' }
    foreach ($executable in $executables) {
        $signature = Get-AuthenticodeSignature -LiteralPath $executable.FullName
        if ($signature.Status -ne 'Valid' -or
            $signature.SignerCertificate.Thumbprint -ne $certificate.Thumbprint -or
            -not $signature.TimeStamperCertificate) {
            throw "Wrong signer, missing timestamp or invalid signature: $($executable.Name)"
        }
    }
} finally {
    try {
        if (Test-Path -LiteralPath $pfxPath) { Remove-Item -LiteralPath $pfxPath -Force }
    } finally {
        try {
            if ($trusted) { Remove-Item -Path (Join-Path $trustStore $trusted.Thumbprint) }
        } finally {
            foreach ($certificate in $imported) {
                Remove-Item -Path "Cert:\CurrentUser\My\$($certificate.Thumbprint)" -DeleteKey
            }
            $password.Dispose()
        }
    }
}
