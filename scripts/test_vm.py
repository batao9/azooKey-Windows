"""Behavioral tests of VM boundaries; never contact a VM or SSH endpoint."""
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch, Mock

spec = importlib.util.spec_from_file_location('vm', Path(__file__).with_name('vm.py'))
vm = importlib.util.module_from_spec(spec)
spec.loader.exec_module(vm)


class VmTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.key = self.root / 'key'
        self.key.write_text('test fixture, not a real key')
        self.profile = dict(vm='vm-uuid', host='127.0.0.1', port=12345, user='guest',
                            key=str(self.key), vbox='not-a-real-vbox', role='build', snapshot='baseline-id')
        self.machine = vm.VM(self.profile, self.root / 'log')

    def test_explicit_connection_values_required(self):
        config = self.root / 'vm.json'
        config.write_text(json.dumps({'profiles': {'build': {**self.profile, 'user': ''}}}))
        with self.assertRaisesRegex(vm.VmError, 'user'):
            vm.load_profile(config, 'build')

    def test_ssh_never_falls_back_to_host_username(self):
        argv = self.machine.ssh_args()
        self.assertEqual(argv[-1], 'guest@127.0.0.1')
        self.assertEqual(argv[argv.index('-i') + 1], str(self.key))
        self.assertEqual(argv[argv.index('-p') + 1], '12345')
        self.assertIn('BatchMode=yes', argv)

    def test_state_recovers_only_transient_read_errors(self):
        for message in ('The object is not ready: E_ACCESSDENIED (0x80070005)',
                        'Machine already has a lock request pending: VBOX_E_INVALID_OBJECT_STATE (0x80bb0007)'):
            with self.subTest(message=message):
                self.machine.vbox = Mock(side_effect=[vm.VmError(message, 'failed'),
                                                     'VMState="poweroff"\nUUID="vm-uuid"\n'])
                with patch.object(vm.time, 'sleep') as sleep:
                    self.assertEqual(self.machine.state()['VMState'], 'poweroff')
                self.assertEqual(self.machine.vbox.call_count, 2)
                self.assertTrue(all(call.args[0] == 'showvminfo' for call in self.machine.vbox.call_args_list))
                sleep.assert_called_once_with(1)

    def test_state_transient_retry_is_bounded(self):
        self.machine.vbox = Mock(side_effect=vm.VmError('The object is not ready (0x80070005)', 'failed'))
        with patch.object(vm.time, 'sleep') as sleep:
            with self.assertRaises(vm.VmError):
                self.machine.state()
        self.assertEqual(self.machine.vbox.call_count, 3)
        self.assertEqual(sleep.call_count, 2)

    def test_state_does_not_retry_unrelated_errors(self):
        self.machine.vbox = Mock(side_effect=vm.VmError('Machine not found', 'failed'))
        with patch.object(vm.time, 'sleep') as sleep:
            with self.assertRaises(vm.VmError):
                self.machine.state()
        self.machine.vbox.assert_called_once()
        sleep.assert_not_called()

    def test_start_saved_resumes_and_syncs_real_clock_without_restoring(self):
        self.machine.state = Mock(return_value={'VMState': 'saved'})
        self.machine.vbox = Mock()
        self.machine.ready = Mock()
        self.machine.ps = Mock()
        self.machine.start()
        calls = [call.args for call in self.machine.vbox.call_args_list]
        self.assertEqual(calls[0], ('startvm', 'vm-uuid', '--type', 'headless'))
        self.assertNotIn('discardstate', str(calls))
        self.assertNotIn('restore', str(calls))
        self.assertIn('Set-Date', self.machine.ps.call_args.args[0])
        self.assertNotIn('AddHours', self.machine.ps.call_args.args[0])

    def test_start_refuses_live_snapshot(self):
        self.machine.state = Mock(return_value={'VMState': 'livesnapshotting'})
        self.machine.vbox = Mock()
        with self.assertRaises(vm.VmError):
            self.machine.start()
        self.machine.vbox.assert_not_called()

    def test_ready_does_not_send_keys_to_responsive_guest(self):
        self.machine.capture = Mock(return_value=subprocess.CompletedProcess(['ssh'], 0))
        self.machine.vbox = Mock()
        self.machine.ready()
        self.machine.vbox.assert_not_called()

    def test_ready_wakes_once_without_shift_after_failed_probe(self):
        self.machine.capture = Mock(side_effect=[subprocess.CompletedProcess(['ssh'], 255),
                                                subprocess.CompletedProcess(['ssh'], 0)])
        self.machine.vbox = Mock()
        with patch.object(vm.time, 'sleep'):
            self.machine.ready()
        self.machine.vbox.assert_called_once_with('controlvm', 'vm-uuid', 'keyboardputscancode', '1d', '9d')

    def test_shutdown_has_short_deadline_and_never_waits_for_ssh(self):
        self.machine.state = Mock(return_value={'VMState': 'running'})
        self.machine.ps = Mock()
        self.machine.vbox = Mock()
        with patch.object(vm.time, 'monotonic', side_effect=[0, 11]):
            with self.assertRaisesRegex(vm.VmError, 'Normal shutdown'):
                self.machine.shutdown()
        self.machine.vbox.assert_called_once_with('controlvm', 'vm-uuid', 'acpipowerbutton')
        self.machine.ps.assert_not_called()

    def test_powered_off_shutdown_is_idempotent(self):
        self.machine.state = Mock(return_value={'VMState': 'poweroff'})
        self.machine.ps = Mock()
        self.machine.shutdown()
        self.machine.ps.assert_not_called()

    def restored_state(self):
        return {'VMState': 'poweroff', 'CurrentSnapshotUUID': 'baseline-id', 'CurrentSnapshotName': 'baseline'}

    def test_abnormal_reset_powers_off_and_restores_without_ssh(self):
        self.machine.vbox = Mock()
        self.machine.ps = Mock()
        self.machine.state = Mock(side_effect=[{'VMState': 'running'}, self.restored_state()])
        with vm.ownership(self.profile, self.root / 'locks') as reservation:
            self.machine.reservation = reservation
            self.machine.begin_operation('build')
            result = self.machine.reset('owned stalled build')
            self.assertFalse(reservation.exists())
        self.machine.ps.assert_not_called()
        self.assertEqual([call.args for call in self.machine.vbox.call_args_list], [
            ('snapshot', 'vm-uuid', 'showvminfo', 'baseline-id'),
            ('controlvm', 'vm-uuid', 'poweroff'),
            ('snapshot', 'vm-uuid', 'restore', 'baseline-id')])
        self.assertEqual(result['task_status'], 'succeeded')
        self.assertEqual(result['interrupted_operation']['operation'], 'build')

    def test_normal_cleanup_falls_back_to_poweroff_and_restore(self):
        self.machine.shutdown = Mock(side_effect=vm.VmError('shutdown deadline'))
        self.machine.vbox = Mock()
        self.machine.state = Mock(side_effect=[{'VMState': 'running'}, {'VMState': 'running'}, self.restored_state()])
        result = self.machine.reset('finished run', graceful=True)
        self.machine.shutdown.assert_called_once()
        self.assertIn('normal_shutdown_error', result)
        self.assertIn(('controlvm', 'vm-uuid', 'poweroff'), [call.args for call in self.machine.vbox.call_args_list])

    def test_unconfirmed_restore_preserves_pending_reservation(self):
        self.machine.vbox = Mock()
        self.machine.state = Mock(return_value={'VMState': 'poweroff', 'CurrentSnapshotUUID': 'wrong-baseline'})
        with vm.ownership(self.profile, self.root / 'locks') as reservation:
            self.machine.reservation = reservation
            self.machine.begin_operation('build')
            with self.assertRaisesRegex(vm.VmError, 'not confirmed'):
                self.machine.reset('owned stalled build')
            self.assertTrue(reservation.exists())

    def test_script_completion_inspection_does_not_depend_on_task_scheduler(self):
        self.machine.state = Mock(return_value={'VMState': 'running'})
        def inspect(code, **kwargs):
            if 'Get-ScheduledTask' in code:
                raise vm.VmError('Task Scheduler inspection unavailable', uncertain=True)
            return json.dumps({'processes': [], 'running_tasks': []})
        self.machine.ps = Mock(side_effect=inspect)
        with vm.ownership(self.profile, self.root / 'locks') as reservation:
            self.machine.reservation = reservation
            self.machine.session_mode = 'maintenance'
            self.machine.begin_operation('script', remote_script='fixture.ps1')
            result = self.machine.operation_status()
        self.assertEqual(result['guest'], {'processes': [], 'running_tasks': []})
        self.assertEqual(result['pending']['mode'], 'maintenance')

    def test_manual_vm_is_also_disposable_and_saved_memory_is_discarded(self):
        self.machine.p['role'] = 'manual'
        self.machine.vbox = Mock()
        self.machine.state = Mock(side_effect=[{'VMState': 'saved'}, self.restored_state()])
        self.machine.reset('new installation')
        self.assertIn(('discardstate', 'vm-uuid'), [call.args for call in self.machine.vbox.call_args_list])

    def test_missing_snapshot_does_not_discard_current_state(self):
        self.machine.vbox = Mock(side_effect=vm.VmError('snapshot missing'))
        self.machine.state = Mock()
        with vm.ownership(self.profile, self.root / 'locks') as reservation:
            self.machine.reservation = reservation
            with self.assertRaises(vm.VmError):
                self.machine.reset('new run')
            self.assertTrue(reservation.exists())
            self.assertEqual(self.machine.resets[-1]['task_status'], 'failed')
        self.machine.vbox.assert_called_once_with('snapshot', 'vm-uuid', 'showvminfo', 'baseline-id')
        self.machine.state.assert_not_called()

    def test_clean_run_restores_before_and_after_work(self):
        events = []
        self.machine.reset = Mock(side_effect=lambda reason, **kw: events.append(reason))
        with self.machine.clean_run():
            events.append('work')
        self.assertEqual(events, ['clean start', 'work', 'finished run'])

    def test_failed_run_resets_immediately_without_retrying_work(self):
        self.machine.reset = Mock()
        with self.assertRaisesRegex(vm.VmError, 'SSH hung'):
            with self.machine.clean_run():
                raise vm.VmError('SSH hung', uncertain=True)
        self.assertEqual(self.machine.reset.call_args_list[-1].args, ('failed run; discard guest immediately',))
        self.assertNotIn('graceful', self.machine.reset.call_args.kwargs)

    def test_failed_reset_does_not_hide_execution_failure(self):
        self.machine.reset = Mock(side_effect=[None, vm.VmError('restore failed')])
        with self.assertRaisesRegex(vm.VmError, 'SSH hung; baseline recovery also failed: restore failed'):
            with self.machine.clean_run():
                raise vm.VmError('SSH hung', uncertain=True)

    def test_ui_session_continues_without_erasing_installed_state(self):
        self.machine.reset = Mock()
        with vm.ownership(self.profile, self.root / 'locks') as reservation:
            self.machine.reservation = reservation
            with self.machine.clean_run(keep=True):
                pass
            self.assertEqual(json.loads(reservation.read_text())['operation'], 'session')
            with self.machine.clean_run(keep=True, continuing=True):
                pass
            self.assertEqual(self.machine.reset.call_count, 1)
            with self.machine.clean_run(continuing=True):
                pass
            self.assertEqual(self.machine.reset.call_args.args, ('finished run',))

    def test_helper_output_stall_is_bounded_and_kills_its_process_group(self):
        child = ('import subprocess,sys,time; '
                 'p=subprocess.Popen([sys.executable,"-c","import time; time.sleep(60)"]); '
                 'print(p.pid,flush=True); time.sleep(60)')
        with self.assertRaises(vm.VmError) as caught:
            self.machine.run(['bash', '-c', 'timeout --foreground 60s "$@"; wait', 'nested-timeout',
                              sys.executable, '-c', child], timeout=10, idle_timeout=0.5)
        self.assertTrue(caught.exception.uncertain)
        self.assertEqual(self.machine.commands[-1]['status'], 'timeout')
        child_pid = next(line for line in self.machine.log.read_text().splitlines() if line.isdecimal())
        proc_stat = Path('/proc') / child_pid / 'stat'
        for _ in range(100):
            try:
                dead = proc_stat.read_text().split()[2] == 'Z'
            except FileNotFoundError:
                dead = True
            if dead:
                break
            vm.time.sleep(0.01)
        else:
            self.fail('helper child survived the deadline')

    def test_helper_that_continues_to_report_progress_can_finish(self):
        self.assertEqual(self.machine.run([sys.executable, '-c', 'print("finished", flush=True)'],
                                         timeout=10, idle_timeout=5), 0)

    def lifecycle_command(self, arguments, pending=None, failure=None, real_dispatch=False,
                          failure_pending=None, clear_before_failure=False):
        locks = self.root / '.cache/azookey-vm-locks'
        with vm.ownership(self.profile, locks, allow_pending=True) as reservation:
            if pending:
                reservation.write_text(json.dumps(pending))
        events, output = [], io.StringIO()
        implementation = vm.execute
        def reset(machine, reason, **kwargs):
            events.append(reason)
            machine.finish_operation()
        def work(machine, args, result):
            events.append(args.command)
            if failure_pending:
                machine.begin_operation(**failure_pending)
            if failure:
                if clear_before_failure:
                    machine.finish_operation()
                raise failure
            if real_dispatch:
                implementation(machine, args, result)
        with patch.object(vm, 'ROOT', self.root), \
                patch.object(vm.Path, 'home', return_value=self.root), \
                patch.object(vm, 'load_profile', return_value=self.profile), \
                patch.object(vm.VM, 'state', return_value={'VMState': 'running', 'UUID': self.profile['vm']}), \
                patch.object(vm.VM, 'reset', autospec=True, side_effect=reset), \
                patch.object(vm.VM, 'shutdown'), patch.object(vm.VM, 'shutdown_current'), patch.object(vm.VM, 'start'), \
                patch.object(vm.VM, 'guest', return_value={'installed': None}), \
                patch.object(vm, 'execute', side_effect=work), \
                patch.object(vm.sys, 'argv', ['vm.py', '--profile', 'build', *arguments]), \
                contextlib.redirect_stdout(output):
            code = vm.main()
        return code, json.loads(output.getvalue()), reservation, events

    def test_cli_build_test_install_and_standalone_probe_start_and_finish_clean(self):
        for args in (['build', 'feature/example'], ['test', 'feature/example'],
                     ['stage', 'fixture.exe'], ['install', 'fixture.exe'], ['exec', '--script', 'probe.ps1'], ['doctor']):
            with self.subTest(args=args):
                code, result, reservation, events = self.lifecycle_command(args)
                self.assertEqual(code, 0)
                self.assertEqual(events, ['clean start', args[0], 'finished run'])
                self.assertFalse(reservation.exists())

    def test_cli_stale_timeout_reservation_is_reset_for_next_clean_run(self):
        code, _, reservation, events = self.lifecycle_command(['test', 'feature/example'], {'operation': 'build'})
        self.assertEqual(code, 0)
        self.assertEqual(events[0], 'clean start')
        self.assertFalse(reservation.exists())

    def test_cli_failure_is_not_success_after_reset(self):
        code, result, reservation, events = self.lifecycle_command(
            ['test', 'feature/example'], failure=vm.VmError('SSH failed', 'failed', uncertain=True))
        self.assertEqual(code, 1)
        self.assertEqual(result['task_status'], 'failed')
        self.assertEqual(events, ['clean start', 'test', 'failed run; discard guest immediately'])
        self.assertFalse(reservation.exists())

    def test_cli_stage_session_keeps_state_until_handoff(self):
        code, _, reservation, events = self.lifecycle_command(['stage', 'fixture.exe', '--leave-running'])
        self.assertEqual(code, 0)
        self.assertEqual(events, ['clean start', 'stage'])
        session = json.loads(reservation.read_text())
        code, _, reservation, events = self.lifecycle_command(['exec', '--script', 'probe.ps1'], session)
        self.assertEqual(code, 0)
        self.assertEqual(events, ['exec'])
        code, _, reservation, events = self.lifecycle_command(['handoff'], session)
        self.assertEqual(code, 0)
        self.assertEqual(events, ['handoff', 'finished run'])
        self.assertFalse(reservation.exists())

    def test_cli_does_not_reset_another_workspaces_ui_session(self):
        session = {'operation': 'session', 'workspace': 'other-workspace'}
        for args in (['test', 'feature/example'], ['shutdown'], ['shutdown', '--keep-current']):
            with self.subTest(args=args):
                code, _, reservation, events = self.lifecycle_command(args, session, real_dispatch=True)
                self.assertEqual(code, 1)
                self.assertEqual(events, [])
                self.assertEqual(json.loads(reservation.read_text()), session)

    def test_cli_maintenance_prepare_validate_stop_and_cold_boot_do_not_reset(self):
        code, _, reservation, events = self.lifecycle_command(['prepare'])
        self.assertEqual(code, 0)
        self.assertEqual(events, ['clean start', 'prepare'])
        session = json.loads(reservation.read_text())
        self.assertEqual(session['mode'], 'maintenance')
        for args in (['doctor'], ['exec', '--script', 'probe.ps1'], ['shutdown', '--keep-current'], ['start', '--keep-current']):
            code, _, reservation, events = self.lifecycle_command(args, session,
                                                                 real_dispatch=args[0] in ('start', 'shutdown'))
            self.assertEqual(code, 0)
            self.assertEqual(events, [args[0]])
            session = json.loads(reservation.read_text())
            self.assertEqual(session['mode'], 'maintenance')

    def test_cli_reboot_keeps_installed_verification_session(self):
        session = {'operation': 'session', 'workspace': str(self.root), 'mode': 'verification'}
        code, _, reservation, events = self.lifecycle_command(['reboot'], session, real_dispatch=True)
        self.assertEqual(code, 0)
        self.assertEqual(events, ['reboot'])
        self.assertEqual(json.loads(reservation.read_text())['mode'], 'verification')

    def test_preserve_current_cannot_replace_an_unresolved_operation_with_a_session(self):
        pending = {'operation': 'interactive', 'task': 'azookey-vm-test', 'remote_script': 'fixture.ps1'}
        for args in (['start', '--keep-current'], ['shutdown', '--keep-current'], ['snapshot', 'new-baseline']):
            with self.subTest(args=args):
                code, _, reservation, events = self.lifecycle_command(args, pending, real_dispatch=True)
                self.assertEqual(code, 1)
                self.assertEqual(events, [])
                self.assertEqual(json.loads(reservation.read_text()), pending)

    def maintenance_session(self):
        return {'operation': 'session', 'workspace': str(self.root), 'mode': 'maintenance'}

    def test_maintenance_known_completed_failure_preserves_session_and_next_probe(self):
        code, result, reservation, events = self.lifecycle_command(
            ['exec', '--script', 'probe.ps1'], self.maintenance_session(),
            failure=vm.VmError('known script failure', 'failed'), clear_before_failure=True)
        self.assertEqual(code, 1)
        self.assertEqual(result['task_status'], 'failed')
        self.assertEqual(events, ['exec'])
        pending = json.loads(reservation.read_text())
        self.assertEqual(pending['mode'], 'maintenance')
        self.assertEqual(pending['operation'], 'session')
        code, _, _, events = self.lifecycle_command(['doctor'], pending)
        self.assertEqual(code, 0)
        self.assertEqual(events, ['doctor'])

    def test_uncertain_maintenance_work_is_preserved_and_next_invocation_cannot_reset_it(self):
        for failure in (vm.VmError('timeout', uncertain=True), KeyboardInterrupt('interrupted')):
            with self.subTest(failure=failure):
                code, _, reservation, events = self.lifecycle_command(
                    ['exec', '--script', 'probe.ps1'], self.maintenance_session(), failure=failure,
                    failure_pending={'operation': 'interactive', 'task': 'azookey-vm-test'})
                self.assertEqual(code, 1)
                self.assertEqual(events, ['exec'])
                pending = json.loads(reservation.read_text())
                self.assertEqual(pending['operation'], 'interactive')
                self.assertEqual(pending['task'], 'azookey-vm-test')
                self.assertEqual(pending['mode'], 'maintenance')
                for args in (['doctor'], ['exec', '--script', 'next.ps1'], ['test', 'feature/example']):
                    code, _, reservation, events = self.lifecycle_command(args, pending)
                    self.assertEqual(code, 1)
                    self.assertEqual(events, [])
                    self.assertEqual(json.loads(reservation.read_text()), pending)

    def test_fresh_verification_does_not_inherit_previous_maintenance_failure_policy(self):
        code, _, reservation, events = self.lifecycle_command(
            ['test', 'feature/example'], self.maintenance_session(), failure=vm.VmError('test failed', 'failed'))
        self.assertEqual(code, 1)
        self.assertEqual(events, ['clean start', 'test', 'failed run; discard guest immediately'])
        self.assertFalse(reservation.exists())

    def test_maintenance_handoff_preserves_current_disk_for_capture(self):
        code, _, reservation, events = self.lifecycle_command(['handoff'], self.maintenance_session(), real_dispatch=True)
        self.assertEqual(code, 0)
        self.assertEqual(events, ['handoff'])
        self.assertEqual(json.loads(reservation.read_text())['mode'], 'maintenance')

    def test_foreign_uncertain_maintenance_requires_explicit_discard(self):
        pending = {'operation': 'script', 'workspace': 'other-workspace', 'mode': 'maintenance', 'remote_script': 'fixture.ps1'}
        code, _, reservation, events = self.lifecycle_command(['reset'], pending, real_dispatch=True)
        self.assertEqual(code, 1)
        self.assertEqual(events, [])
        self.assertEqual(json.loads(reservation.read_text()), pending)
        code, _, reservation, events = self.lifecycle_command(['reset', '--discard-session'], pending, real_dispatch=True)
        self.assertEqual(code, 0)
        self.assertFalse(reservation.exists())

    def test_preserving_shutdown_observes_poweroff_after_lost_ack_without_replaying(self):
        self.machine.session_mode = 'maintenance'
        self.machine.state = Mock(side_effect=[{'VMState': 'running'}, {'VMState': 'poweroff'}])
        self.machine.capture = Mock(side_effect=vm.VmError('SSH disconnected', uncertain=True))
        self.machine.vbox = Mock()
        with vm.ownership(self.profile, self.root / 'locks') as reservation:
            self.machine.reservation = reservation
            self.machine.begin_operation('session')
            self.machine.shutdown_current()
            self.assertEqual(json.loads(reservation.read_text())['operation'], 'shutdown_current')
        self.machine.capture.assert_called_once()
        self.assertEqual(self.machine.capture.call_args.args[0][-1], 'shutdown.exe /s /t 0')
        self.machine.vbox.assert_not_called()

    def test_preserving_shutdown_deadline_retains_uncertain_operation_and_never_forces(self):
        self.machine.session_mode = 'maintenance'
        self.machine.state = Mock(return_value={'VMState': 'running'})
        self.machine.capture = Mock(return_value=subprocess.CompletedProcess(['ssh'], 0))
        self.machine.vbox = Mock()
        with vm.ownership(self.profile, self.root / 'locks') as reservation:
            self.machine.reservation = reservation
            with patch.object(vm.time, 'monotonic', side_effect=[0, 121]):
                with self.assertRaises(vm.VmError) as caught:
                    self.machine.shutdown_current()
            self.assertTrue(caught.exception.uncertain)
            self.assertEqual(json.loads(reservation.read_text())['operation'], 'shutdown_current')
        self.machine.capture.assert_called_once()
        self.machine.vbox.assert_not_called()

    def test_snapshot_capture_releases_only_after_new_poweredoff_identity(self):
        self.machine.session_mode = 'maintenance'
        self.machine.state = Mock(side_effect=[self.restored_state(),
            {'VMState': 'poweroff', 'CurrentSnapshotName': 'new-baseline', 'CurrentSnapshotUUID': 'new-id'}])
        self.machine.vbox = Mock()
        with vm.ownership(self.profile, self.root / 'locks') as reservation:
            self.machine.reservation = reservation
            result = self.machine.snapshot('new-baseline')
            self.assertEqual(result['uuid'], 'new-id')
            self.assertFalse(reservation.exists())

    def test_snapshot_timeout_and_interrupt_keep_capture_reservation(self):
        for failure in (vm.VmError('snapshot timed out', uncertain=True), KeyboardInterrupt('interrupted')):
            with self.subTest(failure=failure):
                self.machine.session_mode = 'maintenance'
                self.machine.state = Mock(return_value=self.restored_state())
                self.machine.vbox = Mock(side_effect=failure)
                with vm.ownership(self.profile, self.root / 'locks', allow_pending=True) as reservation:
                    self.machine.reservation = reservation
                    with self.assertRaises(type(failure)):
                        self.machine.snapshot('new-baseline')
                    pending = json.loads(reservation.read_text())
                    self.assertEqual(pending['operation'], 'snapshot')
                    self.assertEqual(pending['previous_snapshot'], 'baseline-id')
                    self.assertEqual(pending['mode'], 'maintenance')
                with self.assertRaises(vm.VmError):
                    with vm.ownership(self.profile, self.root / 'locks'):
                        self.fail('Unconfirmed capture must block another mutation')

    def test_snapshot_unchanged_identity_does_not_release_capture(self):
        self.machine.state = Mock(return_value=self.restored_state())
        self.machine.vbox = Mock()
        with vm.ownership(self.profile, self.root / 'locks') as reservation:
            self.machine.reservation = reservation
            with self.assertRaisesRegex(vm.VmError, 'identity is unconfirmed'):
                self.machine.snapshot('new-baseline')
            self.assertTrue(reservation.exists())

    def test_one_owner_per_vm_even_with_different_profile_aliases(self):
        with vm.ownership(self.profile, self.root / 'locks'):
            with self.assertRaisesRegex(vm.VmError, 'owns this VM'):
                with vm.ownership({**self.profile, 'role': 'manual'}, self.root / 'locks'):
                    self.fail('Concurrent owner admitted')

    def test_environment_does_not_inherit_destructive_staging_or_restore_flags(self):
        with patch.dict(os.environ, {'STAGING_VM_NAME': 'user-vm', 'RESTORE_AFTER_BUILD': '1',
                                    'VERIFY_SAFE_REINSTALL': '1', 'UNINSTALL_AFTER_INSTALL': '1'}):
            env = self.machine.environment()
        self.assertEqual(env['STAGING_VM_NAME'], '')
        self.assertEqual(env['RESTORE_AFTER_BUILD'], '0')
        self.assertEqual(env['RESTORE_BEFORE_BUILD'], '0')
        self.assertEqual(env['VM_LIFECYCLE_EXTERNAL'], '1')
        self.assertNotIn('VERIFY_SAFE_REINSTALL', env)
        self.assertNotIn('UNINSTALL_AFTER_INSTALL', env)

    def test_guest_cache_path_is_profile_owned_and_optional(self):
        with patch.dict(os.environ, {'VM_GUEST_CACHE_ROOT': r'X:\unrelated-cache'}):
            self.assertEqual(self.machine.environment()['VM_GUEST_CACHE_ROOT'], '')
            self.machine.p['guest_cache_root'] = r'X:\build-cache\dependencies with spaces'
            self.assertEqual(self.machine.environment()['VM_GUEST_CACHE_ROOT'], self.machine.p['guest_cache_root'])

    def test_failed_command_is_task_failure_and_retains_exit_code(self):
        with patch.object(vm.subprocess, 'run', return_value=subprocess.CompletedProcess(['test'], 7)):
            with self.assertRaises(vm.VmError) as caught:
                self.machine.run(['test'])
        self.assertEqual(caught.exception.status, 'failed')
        self.assertEqual(self.machine.commands[-1]['exit_code'], 7)

    def test_known_helper_failure_releases_operation_reservation(self):
        with vm.ownership(self.profile, self.root / 'locks') as reservation:
            self.machine.reservation = reservation
            with patch.object(self.machine, 'run', side_effect=vm.VmError('helper exit7', 'failed')):
                with self.assertRaises(vm.VmError) as caught:
                    self.machine.run_operation('test', ['helper'])
            self.assertFalse(caught.exception.uncertain)
            self.assertFalse(reservation.exists())
        with vm.ownership(self.profile, self.root / 'locks'):
            pass

    def test_uncertain_helper_exits_preserve_operation_reservation(self):
        for code in (124, 125, 141, 255, -15):
            with self.subTest(code=code):
                locks = self.root / ('locks-' + str(code))
                with vm.ownership(self.profile, locks) as reservation:
                    self.machine.reservation = reservation
                    with patch.object(self.machine, 'run', side_effect=vm.VmError(f'helper exit{code}', uncertain=True)):
                        with self.assertRaises(vm.VmError) as caught:
                            self.machine.run_operation('test', ['helper'])
                    self.assertTrue(caught.exception.uncertain)
                    self.assertTrue(reservation.exists())
                with self.assertRaisesRegex(vm.VmError, 'Unresolved guest operation'):
                    with vm.ownership(self.profile, locks):
                        self.fail('Uncertain guest work was not reserved')
                self.machine.finish_operation()

    def test_successful_helper_releases_operation_reservation(self):
        with vm.ownership(self.profile, self.root / 'locks') as reservation:
            self.machine.reservation = reservation
            with patch.object(self.machine, 'run', return_value=0):
                self.machine.run_operation('build', ['helper'])
            self.assertFalse(reservation.exists())

    def test_local_helper_launch_failure_releases_operation_reservation(self):
        with vm.ownership(self.profile, self.root / 'locks') as reservation:
            self.machine.reservation = reservation
            with patch.object(self.machine, 'run', side_effect=FileNotFoundError('helper missing')):
                with self.assertRaises(FileNotFoundError):
                    self.machine.run_operation('test', ['helper'])
            self.assertFalse(reservation.exists())

    def test_timeout_does_not_retry(self):
        with patch.object(vm.subprocess, 'run', side_effect=subprocess.TimeoutExpired(['test'], 1)) as run:
            with self.assertRaisesRegex(vm.VmError, 'remote work may still be active'):
                self.machine.run(['test'], timeout=1)
        self.assertEqual(run.call_count, 1)

    def test_interactive_disconnect_reserves_vm_across_invocations(self):
        self.machine.ps = Mock(side_effect=['', vm.VmError('SSH disconnected', uncertain=True)])
        with vm.ownership(self.profile, self.root / 'locks') as reservation:
            self.machine.reservation = reservation
            with self.assertRaisesRegex(vm.VmError, 'SSH disconnected'):
                self.machine.interactive('C:\\probe.ps1', '', 'azookey-vm-test', 30)
            self.assertEqual(json.loads(reservation.read_text())['task'], 'azookey-vm-test')
        with self.assertRaisesRegex(vm.VmError, 'Unresolved guest operation'):
            with vm.ownership(self.profile, self.root / 'locks'):
                self.fail('A later invocation may not mutate an unresolved VM')
        with vm.ownership(self.profile, self.root / 'locks', allow_pending=True):
            self.machine.finish_operation()
        with vm.ownership(self.profile, self.root / 'locks'):
            pass

    def test_interactive_confirmed_completion_releases_reservation(self):
        self.machine.ps = Mock(side_effect=['', json.dumps({'code': '0', 'output': 'complete'}), ''])
        with vm.ownership(self.profile, self.root / 'locks') as reservation:
            self.machine.reservation = reservation
            self.assertEqual(self.machine.interactive('C:\\probe.ps1', '', 'azookey-vm-test', 30), 'complete')
            self.assertFalse(reservation.exists())

    def test_interactive_rejected_launch_without_task_releases_reservation(self):
        self.machine.ps = Mock(side_effect=[vm.VmError('registration failed', 'failed'), 'absent'])
        with vm.ownership(self.profile, self.root / 'locks') as reservation:
            self.machine.reservation = reservation
            with self.assertRaises(vm.VmError) as caught:
                self.machine.interactive('C:\\probe.ps1', '', 'azookey-vm-test', 30)
            self.assertFalse(caught.exception.uncertain)
            self.assertFalse(reservation.exists())

    def test_interactive_rejected_launch_unregisters_inactive_task(self):
        self.machine.ps = Mock(side_effect=[vm.VmError('start failed', 'failed'), 'Ready', '', 'absent'])
        with vm.ownership(self.profile, self.root / 'locks') as reservation:
            self.machine.reservation = reservation
            with self.assertRaises(vm.VmError):
                self.machine.interactive('C:\\probe.ps1', '', 'azookey-vm-test', 30)
            self.assertIn('Unregister-ScheduledTask', self.machine.ps.call_args_list[-2].args[0])
            self.assertFalse(reservation.exists())

    def test_interactive_uncertain_launch_preserves_reservation(self):
        self.machine.ps = Mock(side_effect=vm.VmError('SSH disconnected', uncertain=True))
        with vm.ownership(self.profile, self.root / 'locks') as reservation:
            self.machine.reservation = reservation
            with self.assertRaises(vm.VmError):
                self.machine.interactive('C:\\probe.ps1', '', 'azookey-vm-test', 30)
            self.assertTrue(reservation.exists())
            self.machine.ps.assert_called_once()

    def test_interactive_failed_launch_with_running_task_keeps_reservation(self):
        self.machine.ps = Mock(side_effect=[vm.VmError('start failed', 'failed'), 'Running'])
        with vm.ownership(self.profile, self.root / 'locks') as reservation:
            self.machine.reservation = reservation
            with self.assertRaises(vm.VmError) as caught:
                self.machine.interactive('C:\\probe.ps1', '', 'azookey-vm-test', 30)
            self.assertTrue(caught.exception.uncertain)
            self.assertTrue(reservation.exists())

    def test_interactive_failed_launch_with_failed_cleanup_keeps_reservation(self):
        self.machine.ps = Mock(side_effect=[vm.VmError('start failed', 'failed'), vm.VmError('probe failed', 'failed')])
        with vm.ownership(self.profile, self.root / 'locks') as reservation:
            self.machine.reservation = reservation
            with self.assertRaises(vm.VmError) as caught:
                self.machine.interactive('C:\\probe.ps1', '', 'azookey-vm-test', 30)
            self.assertTrue(caught.exception.uncertain)
            self.assertTrue(reservation.exists())

    def resolve_interactive(self, responses, mode='verification'):
        locks = self.root / '.cache/azookey-vm-locks'
        with vm.ownership(self.profile, locks, allow_pending=True) as reservation:
            self.machine.reservation = reservation
            self.machine.session_mode = mode
            self.machine.begin_operation('interactive', task='azookey-vm-test', workspace=str(self.root))
            pending = json.loads(reservation.read_text())
        output = io.StringIO()
        with patch.object(vm, 'ROOT', self.root), \
                patch.object(vm.Path, 'home', return_value=self.root), \
                patch.object(vm, 'load_profile', return_value=self.profile), \
                patch.object(vm.VM, 'state', return_value={'VMState': 'running', 'UUID': self.profile['vm']}), \
                patch.object(vm.VM, 'operation_status', return_value={
                    'pending': pending, 'guest': {'processes': [], 'running_tasks': []}}), \
                patch.object(vm.VM, 'ps', side_effect=responses) as ps, \
                patch.object(vm.sys, 'argv', ['vm.py', '--profile', 'build', 'resolve-operation', '--reason', 'inspected task']), \
                contextlib.redirect_stdout(output):
            code = vm.main()
        return code, json.loads(output.getvalue()), reservation, ps

    def test_resolve_interactive_requires_confirmed_task_removal(self):
        for responses in (
                ['Ready', vm.VmError('unregister denied', 'failed')],
                ['Ready', '', 'Ready'],
                ['Ready', '', vm.VmError('absence probe denied', 'failed')],
                [vm.VmError('state probe denied', 'failed')],
                ['Running'], ['Queued'], ['Unknown']):
            with self.subTest(responses=responses):
                code, result, reservation, _ = self.resolve_interactive(responses)
                self.assertEqual(code, 1)
                self.assertEqual(result['task_status'], 'blocked')
                self.assertTrue(reservation.exists())
                with self.assertRaisesRegex(vm.VmError, 'Unresolved guest operation'):
                    with vm.ownership(self.profile, self.root / '.cache/azookey-vm-locks'):
                        self.fail('Task removal was not confirmed')

    def test_resolve_interactive_confirmed_removal_allows_next_owner(self):
        for state in ('Ready', 'Disabled'):
            with self.subTest(state=state):
                code, result, reservation, ps = self.resolve_interactive([state, '', 'absent'])
                self.assertEqual(code, 0)
                self.assertEqual(result['task_status'], 'succeeded')
                self.assertFalse(reservation.exists())
                self.assertEqual(ps.call_count, 3)
                self.assertIn('Unregister-ScheduledTask', ps.call_args_list[1].args[0])
                self.assertIn('-ErrorAction Stop', ps.call_args_list[1].args[0])
                with vm.ownership(self.profile, self.root / '.cache/azookey-vm-locks'):
                    pass

    def test_resolve_interactive_absent_task_needs_no_unregister(self):
        code, result, reservation, ps = self.resolve_interactive(['absent'])
        self.assertEqual(code, 0)
        self.assertEqual(result['task_status'], 'succeeded')
        self.assertFalse(reservation.exists())
        ps.assert_called_once()
        self.assertNotIn('Unregister-ScheduledTask', ps.call_args.args[0])

    def test_resolved_maintenance_operation_keeps_session_for_next_probes(self):
        code, result, reservation, _ = self.resolve_interactive(['absent'], mode='maintenance')
        self.assertEqual(code, 0)
        self.assertEqual(result['task_status'], 'succeeded')
        session = json.loads(reservation.read_text())
        self.assertEqual(session['operation'], 'session')
        self.assertEqual(session['mode'], 'maintenance')
        self.assertEqual(session['workspace'], str(self.root))
        for args in (['doctor'], ['exec', '--script', 'probe.ps1']):
            code, _, reservation, events = self.lifecycle_command(args, session)
            self.assertEqual(code, 0)
            self.assertEqual(events, [args[0]])
            session = json.loads(reservation.read_text())
            self.assertEqual(session['mode'], 'maintenance')

    def test_script_uploads_japanese_utf8_with_bom(self):
        source = self.root / 'probe.ps1'
        source.write_text("Write-Output '日本語確認'\n", encoding='utf-8')
        self.machine.start = Mock()
        self.machine.transfer = Mock()
        self.machine.ps = Mock(return_value='日本語確認')
        self.assertEqual(self.machine.script(source), '日本語確認')
        uploaded = Path(self.machine.transfer.call_args.args[0])
        self.assertEqual(uploaded.read_bytes(), b'\xef\xbb\xbf' + source.read_bytes())

    def test_invalid_script_encoding_rejected_before_guest_mutation(self):
        source = self.root / 'probe.ps1'
        source.write_bytes(b'\xff\xfeinvalid utf8')
        self.machine.start = Mock()
        with self.assertRaises(UnicodeError):
            self.machine.script(source)
        self.machine.start.assert_not_called()

    def test_script_exit_failure_releases_but_disconnect_retains_reservation(self):
        source = self.root / 'probe.ps1'
        source.write_text('exit 7')
        self.machine.start = Mock()
        self.machine.transfer = Mock()
        for uncertain, hold_pending in ((False, False), (True, False), (False, True), (True, True)):
            with self.subTest(uncertain=uncertain, hold_pending=hold_pending):
                self.machine.ps = Mock(side_effect=vm.VmError('failed script', 'failed', uncertain=uncertain))
                with vm.ownership(self.profile, self.root / 'locks') as reservation:
                    self.machine.reservation = reservation
                    with self.assertRaises(vm.VmError):
                        self.machine.script(source, hold_pending=hold_pending)
                    self.assertEqual(reservation.exists(), uncertain)
                self.machine.finish_operation()

    def test_install_success_output_is_reserved_until_json_parses(self):
        self.machine.start = Mock()
        self.machine.transfer = Mock()
        self.machine.ps = Mock(return_value='not JSON')
        with vm.ownership(self.profile, self.root / 'locks') as reservation:
            self.machine.reservation = reservation
            with self.assertRaises(ValueError):
                self.machine.guest('install')
            self.assertTrue(reservation.exists())

    def test_guest_timeout_status_is_uncertain(self):
        with patch.object(vm.subprocess, 'run', return_value=subprocess.CompletedProcess(['ssh'], 124, '', 'installer timeout')):
            with self.assertRaises(vm.VmError) as caught:
                self.machine.capture(['ssh'])
        self.assertTrue(caught.exception.uncertain)
        self.assertEqual(caught.exception.status, 'blocked')

    def test_capture_timeout_records_indeterminate_outcome(self):
        with patch.object(vm.subprocess, 'run', side_effect=subprocess.TimeoutExpired(['ssh'], 1)):
            with self.assertRaises(vm.VmError) as caught:
                self.machine.capture(['ssh'], timeout=1)
        self.assertTrue(caught.exception.uncertain)
        self.assertEqual(self.machine.commands[-1]['status'], 'timeout')

    def test_stage_checks_metadata_before_mutation(self):
        installer = self.root / 'setup.exe'
        installer.write_bytes(b'installer fixture')
        installer.with_suffix('.build-info.json').write_text('{}')
        self.machine.start = Mock()
        with self.assertRaisesRegex(vm.VmError, 'identity'):
            self.machine.stage(installer, False)
        self.machine.start.assert_not_called()

    def test_stage_reports_identity_before_outer_lifecycle_resets(self):
        installer = self.root / 'setup.exe'
        installer.write_bytes(b'installer fixture')
        identity = dict(version='test-version', channel='validation', revision='test-sha', installerGeneration=2)
        installer.with_suffix('.build-info.json').write_text(json.dumps(identity))
        self.machine.start = Mock()
        self.machine.transfer = Mock()
        self.machine.guest = Mock(return_value={'installed': identity})
        self.machine.shutdown = Mock()
        self.machine.vbox = Mock()
        self.machine.stage(installer, False)
        self.machine.shutdown.assert_not_called()
        self.machine.vbox.assert_not_called()
        self.assertEqual(json.loads(self.machine.guest.call_args.kwargs['Expected']), identity)
        self.machine.stage(installer, True)
        self.machine.shutdown.assert_not_called()

    def test_stage_recovers_log_for_completed_install_but_not_ssh_failure(self):
        installer = self.root / 'setup.exe'
        installer.write_bytes(b'installer fixture')
        identity = dict(version='test-version', channel='validation', revision='test-sha', installerGeneration=2)
        installer.with_suffix('.build-info.json').write_text(json.dumps(identity))
        for error in (None, vm.VmError('installer exit7', 'failed'), vm.VmError('SSH failed', uncertain=True)):
            with self.subTest(error=error):
                self.machine.evidence = []
                self.machine.start = Mock()
                def transfer(source, destination, download=False, **kwargs):
                    if download:
                        Path(destination).write_text('installer diagnostic evidence')
                self.machine.transfer = Mock(side_effect=transfer)
                self.machine.guest = Mock(side_effect=error, return_value={'installed': identity})
                if error:
                    with self.assertRaises(vm.VmError) as caught:
                        self.machine.stage(installer, False)
                    self.assertIs(caught.exception, error)
                else:
                    self.machine.stage(installer, False)
                recovered = error is None or not error.uncertain
                self.assertEqual(self.machine.transfer.call_count, 2 if recovered else 1)
                self.assertEqual(self.machine.evidence[-1]['task_status'], 'succeeded' if recovered else 'not_run')

    def test_install_log_transport_timeout_fails_and_resets_instead_of_leaving_a_session(self):
        installer = self.root / 'setup.exe'
        installer.write_bytes(b'installer fixture')
        identity = dict(version='test-version', channel='validation', revision='test-sha', installerGeneration=2)
        installer.with_suffix('.build-info.json').write_text(json.dumps(identity))
        self.machine.start = Mock()
        self.machine.guest = Mock(return_value={'installed': identity, 'installer_exit': 0})
        self.machine.transfer = Mock(side_effect=[None, vm.VmError('log SCP timed out', uncertain=True)])
        self.machine.reset = Mock(side_effect=lambda *args, **kwargs: self.machine.finish_operation())
        with vm.ownership(self.profile, self.root / 'locks') as reservation:
            self.machine.reservation = reservation
            with self.assertRaisesRegex(vm.VmError, 'log SCP timed out'):
                with self.machine.clean_run(keep=True):
                    self.machine.stage(installer, True)
            self.assertFalse(reservation.exists())
            self.assertEqual(self.machine.reset.call_args.args, ('failed run; discard guest immediately',))
            self.assertEqual(self.machine.evidence[-1]['installation']['installed'], identity)

    def test_controller_scp_timeout_also_terminates_ssh_child(self):
        bin_dir = self.root / 'bin'
        bin_dir.mkdir()
        stub = bin_dir / 'scp'
        stub.write_text('#!' + sys.executable + '\n'
                        'import subprocess,sys,time\n'
                        'p=subprocess.Popen([sys.executable,"-c","import time; time.sleep(60)"])\n'
                        'print(p.pid,flush=True)\n'
                        'time.sleep(60)\n')
        stub.chmod(0o755)
        with patch.dict(os.environ, {'PATH': str(bin_dir) + os.pathsep + os.environ['PATH']}):
            with self.assertRaises(vm.VmError) as caught:
                self.machine.transfer(str(self.key), r'C:\fixture.bin', timeout=0.5)
        self.assertTrue(caught.exception.uncertain)
        child_pid = next(line for line in self.machine.log.read_text().splitlines() if line.isdecimal())
        proc_stat = Path('/proc') / child_pid / 'stat'
        for _ in range(100):
            try:
                dead = proc_stat.read_text().split()[2] == 'Z'
            except FileNotFoundError:
                dead = True
            if dead:
                break
            vm.time.sleep(0.01)
        else:
            self.fail('SCP SSH child survived the deadline')

    def test_powershell_quote_does_not_inject(self):
        self.assertEqual(vm.ps_quote("C:\\user's file.ps1"), "'C:\\user''s file.ps1'")

    def test_transfer_rejects_relative_remote_path_before_running_scp(self):
        self.machine.run = Mock()
        with self.assertRaisesRegex(vm.VmError, 'absolute Windows path'):
            self.machine.transfer(str(self.key), 'relative.txt')
        self.machine.run.assert_not_called()


if __name__ == '__main__':
    unittest.main()
