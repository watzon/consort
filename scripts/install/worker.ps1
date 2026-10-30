<#
.SYNOPSIS
Installs the Consort worker on Windows, and pairs it with a hub.

.DESCRIPTION
1. Finds the release asset for this system on GitHub Releases.
2. Downloads it and checks its SHA-256 against the SHA256SUMS file of the
   same release.
3. Puts consort.exe in the install folder (default
   %LOCALAPPDATA%\Consort\bin) and adds that folder to the PATH of your user.
4. Runs `consort worker install`, which pairs the machine with the hub and
   installs a Task Scheduler task for your user. It needs no admin rights.

Run it from PowerShell:

  & ([scriptblock]::Create((irm https://raw.githubusercontent.com/watzon/consort/main/scripts/install/worker.ps1))) -Hub https://hub.example.com -Code ABCD-EFGH

.PARAMETER Hub
The hub URL. Required, unless -NoService is set.

.PARAMETER Code
The pairing code from a client. Without it, the CONSORT_PAIRING_CODE
variable or a prompt gives it.

.PARAMETER Name
The worker name. The default is the host name.

.PARAMETER Tag
Tags for placement.

.PARAMETER Version
A release tag such as v0.2.0. The default is the latest release.

.PARAMETER NoService
Install the program only. Do not pair or install a task.

.PARAMETER DryRun
Print the steps and download nothing.

The variables CONSORT_INSTALL_DIR and CONSORT_DOWNLOAD_BASE change the install
folder and the folder URL of the release assets. The tests use them.
#>
[CmdletBinding()]
param(
  [string]$Hub = '',
  [string]$Code = '',
  [string]$Name = '',
  [string[]]$Tag = @(),
  [string]$Version = '',
  [switch]$NoService,
  [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# The one place that names the GitHub repository in this script. The shell
# script, the release workflow and the app have their own copy.
$Repo = 'watzon/consort'

function Fail([string]$Message) {
  throw "consort install: $Message"
}

if (-not $NoService -and -not $Hub) {
  Fail 'pass -Hub with the URL of your hub, or pass -NoService'
}

# The platform key is the same one that the release workflow puts in the
# asset names.
$arch = $env:PROCESSOR_ARCHITEW6432
if (-not $arch) { $arch = $env:PROCESSOR_ARCHITECTURE }
switch ($arch) {
  'AMD64' { $key = 'windows-x86_64' }
  default { Fail "no release for the processor $arch" }
}
$asset = "consort-$key.zip"

if ($env:CONSORT_DOWNLOAD_BASE) {
  $base = $env:CONSORT_DOWNLOAD_BASE.TrimEnd('/')
} elseif ($Version) {
  $base = "https://github.com/$Repo/releases/download/$Version"
} else {
  $base = "https://github.com/$Repo/releases/latest/download"
}

if ($env:CONSORT_INSTALL_DIR) {
  $installDir = $env:CONSORT_INSTALL_DIR
} else {
  $installDir = Join-Path $env:LOCALAPPDATA 'Consort\bin'
}

if ($DryRun) {
  Write-Host 'Dry run. Nothing is downloaded or changed.'
  Write-Host "Download $base/$asset"
  Write-Host "Check it against $base/SHA256SUMS"
  Write-Host "Put consort.exe in $installDir"
  if (-not $NoService) {
    Write-Host "Run: consort worker install --hub $Hub (with the pairing code)"
  }
  return
}

$work = Join-Path ([System.IO.Path]::GetTempPath()) ("consort-" + [System.Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work | Out-Null
try {
  Write-Host "Downloading $asset"
  $zip = Join-Path $work $asset
  $sums = Join-Path $work 'SHA256SUMS'
  Invoke-WebRequest -UseBasicParsing -Uri "$base/$asset" -OutFile $zip
  Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS" -OutFile $sums

  # A line of SHA256SUMS is "<hash>  <file name>".
  $expected = $null
  foreach ($line in Get-Content -Path $sums) {
    $parts = $line.Trim() -split '\s+', 2
    if ($parts.Length -eq 2 -and $parts[1].TrimStart('*') -eq $asset) {
      $expected = $parts[0].ToLowerInvariant()
      break
    }
  }
  if (-not $expected) { Fail "SHA256SUMS has no entry for $asset" }
  $actual = (Get-FileHash -Algorithm SHA256 -Path $zip).Hash.ToLowerInvariant()
  if ($expected -ne $actual) {
    Fail "the checksum of $asset does not match. Expected $expected, got $actual. Nothing was installed"
  }
  Write-Host 'Checksum is correct.'

  $unpacked = Join-Path $work 'unpacked'
  Expand-Archive -Path $zip -DestinationPath $unpacked
  $exe = Join-Path $unpacked 'consort.exe'
  if (-not (Test-Path $exe)) { Fail "$asset has no consort.exe" }

  New-Item -ItemType Directory -Force -Path $installDir | Out-Null
  $target = Join-Path $installDir 'consort.exe'
  # A running worker holds its file. Move it aside first, so the copy works.
  # The old file goes away at the next install.
  $old = "$target.old"
  if (Test-Path $old) { Remove-Item -Force -ErrorAction SilentlyContinue $old }
  if (Test-Path $target) { Move-Item -Force $target $old }
  Copy-Item -Force $exe $target
  Write-Host "Installed $target"
} finally {
  Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $work
}

# Add the folder to the PATH of the user, and to this session.
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$entries = @()
if ($userPath) { $entries = $userPath -split ';' | Where-Object { $_ } }
if ($entries -notcontains $installDir) {
  [Environment]::SetEnvironmentVariable('Path', (($entries + $installDir) -join ';'), 'User')
  Write-Host "Added $installDir to your PATH. Open a new terminal to use it."
}
if (($env:Path -split ';') -notcontains $installDir) {
  $env:Path = "$env:Path;$installDir"
}

if ($NoService) {
  Write-Host 'Done. Pair this machine with: consort worker install --hub <url> --code <code>'
  return
}

$install = @('worker', 'install', '--hub', $Hub)
if ($Name) { $install += @('--name', $Name) }
foreach ($t in $Tag) { $install += @('--tag', $t) }
# The code goes in the environment, so other users of this machine do not
# see it in the process list.
if ($Code) { $env:CONSORT_PAIRING_CODE = $Code }
try {
  & $target @install
  if ($LASTEXITCODE -ne 0) { Fail "consort worker install failed with exit code $LASTEXITCODE" }
} finally {
  if ($Code) { Remove-Item Env:\CONSORT_PAIRING_CODE -ErrorAction SilentlyContinue }
}
