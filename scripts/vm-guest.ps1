param(
    [Parameter(Mandatory=$true)][ValidateSet('doctor','prepare','identity','install')][string]$Action,
    [string]$RustVersion,
    [string]$HostUtc,
    [string]$Installer,
    [string]$Sha256,
    [string]$Expected
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = New-Object Text.UTF8Encoding($false)
$env:Path += ";$env:USERPROFILE\.cargo\bin"
Add-Type -TypeDefinition 'using System.Runtime.InteropServices; public static class VmAwake { [DllImport("kernel32.dll")] public static extern uint SetThreadExecutionState(uint flags); }'
if([VmAwake]::SetThreadExecutionState([uint32]2147483649) -eq 0){throw 'Sleep prevention failed'}
function Identity {
    $file = Join-Path $env:ProgramFiles 'Azookey\build-info.json'
    if(Test-Path $file){return Get-Content $file -Raw -Encoding UTF8 | ConvertFrom-Json}
    return $null
}
function Probe-Tool {
    param([string]$Name, [string[]]$Arguments)
    $command = Get-Command $Name -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if(!$command){return @{ready=$false; version=$null}}
    $ErrorActionPreference = 'Continue'
    $version = & $command.Source @Arguments 2>$null
    $code = $LASTEXITCODE
    return @{ready=($code -eq 0 -and !!$version); version=$version}
}
try {
    if($Action -eq 'identity') {
        @{installed=Identity} | ConvertTo-Json -Depth 6 -Compress
    } elseif($Action -eq 'install') {
        $completionKnown = $true
        try {
        $principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
        if(!$principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)){throw 'Installation requires elevated same-user SSH, not a Limited interactive task'}
        if(@(Get-Process frontend -ErrorAction SilentlyContinue).Count){throw 'Close settings editor before install; drafts must not be discarded'}
        if((Get-FileHash $Installer -Algorithm SHA256).Hash -ne $Sha256){throw 'Transferred installer SHA256 mismatch'}
        $wanted = $Expected | ConvertFrom-Json
        $settings = Join-Path $env:APPDATA 'Azookey\settings.json'
        $backup = Join-Path $env:LOCALAPPDATA ('Azookey\vm-backup-'+[guid]::NewGuid().ToString('N'))
        $hashBefore = $null
        if(Test-Path $settings){
            New-Item -ItemType Directory -Path $backup | Out-Null
            Copy-Item $settings (Join-Path $backup 'settings.json')
            $hashBefore = (Get-FileHash $settings).Hash
        }
        $before = Identity
        $log = $Installer + '.log'
        $completionKnown = $false
        $p = Start-Process -FilePath $Installer -ArgumentList @('/SP-','/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART','/RESTARTEXITCODE=3010',('/LOG="'+$log+'"')) -PassThru
        $null = $p.Handle
        if(!$p.WaitForExit(1200000)){
            [Console]::Error.WriteLine('Installer timeout; inspect running installer before retry')
            exit 124
        }
        $completionKnown = $true
        if($p.ExitCode -notin @(0,3010)){throw ('Installer exit '+$p.ExitCode+'; log '+$log)}
        $installed = Identity
        foreach($key in @('version','channel','revision','installerGeneration')){
            if($installed.$key -ne $wanted.$key){throw ('Installed identity mismatch: '+$key)}
        }
        $hashAfter = if(Test-Path $settings){(Get-FileHash $settings).Hash}else{$null}
        if($hashBefore -and !$hashAfter){throw ('Existing settings disappeared; backup retained at '+$backup)}
        @{installed=$installed; before=$before; installer_exit=$p.ExitCode; reboot_required=($p.ExitCode -eq 3010); installer_sha256=$Sha256;
          runtime_verification='not_checked';
          settings_bytes_unchanged=($hashBefore -eq $hashAfter); settings_backup=$(if($hashBefore){$backup}else{$null});
          credential_preservation='not_checked'; install_log=$log} | ConvertTo-Json -Depth 6 -Compress
        } catch {
            if(!$completionKnown){
                [Console]::Error.WriteLine('Installer completion is unknown: ' + $_.Exception.Message)
                exit 124
            }
            throw
        }
    } else {
        $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
        $vs = if(Test-Path $vswhere){& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath}else{$null}
        if($Action -eq 'prepare'){
            if(!$RustVersion){throw 'Pinned Rust version is required'}
            if(!(Get-Command rustup -CommandType Application -ErrorAction SilentlyContinue)){throw 'rustup is not installed; provision it before preparing the pinned toolchain'}
            if($HostUtc){Set-Date -Date ([DateTime]::Parse($HostUtc).ToUniversalTime().ToLocalTime()) | Out-Null}
            # Provision only the designated build VM; guest maintenance output goes to stderr.
            $ErrorActionPreference = 'Continue'
            & rustup toolchain install $RustVersion --profile minimal --component clippy --component rustfmt --target i686-pc-windows-msvc --no-self-update 2>&1 | ForEach-Object {[Console]::Error.WriteLine($_)}
            $installExit = $LASTEXITCODE
            $ErrorActionPreference = 'Stop'
            if($installExit -ne 0){throw 'Pinned Rust installation failed; inspect TLS and clock without disabling verification'}
        }
        $rust = Probe-Tool rustup @('run', $RustVersion, 'rustc', '--version')
        $swift = Probe-Tool swift @('--version')
        $node = Probe-Tool node @('--version')
        $power = (& powercfg /query SCHEME_CURRENT SUB_SLEEP STANDBYIDLE) -join "`n"
        if($LASTEXITCODE -ne 0){throw 'Unable to query sleep policy'}
        @{installed=Identity; utc=(Get-Date).ToUniversalTime().ToString('o'); rust=$rust.version; rust_ready=$rust.ready;
          swift=($swift.version -join ' '); swift_ready=$swift.ready; node=$node.version; node_ready=$node.ready;
          visual_studio=$vs; sleep_policy=$power; interactive_sessions=@(Get-Process explorer -ErrorAction SilentlyContinue|Select-Object -ExpandProperty SessionId)} | ConvertTo-Json -Depth 6 -Compress
    }
} finally {
    [void][VmAwake]::SetThreadExecutionState([uint32]2147483648)
}
