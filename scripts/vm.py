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


class VM:
    def __init__(self, profile, directory):
        self.p = profile
        self.directory = Path(directory)
        self.directory.mkdir(parents=True, exist_ok=True)
        self.log = self.directory / 'commands.log'
        self.remote = rf'C:\Users\{profile["user"]}\AppData\Local\Temp'
        self.commands = []
        self.reservation = None

    def begin_operation(self, operation, **details):
        if self.reservation is not None:
            value = {'operation': operation, 'run_directory': str(self.directory), **details}
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
            value['guest'] = json.loads(self.ps(
                "$p=@(Get-Process -ErrorAction SilentlyContinue | Where-Object {"
                "$_.Id -ne $PID -and $_.ProcessName -match '^(cargo(-.*)?|rustc|rustup|swift.*|node|msbuild|cl|link|powershell|pwsh|azookey-install-.*|setup.*)$'"
                "} | Select-Object ProcessName,Id,Path,StartTime); "
                "$t=@(Get-ScheduledTask | Where-Object {$_.TaskName -like 'azookey-vm-*' -and $_.State -eq 'Running'} | Select-Object TaskName,State); "
                "@{processes=$p;running_tasks=$t} | ConvertTo-Json -Depth 4 -Compress").lstrip('\ufeff'))
        elif state != 'poweroff':
            raise VmError(f'Cannot inspect unresolved guest work during VM state {state}')
        return value

    def run(self, argv, timeout=60, check=True, env=None):
        # Secrets belong in guest credential storage, not command arguments.
        with self.log.open('ab') as log:
            log.write((json.dumps({'command': argv}, ensure_ascii=False) + '\n').encode())
            log.flush()
            try:
                proc = subprocess.run(argv, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT,
                                      env=env, timeout=timeout, stdin=subprocess.DEVNULL)
            except subprocess.TimeoutExpired as error:
                self.commands.append({'command': argv, 'exit_code': None, 'status': 'timeout'})
                raise VmError('Command timed out; remote work may still be active. Inspect before retry.', uncertain=True) from error
        self.commands.append({'command': argv, 'exit_code': proc.returncode})
        if check and proc.returncode:
            raise VmError(f'Command exited {proc.returncode}; see {self.log}', 'failed')
        return proc.returncode

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
            raise VmError(f'Command exited {proc.returncode}: {proc.stderr.strip()[:250]}; see {self.log}',
                          'failed', uncertain=(argv[0] == 'ssh' and proc.returncode == 255))
        return proc

    def vbox(self, *args):
        return self.capture([self.p['vbox'], *args]).stdout

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
                '-o', 'StrictHostKeyChecking=accept-new', '-o', 'ConnectTimeout=8',
                '-o', 'ServerAliveInterval=15', '-o', 'ServerAliveCountMax=3',
                f'{self.p["user"]}@{self.p["host"]}']

    def ps(self, code, timeout=60):
        code = "$ErrorActionPreference='Stop'; [Console]::OutputEncoding=New-Object Text.UTF8Encoding($false); " + code
        encoded = base64.b64encode(code.encode('utf-16le')).decode()
        return self.capture(self.ssh_args() + ['powershell -NoProfile -ExecutionPolicy Bypass -EncodedCommand ' + encoded], timeout).stdout

    def ready(self, seconds=180):
        end = time.monotonic() + seconds
        woke = False
        while time.monotonic() < end:
            try:
                if self.capture(self.ssh_args() + ['echo VM_READY'], timeout=12, check=False).returncode == 0:
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
        raise VmError('SSH readiness deadline exceeded; inspect console, boot media and sleep before retry')

    def start(self, gui=False):
        state = self.state()['VMState']
        if state in ('poweroff', 'saved', 'aborted'):
            self.vbox('startvm', self.p['vm'], '--type', 'gui' if gui else 'headless')
        elif state != 'running':
            raise VmError(f'Refuse start during VM state {state}')
        self.ready(int(self.p.get('ready_timeout', 180)))
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
        # No /f: never silently discard application drafts or force-poweroff on timeout.
        try:
            self.ps('shutdown.exe /s /t 0; if($LASTEXITCODE -notin @(0,1115)){exit $LASTEXITCODE}')
        except (VmError, subprocess.TimeoutExpired):
            # An idle guest may not serve SSH. ACPI requests normal OS shutdown,
            # unlike poweroff, and remains useful without the guest network.
            self.vbox('controlvm', self.p['vm'], 'acpipowerbutton')
        end = time.monotonic() + int(self.p.get('shutdown_timeout', 180))
        while time.monotonic() < end:
            if self.state()['VMState'] == 'poweroff':
                return
            time.sleep(2)
        raise VmError('Normal shutdown did not complete; VM preserved. Inspect blockers instead of forcing poweroff.')

    def recover_poweroff(self, reason):
        if self.p.get('role') not in ('build', 'disposable') or not reason.strip():
            raise VmError('Explicit poweroff recovery is limited to an owned build/disposable profile with a recorded reason')
        result = {'recovery_reason': reason}
        if self.reservation is not None and self.reservation.exists():
            result['interrupted_operation'] = json.loads(self.reservation.read_text())
        try:
            self.shutdown()
            result['recovery_method'] = 'normal_shutdown'
        except VmError as error:
            result['normal_shutdown_error'] = str(error)
            self.vbox('controlvm', self.p['vm'], 'poweroff')
            result['recovery_method'] = 'poweroff'
        if self.state()['VMState'] != 'poweroff':
            raise VmError('Recovery poweroff did not complete', 'failed')
        # A confirmed powered-off guest cannot leave remote work running.
        self.finish_operation()
        return result

    def transfer(self, source, destination, download=False):
        # OpenSSH Windows accepts /C:/... paths; no remote shell quoting or relative ambiguity.
        remote = source if download else destination
        if not re.match(r'^[A-Za-z]:[\\/]', remote):
            raise VmError('Remote transfer path must be an absolute Windows path')
        remote = '/' + remote.replace('\\', '/')
        endpoint = f'{self.p["user"]}@{self.p["host"]}:{remote}'
        local = str(Path(destination if download else source).resolve())
        if download:
            Path(local).parent.mkdir(parents=True, exist_ok=True)
        elif not Path(local).is_file():
            raise VmError(f'Upload source does not exist: {local}')
        argv = ['scp', '-i', self.p['key'], '-P', str(self.p['port']), '-o', 'BatchMode=yes',
                '-o', 'StrictHostKeyChecking=accept-new', '-o', 'ConnectTimeout=8']
        self.run(argv + ([endpoint, local] if download else [local, endpoint]), timeout=600)

    def script(self, source, arguments=None, interactive=False, timeout=1800, hold_pending=False):
        token = 'azookey-vm-' + uuid.uuid4().hex
        remote = self.remote + '\\' + token + '.ps1'
        # Windows PowerShell 5.1 reads BOM-less scripts using the ANSI code page.
        # Validate UTF-8 before touching the guest and upload an explicit UTF-8 BOM.
        normalized = self.directory / (token + '.ps1')
        normalized.write_text(Path(source).read_text(encoding='utf-8-sig'), encoding='utf-8-sig')
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
            if not error.uncertain and not hold_pending:
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
        self.ps(f"[IO.File]::WriteAllBytes({ps_quote(wrapper)},[Convert]::FromBase64String('{encoded}')); "
                f"$a=New-ScheduledTaskAction -Execute 'powershell.exe' -Argument ('-NoProfile -ExecutionPolicy Bypass -File \"'+{ps_quote(wrapper)}+'\"'); "
                f"$p=New-ScheduledTaskPrincipal -UserId ($env:COMPUTERNAME+'\\'+{ps_quote(self.p['user'])}) -LogonType Interactive -RunLevel Limited; "
                f"$s=New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Seconds {timeout}); "
                f"Register-ScheduledTask -TaskName '{token}' -Action $a -Principal $p -Settings $s | Out-Null; Start-ScheduledTask -TaskName '{token}'")
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            response = self.ps(f"if(Test-Path {ps_quote(done)}){{ @{{code=[IO.File]::ReadAllText({ps_quote(done)});output=[IO.File]::ReadAllText({ps_quote(output)})}} | ConvertTo-Json -Compress }}")
            if response.strip():
                value = json.loads(response.lstrip('\ufeff'))
                self.ps(f"Unregister-ScheduledTask -TaskName '{token}' -Confirm:$false")
                self.finish_operation()
                if value['code'] != '0':
                    raise VmError(f'Interactive script failed; see {output} and {self.log}', 'failed')
                return value['output']
            time.sleep(2)
        raise VmError(f'Interactive task {token} did not finish; inspect it before retry')

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

    def stage(self, installer, leave_running):
        installer = Path(installer).resolve()
        if not installer.is_file():
            raise VmError('Installer file not found; select an exact artifact')
        metadata = installer.with_suffix('.build-info.json')
        if not metadata.exists():
            metadata = installer.parent / 'build-info.json'
        expected = json.loads(metadata.read_text())
        for key in ('version', 'channel', 'revision', 'installerGeneration'):
            if key not in expected:
                raise VmError(f'Missing build identity: {key}')
        with installer.open('rb') as stream:
            digest = hashlib.file_digest(stream, 'sha256').hexdigest()
        self.start()
        remote = self.remote + '\\azookey-install-' + uuid.uuid4().hex + '.exe'
        self.transfer(str(installer), remote)
        result = self.guest('install', Installer=remote, Sha256=digest,
                            Expected=json.dumps({key: expected[key] for key in ('version', 'channel', 'revision', 'installerGeneration')}))
        if not leave_running:
            self.shutdown()
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
    for name in ('status', 'shutdown', 'doctor', 'prepare', 'handoff', 'diagnostics', 'operation-status', 'wake'):
        subs.add_parser(name)
    resolve = subs.add_parser('resolve-operation', help='Explicitly release an inspected unresolved operation')
    resolve.add_argument('--reason', required=True)
    start = subs.add_parser('start')
    start.add_argument('--gui', action='store_true')
    recovery = subs.add_parser('recover-poweroff', help='Explicit recovery of an owned build/disposable guest, never an automatic fallback')
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
    execute.add_argument('--timeout', type=int, default=1800)
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


def main():
    args = parser().parse_args()
    run_id = dt.datetime.now(dt.timezone.utc).strftime('%Y%m%dT%H%M%S') + '-' + uuid.uuid4().hex[:8]
    directory = ROOT / '.local/logs/vm' / run_id
    result = {'task_status': 'running', 'profile': args.profile, 'operation': args.command, 'run_id': run_id}
    vm = None
    try:
        profile = load_profile(args.config, args.profile)
        vm = VM(profile, directory)
        identity = {**profile, 'vm': vm.state()['UUID'] or profile['vm']}
        lock = contextlib.nullcontext() if args.command == 'status' else ownership(
            identity, Path.home() / '.cache/azookey-vm-locks',
            allow_pending=args.command in ('operation-status', 'resolve-operation', 'diagnostics', 'screenshot', 'wake', 'recover-poweroff'))
        with lock as reservation:
            vm.reservation = reservation
            result['before'] = vm.state()
            result_path = directory / 'result.json'
            result_path.write_text(json.dumps(result, indent=2))
            cmd = args.command
            if cmd == 'start':
                vm.start(args.gui)
            elif cmd == 'shutdown':
                vm.shutdown()
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
                        raise VmError('Resolution requires a reason and no active assigned guest work; inspect before retry')
                    if pending.get('task') and vm.state()['VMState'] == 'running':
                        vm.ps(f"Unregister-ScheduledTask -TaskName {ps_quote(pending['task'])} -Confirm:$false -ErrorAction SilentlyContinue")
                    result['resolution_reason'] = args.reason
                    vm.finish_operation()
            elif cmd == 'recover-poweroff':
                result.update(vm.recover_poweroff(args.reason))
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
                if cmd == 'prepare' and profile.get('role') != 'build':
                    raise VmError('Toolchain preparation requires a build profile')
                import tomllib
                pin = tomllib.loads((ROOT / 'rust-toolchain.toml').read_text())['toolchain']['channel']
                result['guest'] = vm.guest(cmd, RustVersion=pin, HostUtc=dt.datetime.now(dt.timezone.utc).isoformat())
            elif cmd == 'handoff':
                result['guest'] = vm.guest('identity')
                vm.shutdown()
            elif cmd in ('restore', 'snapshot'):
                if vm.state()['VMState'] != 'poweroff':
                    raise VmError('Snapshot operations require normal shutdown first; no live/saved-state snapshots')
                if cmd == 'snapshot':
                    # Existing snapshots are retained. Promotion to the profile is a separate explicit edit.
                    vm.vbox('snapshot', profile['vm'], 'take', args.snapshot, '--description', 'Verified powered-off maintenance baseline')
                else:
                    vm.vbox('snapshot', profile['vm'], 'restore', args.snapshot)
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
                vm.transfer(args.source, args.destination, cmd == 'download')
            elif cmd == 'exec':
                result['output'] = vm.script(Path(args.script).resolve(), interactive=args.interactive, timeout=args.timeout)
            elif cmd in ('build', 'test'):
                if profile.get('role') != 'build':
                    raise VmError('Build/test commands require a build profile')
                vm.start()
                command = ['bash', str(ROOT / 'scripts' / ('vm_build.sh' if cmd == 'build' else 'vm_test_client_composition.sh')), args.branch]
                if cmd == 'test':
                    command += [args.cargo_filter, args.swift_filter]
                vm.begin_operation(cmd, branch=args.branch)
                vm.run(command, timeout=14400, env=vm.environment())
                vm.finish_operation()
                if cmd == 'build':
                    # The helper names the recovered pair in its terminal completion line.
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
                vm.shutdown()
            elif cmd in ('stage', 'install'):
                if profile.get('role') not in ('manual', 'disposable'):
                    raise VmError('Installation requires a manual or disposable profile')
                result['guest'] = vm.stage(args.installer, args.leave_running)
            result['task_status'] = 'succeeded'
            result['after'] = vm.state()
    except (VmError, OSError, ValueError, KeyError, subprocess.TimeoutExpired) as error:
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
    (directory / 'result.json').write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n')
    # Full command evidence stays in result.json; avoid dumping encoded PowerShell
    # and repeated polling commands into the caller's conversation.
    print(json.dumps({key: value for key, value in result.items() if key != 'commands'}, ensure_ascii=False, indent=2))
    return 0 if result['task_status'] == 'succeeded' else 1


if __name__ == '__main__':
    raise SystemExit(main())
