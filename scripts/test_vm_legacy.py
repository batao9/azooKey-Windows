#!/usr/bin/env python3
"""Regression checks for the legacy VM scripts and their local controller wrappers."""

import os
from pathlib import Path
import contextlib
import io
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / "scripts"
LOCAL = ROOT / ".local"


def sanitized_env():
    env = os.environ.copy()
    for key in list(env):
        if key.startswith(("RESTORE_", "DISCARD_SAVED_STATE_", "VERIFY_")) or key in {
            "STAGING_VM_NAME",
            "UNINSTALL_AFTER_INSTALL",
            "SHUTDOWN_AFTER_INSTALL",
            "INSTALL_DIR_WIN",
            "DEEP_DLL_REGISTRATION_DIR_WIN",
            "INSTALL_TIMEOUT_SEC",
            "VM_NAME",
            "SNAPSHOT_NAME",
            "SSH_USER",
            "SSH_PORT",
            "SSH_KEY",
            "SSH_HOST",
            "VBOX_MANAGE",
        }:
            env.pop(key, None)
    return env


class VmLegacyScriptTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.build = (SCRIPTS / "vm_build.sh").read_text()
        cls.test = (SCRIPTS / "vm_test_client_composition.sh").read_text()
        cls.stage = (SCRIPTS / "vm_stage_for_manual_test.sh").read_text()

    def test_invalid_cargo_package_rejected_before_vm_operations(self):
        env = sanitized_env()
        env['CARGO_TEST_PACKAGE'] = 'frontend;unexpected-command'
        result = subprocess.run(['bash', str(SCRIPTS / 'vm_test_client_composition.sh'), 'feature/example'],
                                cwd=ROOT, env=env, text=True, capture_output=True)
        self.assertEqual(result.returncode, 1)
        self.assertIn('valid Cargo package name', result.stdout)

    def test_invalid_real_zenzai_flag_rejected_before_vm_operations(self):
        env = sanitized_env()
        env['SWIFT_TEST_REAL_ZENZAI'] = 'unexpected'
        result = subprocess.run(['bash', str(SCRIPTS / 'vm_test_client_composition.sh'), 'feature/example', 'skip', 'all'],
                                cwd=ROOT, env=env, text=True, capture_output=True)
        self.assertEqual(result.returncode, 1)
        self.assertIn('SWIFT_TEST_REAL_ZENZAI must be 0 or 1', result.stdout)

    def make_python_stub(self, temp_dir):
        bin_dir = temp_dir / "bin"
        bin_dir.mkdir()
        stub = bin_dir / "python3"
        stub.write_text(
            "#!/usr/bin/env bash\n"
            "set -euo pipefail\n"
            "if [[ \"${1:-}\" == \"-c\" ]]; then\n"
            "  printf '%s\\n' \"$MOCK_LATEST_INSTALLER\"\n"
            "else\n"
            "  printf '%s\\0' \"$@\" > \"$MOCK_ARGS\"\n"
            "  printf '%s' \"${CARGO_TEST_PACKAGE:-}\" > \"$MOCK_PACKAGE\"\n"
            "fi\n"
        )
        stub.chmod(0o755)
        return bin_dir

    def run_wrapper(self, wrapper, args=(), extra_env=None, latest="/tmp/latest-setup.exe"):
        with tempfile.TemporaryDirectory() as temp:
            temp_dir = Path(temp)
            bin_dir = self.make_python_stub(temp_dir)
            captures = temp_dir / "args"
            package_capture = temp_dir / "package"
            env = sanitized_env()
            env.update(
                PATH=f"{bin_dir}:{env.get('PATH', '')}",
                MOCK_ARGS=str(captures),
                MOCK_PACKAGE=str(package_capture),
                MOCK_LATEST_INSTALLER=latest,
            )
            if extra_env:
                env.update(extra_env)
            proc = subprocess.run(
                ["bash", str(wrapper), *args],
                cwd=ROOT,
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )
            argv = captures.read_bytes().rstrip(b"\0").decode().split("\0") if captures.exists() else None
            package = package_capture.read_text() if package_capture.exists() else None
            return proc, argv, package

    @unittest.skipUnless((LOCAL / 'vm_build_master.sh').is_file() and (LOCAL / 'vm_test_client_composition.sh').is_file(), 'Local compatibility wrappers are not installed')
    def test_local_build_and_test_wrappers_dispatch_and_reject_restore_intent(self):
        proc, argv, _ = self.run_wrapper(LOCAL / "vm_build_master.sh", ["feature/example"])
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertTrue(argv[0].endswith("/scripts/vm.py"))
        self.assertEqual(argv[1:], ["--profile", "build", "build", "feature/example"])

        proc, argv, package = self.run_wrapper(
            LOCAL / "vm_test_client_composition.sh",
            ["feature/example", "frontend_filter", "skip"],
            {"CARGO_TEST_PACKAGE": "frontend"},
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertTrue(argv[0].endswith("/scripts/vm.py"))
        self.assertEqual(argv[1:], ["--profile", "build", "test", "feature/example", "frontend_filter", "skip"])
        self.assertEqual(package, "frontend")

        proc, argv, _ = self.run_wrapper(
            LOCAL / "vm_build_master.sh", ["feature/example"], {"RESTORE_AFTER_BUILD": "1"}
        )
        self.assertEqual(proc.returncode, 2)
        self.assertIsNone(argv)
        self.assertIn("restore", proc.stderr)

    @unittest.skipUnless((LOCAL / 'vm_stage_for_manual_test.sh').is_file(), 'Local compatibility wrapper is not installed')
    def test_local_stage_wrapper_resolves_latest_and_maps_shutdown_flag(self):
        wrapper = (LOCAL / "vm_stage_for_manual_test.sh").read_text()
        self.assertIn("path.stat().st_mtime_ns, path.name", wrapper)
        self.assertIn("if not candidates:", wrapper)
        selector = wrapper.split("installer=\"$(python3 -c '", 1)[1].split("' \"$ARTIFACT_DIR\")\"", 1)[0]
        compile(selector, "<latest-installer-selector>", "exec")
        with tempfile.TemporaryDirectory() as temp:
            artifacts = Path(temp)
            older = artifacts / "azookey-setup-a.exe"
            tied_a = artifacts / "azookey-setup-a-tie.exe"
            tied_z = artifacts / "azookey-setup-z-tie.exe"
            for artifact in (older, tied_a, tied_z):
                artifact.write_bytes(b"mock installer")
            os.utime(older, ns=(10, 10))
            os.utime(tied_a, ns=(20, 20))
            os.utime(tied_z, ns=(20, 20))
            original_argv = sys.argv
            output = io.StringIO()
            try:
                sys.argv = ["latest-installer-selector", str(artifacts)]
                with contextlib.redirect_stdout(output):
                    exec(compile(selector, "<latest-installer-selector>", "exec"), {})
            finally:
                sys.argv = original_argv
            self.assertEqual(output.getvalue().strip(), str(tied_z.resolve()))

        proc, argv, _ = self.run_wrapper(
            LOCAL / "vm_stage_for_manual_test.sh",
            ["latest"],
            {"SHUTDOWN_AFTER_INSTALL": "0"},
            latest="/tmp/artifact selected deterministically.exe",
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertTrue(argv[0].endswith("/scripts/vm.py"))
        self.assertEqual(
            argv[1:],
            ["--profile", "manual", "stage", "/tmp/artifact selected deterministically.exe", "--leave-running"],
        )

        proc, argv, _ = self.run_wrapper(
            LOCAL / "vm_stage_for_manual_test.sh", ["latest"], {"VERIFY_TASK_RUN_FAILURE": "1"}
        )
        self.assertEqual(proc.returncode, 2)
        self.assertIsNone(argv)
        self.assertIn("fixture", proc.stderr)

    def test_external_lifecycle_rejects_conflicting_restore_before_vm_tools(self):
        env = sanitized_env()
        env.update(
            VM_NAME="a-vm-uuid",
            SSH_USER="testuser",
            SSH_PORT="2222",
            SSH_KEY="/missing/ssh-key",
            VBOX_MANAGE="/missing/VBoxManage",
            VM_LIFECYCLE_EXTERNAL="1",
            RESTORE_AFTER_BUILD="1",
        )
        proc = subprocess.run(
            ["bash", str(SCRIPTS / "vm_build.sh"), "feature/agent-vm-operations"],
            cwd=ROOT,
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(proc.returncode, 1)
        self.assertIn("外部 VM lifecycle 中", proc.stdout + proc.stderr)

    def test_public_stage_blocks_destructive_fixture_without_restore(self):
        with tempfile.TemporaryDirectory() as temp:
            temp_dir = Path(temp)
            vbox_marker = temp_dir / "vbox-called"
            vbox = temp_dir / "VBoxManage"
            vbox.write_text(f"#!/usr/bin/env bash\nprintf called > {vbox_marker}\n")
            vbox.chmod(0o755)
            key = temp_dir / "ssh-key"
            key.write_text("mock key")
            installer = temp_dir / "installer.exe"
            installer.write_text("not executed")

            env = sanitized_env()
            env.update(
                VM_NAME="fixture-test-vm",
                SSH_USER="testuser",
                SSH_PORT="2222",
                SSH_KEY=str(key),
                VBOX_MANAGE=str(vbox),
                RESTORE_BEFORE_STAGE="0",
                VERIFY_TASK_RUN_FAILURE="1",
            )
            proc = subprocess.run(
                ["bash", str(SCRIPTS / "vm_stage_for_manual_test.sh"), str(installer)],
                cwd=ROOT,
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(proc.returncode, 1)
            self.assertIn("破壊的な fixture", proc.stdout + proc.stderr)
            self.assertFalse(vbox_marker.exists())


if __name__ == "__main__":
    unittest.main()
