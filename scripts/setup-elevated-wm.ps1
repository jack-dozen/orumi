<#
.SYNOPSIS
Installs, updates, checks, or removes the elevated Scheduled Task for vscroll-wm.

.DESCRIPTION
Keeps ORUMI (Tauri / WebView2) running as a standard non-admin user while running
vscroll-wm.exe elevated ("Run with highest privileges") from an admin-protected
Program Files directory so both ORUMI and vscroll-wm can control admin windows
without code-signing certificates or UAC prompts on every launch.
#>
[CmdletBinding()]
param(
  [ValidateSet("Install", "Uninstall", "Status")]
  [string]$Action = "Install",
  [string]$SourceExe = "",
  [string]$TargetDir = "C:\Program Files\ORUMI",
  [string]$TaskName = "ORUMI-vscroll-wm",
  [string]$UserSid = "",
  [switch]$SelfElevate
)

$ErrorActionPreference = "Stop"

function Test-IsAdmin {
  $id = [Security.Principal.WindowsIdentity]::GetCurrent()
  $principal = New-Object Security.Principal.WindowsPrincipal($id)
  return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Resolve-TargetUserSid {
  param([string]$ExplicitSid)
  if (-not [string]::IsNullOrWhiteSpace($ExplicitSid)) {
    return $ExplicitSid.Trim()
  }
  return [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
}

function Resolve-SourceExe {
  param([string]$ExplicitPath)
  if (-not [string]::IsNullOrWhiteSpace($ExplicitPath)) {
    if (-not (Test-Path -LiteralPath $ExplicitPath -PathType Leaf)) {
      throw "Source binary not found: $ExplicitPath"
    }
    return (Resolve-Path -LiteralPath $ExplicitPath).Path
  }
  $root = Split-Path -Parent $PSScriptRoot
  $candidates = @(
    (Join-Path $root "src-tauri\binaries\vscroll-wm-x86_64-pc-windows-msvc.exe"),
    (Join-Path $root "vscroll-wm\target\release\vscroll-wm.exe"),
    (Join-Path $root "vscroll-wm\target\debug\vscroll-wm.exe")
  )
  foreach ($c in $candidates) {
    if (Test-Path -LiteralPath $c -PathType Leaf) {
      return $c
    }
  }
  throw "Could not find vscroll-wm.exe. Build it first or pass -SourceExe."
}

function Escape-Xml {
  param([string]$Value)
  return [System.Security.SecurityElement]::Escape($Value)
}

$targetExe = Join-Path $TargetDir "vscroll-wm.exe"

if ($Action -eq "Status") {
  $exeOk = Test-Path -LiteralPath $targetExe -PathType Leaf
  $taskOk = $false
  try {
    & schtasks.exe /Query /TN $TaskName *> $null
    $taskOk = ($LASTEXITCODE -eq 0)
  } catch {
    $taskOk = $false
  }
  [PSCustomObject]@{
    TaskName  = $TaskName
    TaskReady = $taskOk
    TargetExe = $targetExe
    ExeExists = $exeOk
  }
  exit ($(if ($exeOk -and $taskOk) { 0 } else { 1 }))
}

if (-not (Test-IsAdmin)) {
  if ($SelfElevate) {
    $resolvedSid = Resolve-TargetUserSid -ExplicitSid $UserSid
    $argList = @(
      "-NoProfile",
      "-ExecutionPolicy", "Bypass",
      "-File", "`"$PSCommandPath`"",
      "-Action", $Action,
      "-TargetDir", "`"$TargetDir`"",
      "-TaskName", "`"$TaskName`"",
      "-UserSid", "`"$resolvedSid`""
    )
    if ($Action -eq "Install") {
      $resolvedSrc = Resolve-SourceExe -ExplicitPath $SourceExe
      $argList += @("-SourceExe", "`"$resolvedSrc`"")
    }
    $proc = Start-Process -FilePath "powershell.exe" -Verb RunAs -Wait -PassThru -ArgumentList $argList
    exit $proc.ExitCode
  }
  throw "Administrator privileges are required for Action=$Action. Re-run as Administrator or pass -SelfElevate."
}

$sid = Resolve-TargetUserSid -ExplicitSid $UserSid

if ($Action -eq "Uninstall") {
  try {
    & schtasks.exe /End /TN $TaskName *> $null
  } catch {}
  try {
    & schtasks.exe /Delete /TN $TaskName /F *> $null
  } catch {}
  if (Test-Path -LiteralPath $targetExe -PathType Leaf) {
    Remove-Item -LiteralPath $targetExe -Force
  }
  if ((Test-Path -LiteralPath $TargetDir -PathType Container) -and
      ((Get-ChildItem -LiteralPath $TargetDir -Force | Measure-Object).Count -eq 0)) {
    Remove-Item -LiteralPath $TargetDir -Force
  }
  Write-Output "Removed scheduled task '$TaskName' and '$targetExe'."
  exit 0
}

$src = Resolve-SourceExe -ExplicitPath $SourceExe

# Stop running elevated instances so replacing the binary in Program Files succeeds cleanly.
try {
  & schtasks.exe /End /TN $TaskName *> $null
} catch {}
Get-Process -Name "vscroll-wm" -ErrorAction SilentlyContinue | Where-Object {
  $_.Path -eq $targetExe
} | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 150

New-Item -ItemType Directory -Force -Path $TargetDir | Out-Null
Copy-Item -LiteralPath $src -Destination $targetExe -Force

$escapedExe = Escape-Xml $targetExe
$escapedSid = Escape-Xml $sid
$taskXml = @"
<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.3" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Elevated vscroll-wm helper for ORUMI (allows managing admin windows while ORUMI stays non-admin).</Description>
  </RegistrationInfo>
  <Triggers />
  <Principals>
    <Principal id="Author">
      <UserId>$escapedSid</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <DisallowStartOnRemoteAppSession>false</DisallowStartOnRemoteAppSession>
    <UseUnifiedSchedulingEngine>true</UseUnifiedSchedulingEngine>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>5</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>$escapedExe</Command>
      <Arguments>--no-tray --no-osd --no-cheatsheet --no-overview</Arguments>
    </Exec>
  </Actions>
</Task>
"@

# Grant Full Access to Administrators & SYSTEM, and Read+Execute only to the target user SID
# so non-admin ORUMI can start the task on demand without a UAC prompt and without write access.
$sddl = "D:(A;;FA;;;BA)(A;;FA;;;SY)(A;;GRGX;;;$sid)"

$svc = New-Object -ComObject Schedule.Service
$svc.Connect()
$folder = $svc.GetFolder("\")
$TASK_CREATE_OR_UPDATE = 6
$TASK_LOGON_INTERACTIVE_TOKEN = 3
$null = $folder.RegisterTask($TaskName, $taskXml, $TASK_CREATE_OR_UPDATE, $sid, $null, $TASK_LOGON_INTERACTIVE_TOKEN, $sddl)

Write-Output "Installed elevated task '$TaskName' -> '$targetExe' (user SID: $sid)."
