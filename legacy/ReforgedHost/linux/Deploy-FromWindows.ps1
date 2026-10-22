<#
.SYNOPSIS
    Sets up a Dauntless Reforged Revival server on a Linux VPS (Debian 13) from this Windows PC,
    and sends it your Dauntless 1.14.7.

.DESCRIPTION
    Uses the OpenSSH client that comes with Windows (ssh, scp). In order:
      1. checks your game: Dauntless 1.14.7, by the SHA-256 of its executable
      2. sets up an SSH key, so the VPS password is asked only once
      3. runs setup-vps.sh on the VPS: packages, MongoDB, the server's user, the server release
         (checked against its SHA256SUMS.txt), reforged-host setup
      4. sends the game (about 15 GB; skipped when the VPS already has the same files)
      5. sends the runtime (MysticParadox.dll, winmm.dll) and the game data pack, when this PC has them
      6. runs setup-vps.sh --finish: starts the server and prints your first invite
    Run it again at any time: every step keeps what is already done. See docs/hosting-linux.md.

.PARAMETER Server
    The VPS's public IPv4 address (Hostinger: hPanel > VPS > Overview) or DNS name.

.PARAMETER Game
    Your Dauntless 1.14.7 folder: the one with Archon inside (default C:\Dauntless).

.PARAMETER Runtime
    A folder with MysticParadox.dll and winmm.dll. Default: the published runtime of a Reforged
    project on this PC (C:\Reforged\.workspace\runtime or beside the game), when there is one.

.PARAMETER PublicPort
    The TCP port players connect to, when 443 is taken on the VPS (a Hostinger Game Panel plan uses
    443 for its panel): 4443, for example. It goes into every invite; allow it in the VPS firewall.

.PARAMETER GameData
    A game data pack (reforged-host gamedata --export). Default: the newest pack of that project.

.PARAMETER Version
    The server release to install (x.y.z). Default: the newest.

.PARAMETER Branch
    The branch setup-vps.sh is downloaded from, when it is not beside this script (default main).

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\Deploy-FromWindows.ps1 -Server 203.0.113.7 -Name "Freeza Hunts"
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Server,
    [string]$Name = 'Dauntless Reforged Revival',
    [string]$Game = 'C:\Dauntless',
    [string]$User = 'root',
    [int]$Port = 22,
    [string]$Runtime = '',
    [string]$GameData = '',
    [string]$Branch = 'main',
    [string]$Version = '',
    [int]$MaxHunts = 0,
    [int]$PublicPort = 0,
    [switch]$SkipInstall,
    [switch]$SkipGame,
    [switch]$NoKey
)

$ErrorActionPreference = 'Stop'
$PinnedExeSha256 = 'F3F95FBF22709EF077A11632ED95D675B7209B7E0172D9ECB9BA7D02A1FBB66B'
$ExeRelative = 'Archon\Binaries\Win64\Dauntless-Win64-Shipping.exe'
$ServerHome = '/home/reforged'
$Repository = 'SilentSpud/Dauntless-Reforged-Revival'
$ScriptUrl = "https://raw.githubusercontent.com/$Repository/$Branch/legacy/ReforgedHost/linux/setup-vps.sh"

function Write-Step([string]$Text) {
    Write-Host ''
    Write-Host "== $Text" -ForegroundColor Cyan
}

function Stop-Deploy([string]$Text) {
    Write-Host ''
    Write-Host "Error: $Text" -ForegroundColor Red
    exit 1
}

# One word for the server's shell: single-quoted. Never a double quote in what goes to ssh (Windows
# PowerShell 5.1 does not escape those on a native command line).
function ConvertTo-ShWord([string]$Text) {
    return "'" + $Text.Replace("'", "'\''") + "'"
}

# ------------------------------------------------------------------ checks

if ($Server -notmatch '^[A-Za-z0-9.:-]{1,253}$') { Stop-Deploy "-Server must be an IP address or a DNS name (got $Server)" }
if ($User -notmatch '^[a-z_][a-z0-9_-]{0,31}$') { Stop-Deploy "-User must be a plain user name like root (got $User)" }
if ($Branch -notmatch '^[A-Za-z0-9._/-]{1,100}$') { Stop-Deploy "-Branch must be a branch name (got $Branch)" }
if ($Version -and $Version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$') { Stop-Deploy "-Version must be a version like 0.3.0 (got $Version)" }
if ($Name -match '["\x00-\x1f]' -or $Name.Trim().Length -eq 0 -or $Name.Length -gt 48) { Stop-Deploy '-Name must be 1 to 48 characters, without double quotes' }
if ($MaxHunts -ne 0 -and ($MaxHunts -lt 1 -or $MaxHunts -gt 10)) { Stop-Deploy '-MaxHunts must be 1 to 10' }
if ($PublicPort -ne 0 -and ($PublicPort -lt 1 -or $PublicPort -gt 65535)) { Stop-Deploy '-PublicPort must be a TCP port from 1 to 65535' }
$PlayersPort = 443
if ($PublicPort -ne 0) { $PlayersPort = $PublicPort }
# The game ports: 8770-8777 up to 6 hunts, one more port below 8770 for each hunt above 6 (8766-8777 at 10).
$GamePorts = '8770-8777'
if ($MaxHunts -gt 6) { $GamePorts = "$(8776 - $MaxHunts)-8777" }
foreach ($Tool in @('ssh', 'scp', 'ssh-keygen')) {
    if (-not (Get-Command $Tool -ErrorAction SilentlyContinue)) {
        Stop-Deploy "$Tool was not found: turn on Windows' OpenSSH Client (Settings > System > Optional features > Add a feature > OpenSSH Client), then open a new PowerShell"
    }
}

$Target = "$User@$Server"
$Sudo = ''
if ($User -ne 'root') { $Sudo = 'sudo ' }
$SshOptions = @('-p', "$Port", '-o', 'ServerAliveInterval=30', '-o', 'ServerAliveCountMax=10')
$ScpOptions = @('-P', "$Port", '-o', 'ServerAliveInterval=30', '-o', 'ServerAliveCountMax=10')

# A command on the server; stops here when it fails. -Tty for one that asks something (sudo).
function Invoke-Remote([string]$Command, [switch]$Tty) {
    $Arguments = @() + $SshOptions
    if ($Tty) { $Arguments += '-t' }
    $Arguments += @($Target, $Command)
    & ssh @Arguments
    if ($LASTEXITCODE -ne 0) { Stop-Deploy "the command on the server failed (exit $LASTEXITCODE)" }
}

# A command's output, or $null when it fails. Never asks anything.
function Get-RemoteOutput([string]$Command) {
    $Previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $Out = & ssh @SshOptions -o BatchMode=yes -o ConnectTimeout=20 $Target $Command 2>$null
        if ($LASTEXITCODE -ne 0) { return $null }
        return ($Out -join "`n").Trim()
    } finally {
        $ErrorActionPreference = $Previous
    }
}

# Sends one file or folder. From its parent folder, by name: scp could read C:\... as a host called C.
function Send-Item([string]$Source, [string]$Destination, [switch]$Recurse) {
    $Arguments = @() + $ScpOptions
    if ($Recurse) { $Arguments += '-r' }
    $Arguments += @((Split-Path -Leaf $Source), "${Target}:$Destination")
    Push-Location -LiteralPath (Split-Path -Parent $Source)
    try { & scp @Arguments }
    finally { Pop-Location }
    if ($LASTEXITCODE -ne 0) { Stop-Deploy "sending $Source failed (exit $LASTEXITCODE)" }
}

# What makes two copies of the game the same here: the files the game ships with. Not the runtime and
# switches the server adds next to the executable, nor Archon\Saved (settings and logs of whoever played).
function Test-GameFile([string]$RelativePath) {
    $Leaf = Split-Path -Leaf $RelativePath
    if ($RelativePath -match '^Archon[\\/]Saved([\\/]|$)') { return $false }
    if ($Leaf -in @('MysticParadox.dll', 'winmm.dll') -or $Leaf -like '*.flag' -or $Leaf -like '*.rr-old-*') { return $false }
    return $true
}

# "<size> <path>" of each game file (Test-GameFile), sorted byte-wise, one per line, then SHA-256: the
# server computes the same for the same files.
function Get-GameListing {
    $Lines = New-Object 'System.Collections.Generic.List[string]'
    foreach ($File in $AllFiles) {
        $Relative = $File.FullName.Substring($GameDir.Length).TrimStart('\', '/').Replace('\', '/')
        if (Test-GameFile $Relative) { $Lines.Add(("{0} {1}" -f $File.Length, $Relative)) }
    }
    $Lines.Sort([StringComparer]::Ordinal)
    $Bytes = [Text.Encoding]::UTF8.GetBytes((($Lines -join "`n") + "`n"))
    $Sha = [Security.Cryptography.SHA256]::Create()
    try { return (-join ($Sha.ComputeHash($Bytes) | ForEach-Object { $_.ToString('x2') })) }
    finally { $Sha.Dispose() }
}

Write-Step 'Your game'
$GameDir = $null
if (Test-Path -LiteralPath $Game -PathType Container) {
    foreach ($Candidate in @($Game, (Join-Path $Game 'Dauntless'))) {
        if (Test-Path -LiteralPath (Join-Path $Candidate $ExeRelative)) { $GameDir = (Resolve-Path -LiteralPath $Candidate).Path; break }
    }
}
if (-not $GameDir) { Stop-Deploy "no $ExeRelative in ${Game}: pass -Game with your Dauntless 1.14.7 folder (the one with Archon inside)" }
Write-Host "folder: $GameDir"
Write-Host 'checking the executable (SHA-256)...'
$ExeHash = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $GameDir $ExeRelative)).Hash
if ($ExeHash -ne $PinnedExeSha256) { Stop-Deploy "this is not Dauntless 1.14.7 (the executable's SHA-256 is $ExeHash). Install 1.14.7 with Legendary: docs/players.md" }
Write-Host 'Dauntless 1.14.7: OK'
$AllFiles = @(Get-ChildItem -LiteralPath $GameDir -Recurse -File -Force)
$GameBytes = ($AllFiles | Measure-Object -Property Length -Sum).Sum
Write-Host ("{0} files, {1:N1} GB" -f $AllFiles.Count, ($GameBytes / 1GB))

# A Reforged project on this PC: its published runtime and newest game data pack go along.
# [IO.Path]::Combine, not Join-Path: Join-Path fails on a drive this PC does not have (D:).
$Workspaces = @([IO.Path]::Combine((Split-Path -Parent $GameDir), 'Reforged', '.workspace'), 'C:\Reforged\.workspace', 'D:\Reforged\.workspace')
if (-not $Runtime) {
    foreach ($W in $Workspaces) {
        $R = [IO.Path]::Combine($W, 'runtime')
        if ((Test-Path -LiteralPath ([IO.Path]::Combine($R, 'MysticParadox.dll'))) -and (Test-Path -LiteralPath ([IO.Path]::Combine($R, 'winmm.dll')))) { $Runtime = $R; break }
    }
}
if ($Runtime) {
    if (-not (Test-Path -LiteralPath $Runtime -PathType Container)) { Stop-Deploy "-Runtime ${Runtime}: no such folder" }
    foreach ($Dll in @('MysticParadox.dll', 'winmm.dll')) {
        if (-not (Test-Path -LiteralPath (Join-Path $Runtime $Dll))) { Stop-Deploy "-Runtime $Runtime has no $Dll" }
    }
    Write-Host "runtime: $Runtime"
} else {
    Write-Host 'runtime: none on this PC (players can join but not play until you send one: docs/hosting-linux.md, step 6)' -ForegroundColor Yellow
}
if (-not $GameData) {
    foreach ($W in $Workspaces) {
        $Packs = [IO.Path]::Combine($W, 'gamedata-packs')
        if (-not (Test-Path -LiteralPath $Packs)) { continue }
        $Pack = Get-ChildItem -LiteralPath $Packs -Filter '*.json.gz' -File | Sort-Object LastWriteTime -Descending | Select-Object -First 1
        if ($Pack) { $GameData = $Pack.FullName; break }
    }
}
if ($GameData) {
    if (-not (Test-Path -LiteralPath $GameData -PathType Leaf)) { Stop-Deploy "-GameData ${GameData}: no such file" }
    Write-Host "game data pack: $GameData"
} else {
    Write-Host 'game data pack: none (the server uses the project preset, and examples for the two stores)' -ForegroundColor Yellow
}

# ------------------------------------------------------------------ can this PC reach the VPS?

Write-Step "Reaching $Server on port $Port (SSH)"
$Reached = $false
$Client = New-Object Net.Sockets.TcpClient
try {
    $Attempt = $Client.BeginConnect($Server, $Port, $null, $null)
    if ($Attempt.AsyncWaitHandle.WaitOne(15000) -and $Client.Connected) { $Reached = $true }
} catch {
    $Reached = $false
} finally {
    $Client.Close()
}
if (-not $Reached) {
    Write-Host "no answer from ${Server}:$Port within 15 seconds. Usually one of these:" -ForegroundColor Yellow
    Write-Host "  - the VPS firewall (its Firewall page in Hostinger's hPanel) drops SSH: add a rule Accept, TCP, port $Port, from any," -ForegroundColor Yellow
    Write-Host "    next to TCP $PlayersPort and UDP $GamePorts, and make sure that firewall is the one active for this VPS" -ForegroundColor Yellow
    Write-Host '  - the VPS is stopped or still installing (hPanel > VPS > Overview), or the address is not its own' -ForegroundColor Yellow
    Write-Host "  - this network blocks outgoing SSH (some work, school and public networks): try another one, a phone hotspot" -ForegroundColor Yellow
    Write-Host "Check again with: Test-NetConnection $Server -Port $Port" -ForegroundColor Yellow
    Stop-Deploy "cannot reach ${Server}:$Port"
}
Write-Host 'the VPS answers'

# ------------------------------------------------------------------ SSH key

if (-not $NoKey) {
    Write-Step "Signing in to $Target"
    if ($null -ne (Get-RemoteOutput 'true')) {
        Write-Host 'your SSH key works: no password needed'
    } else {
        $KeyFile = Join-Path $env:USERPROFILE '.ssh\id_ed25519'
        if (-not (Test-Path -LiteralPath "$KeyFile.pub")) {
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent $KeyFile) | Out-Null
            # One string, passed as is: an empty passphrase survives every PowerShell version.
            Start-Process -FilePath 'ssh-keygen' -ArgumentList "-q -t ed25519 -N `"`" -f `"$KeyFile`"" -NoNewWindow -Wait
            if (-not (Test-Path -LiteralPath "$KeyFile.pub")) { Stop-Deploy 'could not make an SSH key (ssh-keygen)' }
            Write-Host "made an SSH key for this PC: $KeyFile"
        }
        $Fields = (Get-Content -Raw -LiteralPath "$KeyFile.pub").Trim() -split '\s+'
        $KeyLine = ConvertTo-ShWord ("{0} {1}" -f $Fields[0], $Fields[1])
        Write-Host "type the VPS password for $User once, to add this PC's key:"
        Invoke-Remote "umask 077; mkdir -p ~/.ssh; touch ~/.ssh/authorized_keys; grep -qxF $KeyLine ~/.ssh/authorized_keys || echo $KeyLine >> ~/.ssh/authorized_keys"
        if ($null -eq (Get-RemoteOutput 'true')) { Write-Host 'the key did not work: you will be asked for the password at each step' -ForegroundColor Yellow }
        else { Write-Host 'key added: no more passwords' }
    }
}

# ------------------------------------------------------------------ 1. install on the VPS

$Upload = 'reforged-upload'
Invoke-Remote "mkdir -p ~/$Upload"
if (-not $SkipInstall) {
    Write-Step 'Installing the server on the VPS (packages, MongoDB, the server release; 5 to 15 minutes the first time)'
    $Local = Join-Path $PSScriptRoot 'setup-vps.sh'
    $Script = Join-Path $env:TEMP 'reforged-setup-vps.sh'
    if (Test-Path -LiteralPath $Local) {
        $Text = [IO.File]::ReadAllText($Local)
    } else {
        Write-Host "downloading $ScriptUrl"
        $Text = (Invoke-WebRequest -UseBasicParsing -Uri $ScriptUrl).Content
    }
    # LF only: a CRLF checkout would break the shell script.
    [IO.File]::WriteAllText($Script, $Text.Replace("`r`n", "`n"), (New-Object Text.UTF8Encoding($false)))
    Send-Item $Script "$Upload/setup-vps.sh"
    # The address this PC reaches the VPS at is the one players use.
    $Options = "--install-only --host $(ConvertTo-ShWord $Server) --name $(ConvertTo-ShWord $Name) --repo $(ConvertTo-ShWord $Repository)"
    if ($Version) { $Options += " --version $(ConvertTo-ShWord $Version)" }
    if ($MaxHunts -ne 0) { $Options += " --max-hunts $MaxHunts" }
    if ($PublicPort -ne 0) { $Options += " --public-port $PublicPort" }
    Invoke-Remote "${Sudo}sh ~/$Upload/setup-vps.sh $Options" -Tty
}

# ------------------------------------------------------------------ 2. the game

$Finish = "${Sudo}sh $ServerHome/Reforged/legacy/ReforgedHost/linux/setup-vps.sh --finish"
if (-not $SkipGame) {
    Write-Step 'Sending the game'
    $There = $null
    if ($User -eq 'root') {
        # Get-GameListing, on the server (the same files left out).
        $There = Get-RemoteOutput "g=$ServerHome/games/Dauntless; [ -f `$g/Archon/Binaries/Win64/Dauntless-Win64-Shipping.exe ] && cd `$g && find . -path ./Archon/Saved -prune -o -type f ! -name MysticParadox.dll ! -name winmm.dll ! -name '*.flag' ! -name '*.rr-old-*' -printf '%s %P\n' | LC_ALL=C sort | sha256sum | cut -c1-64"
    }
    $Wanted = Get-GameListing
    if ($There -eq $Wanted) {
        Write-Host 'the VPS already has this game (same files, same sizes): not sending it again'
    } else {
        Write-Host ("about {0:N1} GB: this takes a while (an hour at 35 Mbit/s upload). Keep this window open." -f ($GameBytes / 1GB))
        Invoke-Remote "rm -rf ~/$Upload/Dauntless"
        Send-Item $GameDir "$Upload/Dauntless" -Recurse
        $Finish += " --game-from ~/$Upload/Dauntless"
    }
}

# ------------------------------------------------------------------ 3. runtime and game data

if ($Runtime) {
    Write-Step 'Sending the runtime'
    Invoke-Remote "rm -rf ~/$Upload/runtime; mkdir -p ~/$Upload/runtime"
    Send-Item (Join-Path $Runtime 'MysticParadox.dll') "$Upload/runtime/MysticParadox.dll"
    Send-Item (Join-Path $Runtime 'winmm.dll') "$Upload/runtime/winmm.dll"
    $Finish += " --runtime ~/$Upload/runtime"
}
if ($GameData) {
    Write-Step 'Sending the game data pack'
    Send-Item $GameData "$Upload/gamedata.json.gz"
    $Finish += " --gamedata ~/$Upload/gamedata.json.gz"
}

# ------------------------------------------------------------------ 4. start it

Write-Step 'Starting the server'
Invoke-Remote $Finish -Tty
Invoke-Remote "rm -rf ~/$Upload"

Write-Step 'Done'
Write-Host 'Send each friend an invite (the first one is above). For more, on the VPS:'
Write-Host '  sudo -iu reforged'
Write-Host '  ~/Reforged/bin/reforged-host invite --for "Alex"'
Write-Host ''
Write-Host "If players cannot join: allow inbound TCP $PlayersPort and UDP $GamePorts in the VPS's Firewall page in Hostinger's hPanel." -ForegroundColor Yellow
Write-Host "If the launcher says the certificate does not match, another program (a hosting panel) answers on port ${PlayersPort}: run this again with -PublicPort 4443 -SkipGame, allow TCP 4443 in the firewall, and use the new invite." -ForegroundColor Yellow
