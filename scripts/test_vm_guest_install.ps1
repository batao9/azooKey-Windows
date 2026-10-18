param([string]$SourceScript = (Join-Path $PSScriptRoot 'vm-guest.ps1'))
$ErrorActionPreference = 'Stop'
$source = (Resolve-Path $SourceScript).Path
$root = Join-Path $env:TEMP ('azookey-install-phase-test-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null
try {
  $programFiles = Join-Path $root 'programs'
  New-Item -ItemType Directory -Path (Join-Path $programFiles 'Azookey') -Force | Out-Null
  $identity = @{version='fixture';channel='validation';revision='fixture';installerGeneration=2} | ConvertTo-Json -Compress
  [IO.File]::WriteAllText((Join-Path $programFiles 'Azookey\build-info.json'), $identity)
  $installer = Join-Path $root 'not-an-installer.txt'
  [IO.File]::WriteAllText($installer, 'Fixture only: must never be executed')
  $sha256 = (Get-FileHash $installer -Algorithm SHA256).Hash
  $cases = @(
    @{mode='editor';code=1;started=$false},
    @{mode='hash';code=1;started=$false},
    @{mode='completed-failure';code=1;started=$true},
    @{mode='timeout';code=124;started=$true},
    @{mode='wait-error';code=124;started=$true},
    @{mode='launch-error';code=124;started=$true},
    @{mode='success';code=0;started=$true}
  )
  foreach ($case in $cases) {
    $started = Join-Path $root ($case.mode + '.started')
    $payload = @{source=$source;mode=$case.mode;started=$started;programFiles=$programFiles;
      appData=(Join-Path $root 'appdata');localAppData=(Join-Path $root 'localappdata');
      installer=$installer;sha256=$sha256;identity=$identity} | ConvertTo-Json -Compress
    $payload64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($payload))
    $code = @'
$ErrorActionPreference = 'Stop'
$c = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('__PAYLOAD__')) | ConvertFrom-Json
$env:ProgramFiles = $c.programFiles
$env:APPDATA = $c.appData
$env:LOCALAPPDATA = $c.localAppData
function Get-Process {
  [CmdletBinding()] param([Parameter(Position=0)][string]$Name)
  if($Name -ne 'frontend'){throw 'Unexpected process query in fixture'}
  if($c.mode -eq 'editor'){[pscustomobject]@{Id=1}}
}
function Start-Process {
  [CmdletBinding()] param([string]$FilePath, [object]$ArgumentList, [switch]$PassThru)
  [IO.File]::WriteAllText($c.started, 'mock process only')
  if($c.mode -eq 'launch-error'){throw 'Controlled ambiguous launch error'}
  $exitCode = if($c.mode -eq 'completed-failure'){5}else{0}
  $p = [pscustomobject]@{Handle=0;ExitCode=$exitCode;Mode=$c.mode}
  Add-Member -InputObject $p -MemberType ScriptMethod -Name WaitForExit -Value {
    param($milliseconds)
    if($this.Mode -eq 'timeout'){return $false}
    if($this.Mode -eq 'wait-error'){throw 'Controlled wait error'}
    return $true
  }
  return $p
}
$hash = if($c.mode -eq 'hash'){'wrong-hash'}else{$c.sha256}
$global:LASTEXITCODE = 0
& $c.source -Action install -Installer $c.installer -Sha256 $hash -Expected $c.identity
exit $LASTEXITCODE
'@
    $code = $code.Replace('__PAYLOAD__', $payload64)
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($code))
    $ErrorActionPreference = 'Continue'
    $output = & powershell.exe -NoProfile -ExecutionPolicy Bypass -EncodedCommand $encoded 2>&1
    $actual = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    if($actual -ne $case.code){throw ("Unexpected guest install phase exit for " + $case.mode + ': ' + $actual + '; ' + ($output | Out-String))}
    if((Test-Path $started) -ne $case.started){throw ('Wrong process-start boundary: '+$case.mode)}
    if($case.mode -eq 'success' -and ($output | Out-String) -notmatch '"installer_exit":0'){
      throw 'Completed install did not return its known-success receipt'
    }
  }
  @{task_status='succeeded';cases=7;installer_processes_mocked=$true;real_installers_started=0} | ConvertTo-Json -Compress
} finally {
  $ErrorActionPreference = 'Stop'
  Remove-Item -LiteralPath $root -Recurse -Force
}
