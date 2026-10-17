"""Behavioral tests of VM boundaries; never contact a VM or SSH endpoint."""
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
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
                            key=str(self.key), vbox='not-a-real-vbox', role='build')
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

    def test_shutdown_timeout_never_force_powers_off_or_restores(self):
        self.machine.state = Mock(return_value={'VMState': 'running'})
        self.machine.ps = Mock()
        self.machine.vbox = Mock()
        with patch.object(vm.time, 'monotonic', side_effect=[0, 181]):
            with self.assertRaisesRegex(vm.VmError, 'Normal shutdown'):
                self.machine.shutdown()
        self.machine.vbox.assert_not_called()
        self.assertNotIn('/f', self.machine.ps.call_args.args[0])

    def test_powered_off_shutdown_is_idempotent(self):
        self.machine.state = Mock(return_value={'VMState': 'poweroff'})
        self.machine.ps = Mock()
        self.machine.shutdown()
        self.machine.ps.assert_not_called()

    def test_recovery_prefers_normal_shutdown_and_clears_confirmed_reservation(self):
        self.machine.shutdown = Mock()
        self.machine.vbox = Mock()
        self.machine.state = Mock(return_value={'VMState': 'poweroff'})
        with vm.ownership(self.profile, self.root / 'locks') as reservation:
            self.machine.reservation = reservation
            self.machine.begin_operation('build')
            result = self.machine.recover_poweroff('owned stalled build')
            self.assertFalse(reservation.exists())
        self.machine.shutdown.assert_called_once()
        self.machine.vbox.assert_not_called()
        self.assertEqual(result['recovery_method'], 'normal_shutdown')
        self.assertEqual(result['interrupted_operation']['operation'], 'build')

    def test_explicit_recovery_forces_only_after_normal_shutdown_fails(self):
        order = []
        def blocked_shutdown():
            order.append('normal')
            raise vm.VmError('shutdown blocked')
        self.machine.shutdown = Mock(side_effect=blocked_shutdown)
        self.machine.vbox = Mock(side_effect=lambda *args: order.append(args[-1]))
        self.machine.state = Mock(return_value={'VMState': 'poweroff'})
        result = self.machine.recover_poweroff('owned unresponsive build guest')
        self.assertEqual(order, ['normal', 'poweroff'])
        self.assertEqual(result['recovery_method'], 'poweroff')

    def test_unconfirmed_recovery_preserves_pending_reservation(self):
        self.machine.shutdown = Mock(side_effect=vm.VmError('shutdown blocked'))
        self.machine.vbox = Mock()
        self.machine.state = Mock(return_value={'VMState': 'running'})
        with vm.ownership(self.profile, self.root / 'locks') as reservation:
            self.machine.reservation = reservation
            self.machine.begin_operation('build')
            with self.assertRaisesRegex(vm.VmError, 'did not complete'):
                self.machine.recover_poweroff('owned stalled build')
            self.assertTrue(reservation.exists())

    def test_explicit_recovery_refuses_manual_vm(self):
        self.machine.p['role'] = 'manual'
        self.machine.shutdown = Mock()
        with self.assertRaisesRegex(vm.VmError, 'build/disposable'):
            self.machine.recover_poweroff('not a disposable guest')
        self.machine.shutdown.assert_not_called()

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
            with patch.object(vm.subprocess, 'run', return_value=subprocess.CompletedProcess(['helper'], 7)):
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
                    with patch.object(vm.subprocess, 'run', return_value=subprocess.CompletedProcess(['helper'], code)):
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
            with patch.object(vm.subprocess, 'run', return_value=subprocess.CompletedProcess(['helper'], 0)):
                self.machine.run_operation('build', ['helper'])
            self.assertFalse(reservation.exists())

    def test_local_helper_launch_failure_releases_operation_reservation(self):
        with vm.ownership(self.profile, self.root / 'locks') as reservation:
            self.machine.reservation = reservation
            with patch.object(vm.subprocess, 'run', side_effect=FileNotFoundError('helper missing')):
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

    def resolve_interactive(self, responses):
        locks = self.root / '.cache/azookey-vm-locks'
        with vm.ownership(self.profile, locks, allow_pending=True) as reservation:
            self.machine.reservation = reservation
            self.machine.begin_operation('interactive', task='azookey-vm-test')
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

    def test_stage_preserves_disk_and_shuts_down_by_default(self):
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
        self.machine.shutdown.assert_called_once()
        self.machine.vbox.assert_not_called()
        self.assertEqual(json.loads(self.machine.guest.call_args.kwargs['Expected']), identity)
        self.machine.shutdown.reset_mock()
        self.machine.stage(installer, True)
        self.machine.shutdown.assert_not_called()

    def test_powershell_quote_does_not_inject(self):
        self.assertEqual(vm.ps_quote("C:\\user's file.ps1"), "'C:\\user''s file.ps1'")

    def test_transfer_rejects_relative_remote_path_before_running_scp(self):
        self.machine.run = Mock()
        with self.assertRaisesRegex(vm.VmError, 'absolute Windows path'):
            self.machine.transfer(str(self.key), 'relative.txt')
        self.machine.run.assert_not_called()


if __name__ == '__main__':
    unittest.main()
