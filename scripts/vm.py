#!/usr/bin/env python3
"""Single-owner VirtualBox/Windows operations. Machine values live in .local/vm.json."""
import argparse
import base64
import contextlib
import datetime as dt
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time
import uuid

ROOT = Path(__file__).resolve().parent.parent


class VmError(RuntimeError):
    def __init__(self, message, status='blocked', uncertain=False):
        super().__init__(message)
        self.status = status
        self.uncertain = uncertain


def ps_quote(value):
    return "'" + str(value).replace("'", "''") + "'"


def load_profile(config, name):
    data = json.loads(Path(config).read_text())
    profile = {**data.get('defaults', {}), **data['profiles'][name]}
    for key in ('vm', 'host', 'user', 'port', 'key', 'vbox'):
        if not profile.get(key):
            raise VmError(f'Missing profile value: {key}')
    if not re.fullmatch(r'[A-Za-z0-9_.-]+', profile['user']):
        raise VmError('SSH user must be an explicit Windows account name')
    if not 1 <= int(profile['port']) <= 65535:
        raise VmError('Invalid SSH port')
    for key in ('key', 'vbox'):
        profile[key] = os.path.expandvars(os.path.expanduser(profile[key]))
    if not Path(profile['key']).is_file():
        raise VmError('Configured SSH key does not exist')
    return profile


def installer_artifact(installer):
    installer = Path(installer).resolve()
    if not installer.is_file():
        raise VmError('Installer file not found; select an exact artifact')
    metadata = installer.with_suffix('.build-info.json')
    if not metadata.exists():
        metadata = installer.parent / 'build-info.json'
    expected = json.loads(metadata.read_text())
    if not isinstance(expected, dict):
        raise VmError('Build identity metadata must be a JSON object')
    for key in ('version', 'channel', 'revision', 'installerGeneration'):
        if key not in expected:
            raise VmError(f'Missing build identity: {key}')
    for key in ('version', 'channel', 'revision'):
        if not isinstance(expected[key], str) or not expected[key].strip():
            raise VmError(f'Invalid build identity: {key}')
    if type(expected['installerGeneration']) is not int or expected['installerGeneration'] < 0:
        raise VmError('Invalid build identity: installerGeneration')
    with installer.open('rb') as stream:
        digest = hashlib.file_digest(stream, 'sha256').hexdigest()
    return installer, expected, digest


def script_artifact(source, directory):
    # Decode before touching even the run directory; PS5.1 needs a UTF-8 BOM.
    content = Path(source).read_text(encoding='utf-8-sig')
    token = 'azookey-vm-' + uuid.uuid4().hex
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    normalized = directory / (token + '.ps1')
    normalized.write_text(content, encoding='utf-8-sig')
    return token, normalized


def transfer_artifact(source, destination, download=False):
    remote = source if download else destination
    if not re.match(r'^[A-Za-z]:[\\/]', remote):
        raise VmError('Remote transfer path must be an absolute Windows path')
    remote = '/' + remote.replace('\\', '/')
    local = Path(destination if download else source).resolve()
    if download:
        local.parent.mkdir(parents=True, exist_ok=True)
    elif not local.is_file():
        raise VmError(f'Upload source does not exist: {local}')
    return str(local), remote


def validate_command_profile(profile, command):
    if command == 'prepare' and profile.get('role') != 'build':
        raise VmError('Toolchain preparation requires a build profile')
    if command in ('build', 'test') and profile.get('role') != 'build':
        raise VmError('Build/test commands require a build profile')
    if command in ('stage', 'install') and profile.get('role') not in ('manual', 'disposable'):
        raise VmError('Installation requires a manual or disposable profile')


class VM:
    def __init__(self, profile, directory):
        self.p = profile
        self.directory = Path(directory)
        self.directory.mkdir(parents=True, exist_ok=True)
        self.log = self.directory / 'commands.log'
        self.remote = rf'C:\Users\{profile["user"]}\AppData\Local\Temp'
        self.commands = []
        self.reservation = None
        self.resets = []
        self.evidence = []
        self.session_mode = 'verification'

    def begin_operation(self, operation, **details):
        if self.reservation is not None:
            value = {'operation': operation, 'run_directory': str(self.directory),
                     'workspace': str(ROOT), 'mode': self.session_mode, **details}
            temporary = self.reservation.with_suffix('.tmp')
            temporary.write_text(json.dumps(value, ensure_ascii=False, indent=2) + '\n')
            temporary.replace(self.reservation)

    def finish_operation(self):
        if self.reservation is not None:
            self.reservation.unlink(missing_ok=True)

    def operation_status(self):
        pending = json.loads(self.reservation.read_text()) if self.reservation.exists() else None
        value = {'pending': pending}
        state = self.state()['VMState']
        if state == 'running':
            tasks = '$t=@(); '
            if pending and pending.get('task'):
                tasks = (f"try {{ $t=@(Get-ScheduledTask -TaskName {ps_quote(pending['task'])} -ErrorAction Stop | "
                         "Where-Object {$_.State -in @('Running','Queued')} | Select-Object TaskName,State) } "
                         "catch { if($_.CategoryInfo.Category -eq [Management.Automation.ErrorCategory]::ObjectNotFound){$t=@()}else{throw} }; ")
            value['guest'] = json.loads(self.ps(
                "$p=@(Get-Process -ErrorAction SilentlyContinue | Where-Object {"
                "$_.Id -ne $PID -and $_.ProcessName -match '^(cargo(-.*)?|rustc|rustup|swift.*|node|msbuild|cl|link|powershell|pwsh|azookey-install-.*|setup.*)$'"
                "} | Select-Object ProcessName,Id,Path,StartTime); "
                + tasks + "@{processes=$p;running_tasks=$t} | ConvertTo-Json -Depth 4 -Compress",
                timeout=30 if pending and pending.get('task') else 15).lstrip('\ufeff'))
        elif state != 'poweroff':
            raise VmError(f'Cannot inspect unresolved guest work during VM state {state}')
        return value

    def remove_inactive_task(self, token):
        query = (f"try {{ $t=Get-ScheduledTask -TaskName {ps_quote(token)} -ErrorAction Stop; [string]$t.State }} "
                 "catch { if($_.CategoryInfo.Category -eq [Management.Automation.ErrorCategory]::ObjectNotFound){'absent'}else{throw} }")
        try:
            state = self.ps(query, timeout=30).lstrip('\ufeff').strip().lower()
            if state == 'absent':
                return
            if state not in ('ready', 'disabled'):
                raise VmError('Interactive task may still start or run', uncertain=True)
            self.ps(f"Unregister-ScheduledTask -TaskName {ps_quote(token)} -Confirm:$false -ErrorAction Stop", timeout=30)
            if self.ps(query, timeout=30).lstrip('\ufeff').strip().lower() != 'absent':
                raise VmError('Interactive task still exists after removal', uncertain=True)
        except (VmError, OSError) as error:
            raise VmError(f'Interactive task removal is unconfirmed: {token}; inspect before retry',
                          uncertain=True) from error

    def run(self, argv, timeout=60, check=True, env=None, idle_timeout=None):
        # Secrets belong in guest credential storage, not command arguments.
        with self.log.open('ab') as log:
            log.write((json.dumps({'command': argv}, ensure_ascii=False) + '\n').encode())
            log.flush()
            try:
                if idle_timeout is None:
                    proc = subprocess.run(argv, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT,
                                          env=env, timeout=timeout, stdin=subprocess.DEVNULL)
                else:
                    proc = subprocess.Popen(argv, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT,
                                            env=env, stdin=subprocess.DEVNULL, start_new_session=True)
                    started = progress = time.monotonic()
                    size = self.log.stat().st_size
                    try:
                        while proc.poll() is None:
                            now = time.monotonic()
                            current_size = self.log.stat().st_size
                            if current_size != size:
                                progress, size = now, current_size
                            if now - started >= timeout or now - progress >= idle_timeout:
                                raise subprocess.TimeoutExpired(argv, min(timeout, idle_timeout))
                            time.sleep(0.2)
                    finally:
                        # Also stop helper children: a surviving SSH process must
                        # not reconnect and write into a freshly restored guest.
                        try:
                            os.killpg(proc.pid, signal.SIGKILL)
                        except ProcessLookupError:
                            pass
                        proc.wait(timeout=5)
            except subprocess.TimeoutExpired as error:
                self.commands.append({'command': argv, 'exit_code': None, 'status': 'timeout'})
                raise VmError('Command timed out; remote work may still be active. Inspect before retry.', uncertain=True) from error
        self.commands.append({'command': argv, 'exit_code': proc.returncode})
        if check and proc.returncode:
            # GNU timeout and shell signal/disconnect statuses cannot establish
            # guest completion. Ambiguous high exit codes stay conservative.
            uncertain = proc.returncode < 0 or proc.returncode in (124, 125) or proc.returncode >= 128
            raise VmError(f'Command exited {proc.returncode}; see {self.log}',
                          'blocked' if uncertain else 'failed', uncertain=uncertain)
        return proc.returncode

    def run_operation(self, operation, command, timeout=60, env=None, **details):
        self.begin_operation(operation, **details)
        try:
            self.run(command, timeout=timeout, env=env,
                     idle_timeout=float(self.p.get('idle_timeout', 120)))
        except VmError as error:
            if not error.uncertain:
                self.finish_operation()
            raise
        except OSError:
            # Failure to launch the local helper cannot leave guest work behind.
            self.finish_operation()
            raise
        self.finish_operation()

    def capture(self, argv, timeout=60, check=True):
        try:
            proc = subprocess.run(argv, cwd=ROOT, capture_output=True, text=True,
                                  encoding='utf-8', errors='replace', timeout=timeout,
                                  stdin=subprocess.DEVNULL)
        except subprocess.TimeoutExpired as error:
            self.commands.append({'command': argv, 'exit_code': None, 'status': 'timeout'})
            with self.log.open('a') as log:
                log.write(json.dumps(self.commands[-1]) + '\n')
            raise VmError('Command timed out; remote work may still be active. Inspect before retry.', uncertain=True) from error
        self.commands.append({'command': argv, 'exit_code': proc.returncode})
        with self.log.open('a') as log:
            log.write(json.dumps({'command': argv, 'exit_code': proc.returncode}, ensure_ascii=False) + '\n')
            log.write(proc.stdout + proc.stderr + '\n')
        if check and proc.returncode:
            uncertain = argv[0] == 'ssh' and (proc.returncode < 0 or proc.returncode in (124, 125) or proc.returncode >= 128)
            raise VmError(f'Command exited {proc.returncode}: {proc.stderr.strip()[:250]}; see {self.log}',
                          'blocked' if uncertain else 'failed', uncertain=uncertain)
        return proc

    def vbox(self, *args):
        return self.capture([self.p['vbox'], *args], timeout=20).stdout

    def state(self):
        for attempt in range(3):
            try:
                raw = self.vbox('showvminfo', self.p['vm'], '--machinereadable')
                break
            except VmError as error:
                # VirtualBox's read-only COM query can briefly lose its object
                # during a power transition. Never repeat a mutating command.
                message = str(error).lower()
                transient = (('object is not ready' in message and '0x80070005' in message) or
                             ('lock request pending' in message and '0x80bb0007' in message))
                if not transient or attempt == 2:
                    raise
                time.sleep(1)
        info = dict(re.findall(r'^([^=]+)="(.*)"\r?$', raw, re.M))
        return {key: info.get(key) for key in ('VMState', 'UUID', 'CurrentSnapshotName', 'CurrentSnapshotUUID')}

    def ssh_args(self):
        return ['ssh', '-i', self.p['key'], '-p', str(self.p['port']), '-o', 'BatchMode=yes',
                '-o', 'StrictHostKeyChecking=accept-new', '-o', 'ConnectTimeout=5',
                '-o', 'ServerAliveInterval=5', '-o', 'ServerAliveCountMax=2',
                f'{self.p["user"]}@{self.p["host"]}']

    def ps(self, code, timeout=15):
        code = "$ErrorActionPreference='Stop'; [Console]::OutputEncoding=New-Object Text.UTF8Encoding($false); " + code
        encoded = base64.b64encode(code.encode('utf-16le')).decode()
        return self.capture(self.ssh_args() + ['powershell -NoProfile -ExecutionPolicy Bypass -EncodedCommand ' + encoded], timeout).stdout

    def ready(self, seconds=90):
        end = time.monotonic() + seconds
        woke = False
        while time.monotonic() < end:
            try:
                if self.capture(self.ssh_args() + ['echo VM_READY'], timeout=min(6, max(0.1, end-time.monotonic())), check=False).returncode == 0:
                    return
            except VmError as error:
                if not error.uncertain:
                    raise
            if not woke:
                # Wake only after a failed probe. Repeated Shift pulses trigger
                # Windows Sticky Keys and must not be sent on every invocation.
                self.vbox('controlvm', self.p['vm'], 'keyboardputscancode', '1d', '9d')
                woke = True
            time.sleep(2)
        raise VmError('SSH readiness deadline exceeded; managed run will reset the baseline')

    def start(self, gui=False, cold=False):
        state = self.state()['VMState']
        if cold and state == 'saved':
            self.begin_operation('start_current', discarded_saved_state=True)
            self.vbox('discardstate', self.p['vm'])
            state = self.state()['VMState']
            if state != 'poweroff':
                raise VmError('Saved memory discard is unconfirmed; current disk preserved', uncertain=True)
        if state in ('poweroff', 'saved', 'aborted'):
            self.vbox('startvm', self.p['vm'], '--type', 'gui' if gui else 'headless')
        elif state != 'running':
            raise VmError(f'Refuse start during VM state {state}')
        self.ready(int(self.p.get('ready_timeout', 90)))
        now = dt.datetime.now(dt.timezone.utc).isoformat()
        self.ps(f"$target=[DateTime]::Parse({ps_quote(now)}).ToUniversalTime(); "
                "if([Math]::Abs(((Get-Date).ToUniversalTime()-$target).TotalSeconds) -gt 30){Set-Date -Date $target.ToLocalTime() | Out-Null}")

    def shutdown(self):
        state = self.state()['VMState']
        if state == 'poweroff':
            return
        if state == 'saved':
            self.start()
        elif state != 'running':
            raise VmError(f'Refuse normal shutdown during VM state {state}')
        # Shutdown/reset must not wait on the SSH connection that just failed.
        self.vbox('controlvm', self.p['vm'], 'acpipowerbutton')
        end = time.monotonic() + int(self.p.get('shutdown_timeout', 10))
        while time.monotonic() < end:
            if self.state()['VMState'] == 'poweroff':
                return
            time.sleep(2)
        raise VmError('Normal shutdown deadline exceeded; reset may power off the disposable guest')

    def shutdown_current(self):
        state = self.state()['VMState']
        if state == 'poweroff':
            return
        if state != 'running':
            raise VmError(f'Preserving shutdown requires running/poweroff, not {state}')
        previous = json.loads(self.reservation.read_text()) if self.reservation is not None and self.reservation.exists() else None
        self.begin_operation('shutdown_current', previous_operation=previous)
        try:
            # One normal request, without /f. Lost acknowledgment is not a
            # reason to replay shutdown or force off a maintenance disk.
            result = self.capture(self.ssh_args() + ['shutdown.exe /s /t 0'], timeout=10, check=False)
        except VmError as error:
            if not error.uncertain:
                raise
        else:
            if result.returncode not in (0, 124, 125, 255) and 0 < result.returncode < 128:
                raise VmError(f'Windows rejected shutdown with exit {result.returncode}', 'failed')
        end = time.monotonic() + int(self.p.get('maintenance_shutdown_timeout', 120))
        while time.monotonic() < end:
            if self.state()['VMState'] == 'poweroff':
                return
            time.sleep(2)
        raise VmError('Preserving shutdown is unconfirmed; inspect current disk before retry', uncertain=True)

    def snapshot(self, name):
        before = self.state()
        if before['VMState'] != 'poweroff':
            raise VmError('Baseline capture requires a powered-off guest')
        self.begin_operation('snapshot', snapshot=name, previous_snapshot=before.get('CurrentSnapshotUUID'))
        self.vbox('snapshot', self.p['vm'], 'take', name, '--description', 'Verified powered-off maintenance baseline')
        after = self.state()
        if (after['VMState'] != 'poweroff' or after.get('CurrentSnapshotName') != name or
                not after.get('CurrentSnapshotUUID') or after['CurrentSnapshotUUID'] == before.get('CurrentSnapshotUUID')):
            raise VmError('New powered-off snapshot identity is unconfirmed; inspect before retry', uncertain=True)
        self.finish_operation()
        return {'name': name, 'uuid': after['CurrentSnapshotUUID'], 'previous_uuid': before.get('CurrentSnapshotUUID')}

    def reset(self, reason, graceful=False, snapshot=None):
        snapshot = snapshot or self.p.get('snapshot')
        if not snapshot or not reason.strip():
            raise VmError('Reset requires a configured baseline snapshot and a reason')
        record = {'reason': reason, 'snapshot': snapshot, 'task_status': 'running'}
        if self.reservation is not None and self.reservation.exists():
            record['interrupted_operation'] = json.loads(self.reservation.read_text())
        self.resets.append(record)
        self.begin_operation('reset', reason=reason, snapshot=snapshot)
        try:
            # Reserve cleanup before even its first query can fail, but validate
            # the target before discarding any current state.
            self.vbox('snapshot', self.p['vm'], 'showvminfo', snapshot)
            state = self.state()['VMState']
            if graceful and state == 'running':
                try:
                    self.shutdown()
                except VmError as error:
                    record['normal_shutdown_error'] = str(error)
                state = self.state()['VMState']
            if state == 'saved':
                self.vbox('discardstate', self.p['vm'])
            elif state in ('running', 'paused'):
                self.vbox('controlvm', self.p['vm'], 'poweroff')
            elif state not in ('poweroff', 'aborted'):
                raise VmError(f'Cannot reset during VM state {state}', uncertain=True)
            self.vbox('snapshot', self.p['vm'], 'restore', snapshot)
            after = self.state()
            if after['VMState'] == 'saved':
                self.vbox('discardstate', self.p['vm'])
                after = self.state()
            if after['VMState'] != 'poweroff' or snapshot.lower() not in (
                    str(after.get('CurrentSnapshotUUID', '')).lower(),
                    str(after.get('CurrentSnapshotName', '')).lower()):
                raise VmError('Baseline restore was not confirmed', uncertain=True)
            record.update(task_status='succeeded', after=after)
            self.finish_operation()
            return record
        except BaseException as error:
            record.update(task_status='failed', error=str(error))
            raise

    @contextlib.contextmanager
    def clean_run(self, keep=False, continuing=False, mode='verification'):
        self.session_mode = mode
        if not continuing:
            self.reset('clean start')
        try:
            yield
        except BaseException as original:
            if mode == 'maintenance':
                # Keep task/process evidence if completion is unknown. A known
                # completed failure may already have cleared its operation.
                if self.reservation is not None and not self.reservation.exists():
                    self.begin_operation('session', last_error=str(original))
                raise
            # Record the original execution failure even if recovery succeeds.
            try:
                self.reset('failed run; discard guest immediately')
            except (VmError, OSError) as recovery:
                raise VmError(f'{original}; baseline recovery also failed: {recovery}', uncertain=True) from original
            raise
        else:
            if keep or mode == 'maintenance':
                self.begin_operation('session', workspace=str(ROOT), mode=mode)
            else:
                self.reset('finished run', graceful=True)

    def recover_poweroff(self, reason):
        return self.reset(reason)

    def transfer(self, source, destination, download=False, timeout=120, prepared=None):
        # OpenSSH Windows accepts /C:/... paths; no remote shell quoting or relative ambiguity.
        local, remote = prepared if prepared is not None else transfer_artifact(source, destination, download)
        endpoint = f'{self.p["user"]}@{self.p["host"]}:{remote}'
        argv = ['scp', '-i', self.p['key'], '-P', str(self.p['port']), '-o', 'BatchMode=yes',
                '-o', 'StrictHostKeyChecking=accept-new', '-o', 'ConnectTimeout=5',
                '-o', 'ServerAliveInterval=5', '-o', 'ServerAliveCountMax=2']
        self.run(argv + ([endpoint, local] if download else [local, endpoint]),
                 timeout=timeout, idle_timeout=timeout)

    def script(self, source, arguments=None, interactive=False, timeout=120, hold_pending=False, prepared=None):
        token, normalized = prepared if prepared is not None else script_artifact(source, self.directory)
        remote = self.remote + '\\' + token + '.ps1'
        self.start()
        self.transfer(str(normalized), remote)
        # PowerShell parameters are quoted as data; not concatenated shell syntax.
        args = ''.join(' -' + key + ' ' + ps_quote(value) for key, value in (arguments or {}).items())
        if interactive:
            return self.interactive(remote, args, token, timeout)
        code = ("Add-Type 'using System.Runtime.InteropServices; public static class RunAwake { [DllImport(\"kernel32.dll\")] public static extern uint SetThreadExecutionState(uint flags); }'; "
                "if([RunAwake]::SetThreadExecutionState([uint32]2147483649) -eq 0){throw 'Sleep prevention failed'}; "
                f"try {{ $global:LASTEXITCODE=0; & {ps_quote(remote)}{args}; $ok=$?; if($LASTEXITCODE -ne 0){{exit $LASTEXITCODE}}; if(!$ok){{exit 1}} }} finally {{ [void][RunAwake]::SetThreadExecutionState([uint32]2147483648) }}")
        self.begin_operation('script', remote_script=remote)
        try:
            output = self.ps(code, timeout=timeout)
        except VmError as error:
            if not error.uncertain:
                self.finish_operation()
            raise
        if not hold_pending:
            self.finish_operation()
        return output

    def interactive(self, remote, args, token, timeout):
        output = remote + '.out'
        done = remote + '.done'
        wrapper = remote + '.wrapper.ps1'
        content = ("$ErrorActionPreference='Stop'; "
                   "Add-Type 'using System.Runtime.InteropServices; public static class TaskAwake { [DllImport(\"kernel32.dll\")] public static extern uint SetThreadExecutionState(uint flags); }'; "
                   "try { if([TaskAwake]::SetThreadExecutionState([uint32]2147483649) -eq 0){throw 'Sleep prevention failed'}; "
                   f"$global:LASTEXITCODE=0; & {ps_quote(remote)}{args} *>&1 | Out-File {ps_quote(output)} -Encoding UTF8; "
                   f"if(!$? -or $LASTEXITCODE -ne 0){{throw 'Guest script failed'}}; [IO.File]::WriteAllText({ps_quote(done)},'0') }} catch {{ "
                   f"$_ | Out-File {ps_quote(output)} -Encoding UTF8; [IO.File]::WriteAllText({ps_quote(done)},'1') }} "
                   "finally { [void][TaskAwake]::SetThreadExecutionState([uint32]2147483648) }")
        encoded = base64.b64encode(content.encode('utf-8-sig')).decode()
        self.begin_operation('interactive', task=token, remote_script=remote, output=output, done=done)
        try:
            self.ps(f"[IO.File]::WriteAllBytes({ps_quote(wrapper)},[Convert]::FromBase64String('{encoded}')); "
                    f"$a=New-ScheduledTaskAction -Execute 'powershell.exe' -Argument ('-NoProfile -ExecutionPolicy Bypass -File \"'+{ps_quote(wrapper)}+'\"'); "
                    f"$p=New-ScheduledTaskPrincipal -UserId ($env:COMPUTERNAME+'\\'+{ps_quote(self.p['user'])}) -LogonType Interactive -RunLevel Limited; "
                    f"$s=New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Seconds {timeout}); "
                    f"Register-ScheduledTask -TaskName '{token}' -Action $a -Principal $p -Settings $s | Out-Null; Start-ScheduledTask -TaskName '{token}'", timeout=30)
        except VmError as error:
            if error.uncertain:
                raise
            self.remove_inactive_task(token)
            self.finish_operation()
            raise
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            response = self.ps(f"if(Test-Path {ps_quote(done)}){{ @{{code=[IO.File]::ReadAllText({ps_quote(done)});output=[IO.File]::ReadAllText({ps_quote(output)})}} | ConvertTo-Json -Compress }}")
            if response.strip():
                value = json.loads(response.lstrip('\ufeff'))
                self.ps(f"Unregister-ScheduledTask -TaskName '{token}' -Confirm:$false", timeout=30)
                self.finish_operation()
                if value['code'] != '0':
                    raise VmError(f'Interactive script failed; see {output} and {self.log}', 'failed')
                return value['output']
            time.sleep(2)
        raise VmError(f'Interactive task {token} did not finish; inspect it before retry', uncertain=True)

    def guest(self, action, **kwargs):
        result = json.loads(self.script(ROOT / 'scripts/vm-guest.ps1', {'Action': action, **kwargs},
                                       hold_pending=(action == 'install')).lstrip('\ufeff'))
        if action == 'install':
            self.finish_operation()
        return result

    def environment(self):
        env = dict(os.environ)
        # Prevent stale ambient destructive-test flags from leaking into an ordinary build.
        for key in list(env):
            if key.startswith(('VERIFY_', 'RESTORE_', 'DISCARD_SAVED_STATE_', 'STAGING_VM_')) or key == 'UNINSTALL_AFTER_INSTALL':
                del env[key]
        env.update(VM_NAME=self.p['vm'], SNAPSHOT_NAME=self.p.get('snapshot', ''),
                   SSH_USER=self.p['user'], SSH_PORT=str(self.p['port']), SSH_KEY=self.p['key'],
                   SSH_HOST=self.p['host'], VBOX_MANAGE=self.p['vbox'], STAGING_VM_NAME='',
                   VM_GUEST_CACHE_ROOT=self.p.get('guest_cache_root', ''),
                   RESTORE_BEFORE_BUILD='0', RESTORE_AFTER_BUILD='0', RESTORE_BEFORE_TEST='0',
                   RESTORE_AFTER_TEST='0', PRUNE_ORPHAN_MEDIA_AFTER_RESTORE='0', VM_LIFECYCLE_EXTERNAL='1')
        return env

    def stage(self, installer, leave_running, prepared=None):
        installer, expected, digest = prepared if prepared is not None else installer_artifact(installer)
        self.start()
        remote = self.remote + '\\azookey-install-' + uuid.uuid4().hex + '.exe'
        self.transfer(str(installer), remote)
        evidence = {'kind': 'installer log', 'remote': remote + '.log',
                    'path': str(self.directory / 'installer.log'), 'task_status': 'not_run'}
        self.evidence.append(evidence)
        def recover_log():
            try:
                self.transfer(evidence['remote'], evidence['path'], download=True, timeout=15)
                if not Path(evidence['path']).is_file():
                    raise VmError('Installer log was not recovered')
                evidence['task_status'] = 'succeeded'
            except (VmError, OSError) as error:
                evidence.update(task_status='failed', error=str(error))
                if isinstance(error, VmError) and error.uncertain:
                    raise
        try:
            result = self.guest('install', Installer=remote, Sha256=digest,
                                Expected=json.dumps({key: expected[key] for key in ('version', 'channel', 'revision', 'installerGeneration')}))
        except (VmError, ValueError) as error:
            if isinstance(error, VmError) and not error.uncertain:
                try:
                    recover_log()
                except VmError as collection:
                    raise VmError(f'{error}; installer log collection also failed: {collection}',
                                  error.status, uncertain=True) from error
            else:
                evidence['reason'] = 'Completion/transport unknown; reset promptly without further guest collection'
            raise
        evidence['installation'] = {key: result.get(key) for key in ('installed', 'installer_exit', 'reboot_required')}
        recover_log()
        result['install_log_recovery'] = evidence
        return result


@contextlib.contextmanager
def ownership(profile, lock_root, allow_pending=False):
    lock_root.mkdir(parents=True, exist_ok=True)
    # Lock the actual UUID/name across worktrees, not the profile alias.
    identity = hashlib.sha256(profile['vm'].encode()).hexdigest()
    with (lock_root / (identity + '.lock')).open('a+') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise VmError('Another operation owns this VM; wait for its result') from error
        reservation = lock_root / (identity + '.pending.json')
        if reservation.exists() and not allow_pending:
            raise VmError(f'Unresolved guest operation: {reservation}. Run operation-status, inspect its evidence, '
                          'then resolve-operation --reason after confirming work has stopped.')
        yield reservation


def parser():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--config', default=str(ROOT / '.local/vm.json'))
    p.add_argument('--profile', required=True)
    subs = p.add_subparsers(dest='command', required=True)
    for name in ('status', 'reboot', 'doctor', 'prepare', 'handoff', 'diagnostics', 'operation-status', 'wake'):
        subs.add_parser(name)
    reset = subs.add_parser('reset')
    reset.add_argument('--discard-session', action='store_true', help='Explicitly discard another workspace\'s idle session; active controller locks are never bypassed')
    stop = subs.add_parser('shutdown')
    stop.add_argument('--keep-current', action='store_true', help='Maintenance only: stop without restoring so a new baseline can be captured')
    resolve = subs.add_parser('resolve-operation', help='Explicitly release an inspected unresolved operation')
    resolve.add_argument('--reason', required=True)
    start = subs.add_parser('start')
    start.add_argument('--gui', action='store_true')
    start.add_argument('--keep-current', action='store_true', help='Explicit maintenance: cold boot current disk without resetting')
    recovery = subs.add_parser('recover-poweroff', help='Compatibility alias for immediate baseline reset')
    recovery.add_argument('--reason', required=True)
    media = subs.add_parser('detach-install-media', help='Detach an ISO/VISO from a powered-off VM')
    media.add_argument('--controller', required=True)
    media.add_argument('--port', type=int, required=True)
    media.add_argument('--device', type=int, default=0)
    for name in ('restore', 'snapshot'):
        sub = subs.add_parser(name)
        sub.add_argument('snapshot')
    shot = subs.add_parser('screenshot')
    shot.add_argument('path')
    for name in ('upload', 'download'):
        sub = subs.add_parser(name)
        sub.add_argument('source')
        sub.add_argument('destination')
    execute = subs.add_parser('exec')
    execute.add_argument('--script', required=True)
    execute.add_argument('--interactive', action='store_true')
    execute.add_argument('--timeout', type=int, default=120)
    build = subs.add_parser('build')
    build.add_argument('branch')
    test = subs.add_parser('test')
    test.add_argument('branch')
    test.add_argument('cargo_filter', nargs='?', default='composition')
    test.add_argument('swift_filter', nargs='?', default='skip')
    for name in ('stage', 'install'):
        stage = subs.add_parser(name)
        stage.add_argument('installer')
        stage.add_argument('--leave-running', action='store_true')
    return p


def execute(vm, args, result):
    profile, cmd = vm.p, args.command
    validate_command_profile(profile, cmd)
    if cmd == 'start':
        vm.start(args.gui, cold=args.keep_current)
    elif cmd == 'reboot':
        vm.shutdown_current()
        vm.begin_operation('reboot')
        vm.start()
    elif cmd in ('shutdown', 'reset', 'recover-poweroff'):
        if cmd == 'shutdown' and args.keep_current:
            vm.shutdown_current()
            vm.begin_operation('session', workspace=str(ROOT), mode='maintenance')
        else:
            vm.reset(getattr(args, 'reason', 'requested baseline reset'), graceful=(cmd == 'shutdown'))
    elif cmd == 'wake':
        if vm.state()['VMState'] != 'running':
            raise VmError('Wake requires a running VM; it never starts or resets a guest')
        vm.vbox('controlvm', profile['vm'], 'keyboardputscancode', '1d', '9d')
    elif cmd == 'diagnostics':
        vm.vbox('showvminfo', profile['vm'], '--log', '0')
    elif cmd in ('operation-status', 'resolve-operation'):
        result['operation_status'] = vm.operation_status()
        if cmd == 'resolve-operation':
            guest = result['operation_status'].get('guest', {})
            pending = result['operation_status']['pending'] or {}
            processes = guest.get('processes') if pending.get('operation') != 'interactive' else []
            if not args.reason.strip() or processes or guest.get('running_tasks'):
                raise VmError('Resolution requires a reason and no active assigned guest work; use reset for a stalled disposable guest')
            if pending.get('task') and vm.state()['VMState'] == 'running':
                vm.remove_inactive_task(pending['task'])
            result['resolution_reason'] = args.reason
            if pending.get('mode') == 'maintenance':
                vm.begin_operation('session', mode='maintenance', resolved_run_directory=pending.get('run_directory'))
            else:
                vm.finish_operation()
    elif cmd == 'detach-install-media':
        if vm.state()['VMState'] != 'poweroff':
            raise VmError('Media maintenance requires poweroff')
        raw = vm.vbox('showvminfo', profile['vm'], '--machinereadable')
        key = f'"{args.controller}-{args.port}-{args.device}"'
        devices = dict(re.findall(r'^([^=]+)="(.*)"$', raw, re.M))
        medium = devices.get(key, '')
        if not medium.lower().endswith(('.iso', '.viso')):
            raise VmError('Selected device is not an attached ISO/VISO; refusing disk detachment')
        result['detached_medium'] = medium
        vm.vbox('storageattach', profile['vm'], '--storagectl', args.controller,
                '--port', str(args.port), '--device', str(args.device), '--type', 'dvddrive', '--medium', 'none')
    elif cmd in ('doctor', 'prepare'):
        import tomllib
        pin = tomllib.loads((ROOT / 'rust-toolchain.toml').read_text())['toolchain']['channel']
        result['guest'] = vm.guest(cmd, RustVersion=pin, HostUtc=dt.datetime.now(dt.timezone.utc).isoformat())
    elif cmd == 'handoff':
        result['guest'] = vm.guest('identity')
        if vm.session_mode == 'maintenance':
            vm.shutdown_current()
    elif cmd in ('restore', 'snapshot'):
        if cmd == 'restore':
            vm.reset('requested snapshot restore', snapshot=args.snapshot)
        else:
            result['snapshot'] = vm.snapshot(args.snapshot)
    elif cmd == 'screenshot':
        target = Path(args.path).resolve()
        target.parent.mkdir(parents=True, exist_ok=True)
        target_arg = str(target)
        if profile['vbox'].endswith('.exe'):
            target_arg = vm.capture(['wslpath', '-w', str(target)]).stdout.strip()
        vm.vbox('controlvm', profile['vm'], 'screenshotpng', target_arg)
        result['screenshot'] = str(target)
    elif cmd in ('upload', 'download'):
        vm.start()
        vm.transfer(args.source, args.destination, cmd == 'download', prepared=getattr(args, 'transfer_artifact', None))
    elif cmd == 'exec':
        result['output'] = vm.script(Path(args.script).resolve(), interactive=args.interactive, timeout=args.timeout,
                                     prepared=getattr(args, 'script_artifact', None))
    elif cmd in ('build', 'test'):
        vm.start()
        command = ['bash', str(ROOT / 'scripts' / ('vm_build.sh' if cmd == 'build' else 'vm_test_client_composition.sh')), args.branch]
        if cmd == 'test':
            command += [args.cargo_filter, args.swift_filter]
        vm.run_operation(cmd, command, timeout=14400, env=vm.environment(), branch=args.branch)
        if cmd == 'build':
            log = vm.log.read_text(errors='replace')
            matches = re.findall(r'\[vm-build\] 完了: (.+\.exe) \((.+\.build-info\.json)\)', log)
            if not matches:
                raise VmError('Build exited zero without recovered artifact evidence', 'failed')
            installer, metadata = map(Path, matches[-1])
            if not installer.is_file() or not metadata.is_file():
                raise VmError('Recovered build files are missing', 'failed')
            with installer.open('rb') as stream:
                digest = hashlib.file_digest(stream, 'sha256').hexdigest()
            result['artifact'] = {'installer': str(installer), 'sha256': digest,
                                  'build_info': json.loads(metadata.read_text())}
    elif cmd in ('stage', 'install'):
        result['guest'] = vm.stage(args.installer, args.leave_running, prepared=getattr(args, 'installer_artifact', None))


def main():
    args = parser().parse_args()
    run_id = dt.datetime.now(dt.timezone.utc).strftime('%Y%m%dT%H%M%S') + '-' + uuid.uuid4().hex[:8]
    directory = ROOT / '.local/logs/vm' / run_id
    result = {'task_status': 'running', 'profile': args.profile, 'operation': args.command, 'run_id': run_id}
    vm = None
    try:
        # Validate local inputs before acquiring or resetting the guest. Reuse
        # the same identity/hash for transfer and installed-identity checks.
        if args.command in ('stage', 'install'):
            args.installer_artifact = installer_artifact(args.installer)
        if args.command == 'exec':
            args.script_artifact = script_artifact(args.script, directory)
        if args.command in ('upload', 'download'):
            args.transfer_artifact = transfer_artifact(args.source, args.destination, args.command == 'download')
        profile = load_profile(args.config, args.profile)
        validate_command_profile(profile, args.command)
        vm = VM(profile, directory)
        identity = {**profile, 'vm': vm.state()['UUID'] or profile['vm']}
        lock = contextlib.nullcontext() if args.command == 'status' else ownership(
            identity, Path.home() / '.cache/azookey-vm-locks',
            allow_pending=args.command != 'detach-install-media')
        with lock as reservation:
            vm.reservation = reservation
            result['before'] = vm.state()
            result_path = directory / 'result.json'
            result_path.write_text(json.dumps(result, indent=2))
            cmd = args.command
            managed = cmd in ('start', 'reboot', 'prepare', 'build', 'test', 'stage', 'install', 'exec', 'doctor', 'upload', 'download', 'handoff')
            pending = json.loads(reservation.read_text()) if reservation is not None and reservation.exists() else {}
            session = pending.get('operation') == 'session'
            protected_maintenance = pending.get('mode') == 'maintenance'
            if (session or protected_maintenance) and pending.get('workspace') != str(ROOT) and cmd not in ('status', 'operation-status', 'diagnostics', 'screenshot'):
                if not (cmd == 'reset' and args.discard_session):
                    raise VmError('Another workspace owns this session; its owner must finish it, or explicitly reset --discard-session')
            inspection_or_recovery = cmd in ('status', 'operation-status', 'diagnostics', 'screenshot', 'wake', 'resolve-operation', 'reset', 'restore', 'recover-poweroff')
            if pending and not session and pending.get('mode') == 'maintenance' and not inspection_or_recovery:
                raise VmError('Unresolved maintenance operation; inspect and resolve or explicitly reset before continuing')
            if cmd == 'reboot' and not session:
                raise VmError('Reboot requires an open verification or maintenance session')
            preserve = cmd in ('start', 'shutdown') and args.keep_current
            if (preserve or cmd == 'snapshot') and pending and not session:
                raise VmError('Unresolved guest operation; reset or confirm resolution before preserving current state')
            continuing = session and cmd in ('reboot', 'prepare', 'exec', 'doctor', 'upload', 'download', 'handoff')
            if cmd == 'prepare' and pending.get('mode') != 'maintenance':
                continuing = False
            keep = cmd in ('start', 'reboot', 'prepare', 'upload') or (cmd in ('stage', 'install') and args.leave_running) or (continuing and cmd != 'handoff')
            mode = 'maintenance' if cmd in ('prepare', 'snapshot') or preserve else (pending.get('mode', 'verification') if continuing else 'verification')
            vm.session_mode = mode
            lifecycle = vm.clean_run(keep=keep, continuing=continuing or preserve or cmd in ('handoff', 'download'), mode=mode) if managed else contextlib.nullcontext()
            with lifecycle:
                execute(vm, args, result)
            result['task_status'] = 'succeeded'
            result['after'] = vm.state()
    except (VmError, OSError, ValueError, KeyError, subprocess.TimeoutExpired, KeyboardInterrupt) as error:
        result['task_status'] = error.status if isinstance(error, VmError) else 'failed'
        result['error'] = str(error)
        if vm:
            try:
                result['after'] = vm.state()
            except Exception:
                result['after'] = {'VMState': 'unknown'}
    directory.mkdir(parents=True, exist_ok=True)
    result['log'] = str(directory / 'commands.log')
    result['commands'] = vm.commands if vm else []
    result['resets'] = vm.resets if vm else []
    result['evidence'] = vm.evidence if vm else []
    (directory / 'result.json').write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n')
    # Full command evidence stays in result.json; avoid dumping encoded PowerShell
    # and repeated polling commands into the caller's conversation.
    print(json.dumps({key: value for key, value in result.items() if key != 'commands'}, ensure_ascii=False, indent=2))
    return 0 if result['task_status'] == 'succeeded' else 1


if __name__ == '__main__':
    def interrupted(signum, frame):
        raise KeyboardInterrupt(f'Interrupted by signal {signum}')
    signal.signal(signal.SIGTERM, interrupted)
    raise SystemExit(main())
