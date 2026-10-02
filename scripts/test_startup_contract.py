"""Local startup-task regression checks: python3 scripts/test_startup_contract.py."""

from pathlib import Path
import re
import unittest
import xml.etree.ElementTree as ET


ROOT = Path(__file__).resolve().parents[1]
NS = {"t": "http://schemas.microsoft.com/windows/2004/02/mit/task"}


class StartupTaskTests(unittest.TestCase):
    def setUp(self):
        self.task = ET.parse(ROOT / "installer" / "Azookey Startup.xml").getroot()

    def text(self, path):
        return self.task.findtext(path, namespaces=NS)

    def test_all_interactive_users_run_without_elevation(self):
        principals = self.task.findall("t:Principals/t:Principal", NS)
        self.assertEqual(len(principals), 1)
        self.assertEqual(self.text("t:Principals/t:Principal/t:GroupId"), "S-1-5-32-545")
        self.assertEqual(self.text("t:Principals/t:Principal/t:RunLevel"), "LeastPrivilege")
        self.assertIsNone(self.text("t:Principals/t:Principal/t:UserId"))
        triggers = self.task.findall("t:Triggers/*", NS)
        self.assertEqual(len(triggers), 1)
        self.assertEqual(triggers[0].tag, f"{{{NS['t']}}}LogonTrigger")
        self.assertEqual(self.text("t:Triggers/t:LogonTrigger/t:Enabled"), "true")
        self.assertIsNone(self.text("t:Triggers/t:LogonTrigger/t:UserId"))

    def test_concurrent_sessions_and_battery_do_not_stop_resident_launcher(self):
        for setting, expected in {
            "MultipleInstancesPolicy": "Parallel",
            "DisallowStartIfOnBatteries": "false",
            "StopIfGoingOnBatteries": "false",
            "ExecutionTimeLimit": "PT0S",
            "Enabled": "true",
            "RunOnlyIfIdle": "false",
            "RunOnlyIfNetworkAvailable": "false",
            "DisallowStartOnRemoteAppSession": "false",
        }.items():
            with self.subTest(setting=setting):
                self.assertEqual(self.text(f"t:Settings/t:{setting}"), expected)

    def test_installer_binds_direct_launcher_action_to_principal(self):
        actions = self.task.find("t:Actions", NS)
        principal = self.task.find("t:Principals/t:Principal", NS)
        self.assertEqual(actions.get("Context"), principal.get("id"))
        self.assertEqual(len(actions), 1)
        self.assertEqual(actions[0].tag, f"{{{NS['t']}}}Exec")
        self.assertEqual(self.text("t:Actions/t:Exec/t:Command"), "PATH_TO_LAUNCHER")
        self.assertEqual(
            self.text("t:Actions/t:Exec/t:WorkingDirectory"), "PATH_TO_WORKING_DIRECTORY"
        )
        self.assertIsNone(self.text("t:Actions/t:Exec/t:Arguments"))

    def test_ipc_callers_use_shared_session_paths_without_fixed_aliases(self):
        callers = {
            "crates/server/src/main.rs": ("server_pipe_path",),
            "crates/ui/src/main.rs": ("ui_pipe_path",),
            "crates/launcher/src/main.rs": ("launcher_pipe_path",),
            "crates/client/src/engine/ipc_service.rs": ("server_pipe_path", "ui_pipe_path"),
            "crates/client/src/launcher_control.rs": ("launcher_pipe_path",),
            "crates/client/tests/rpc_composition_isolation.rs": ("server_pipe_path",),
            "frontend/src-tauri/src/ipc.rs": ("server_pipe_path",),
            "frontend/src-tauri/src/server_process.rs": ("launcher_pipe_path",),
        }
        for path, accessors in callers.items():
            source = (ROOT / path).read_text(encoding="utf-8")
            for accessor in accessors:
                with self.subTest(path=path, accessor=accessor):
                    self.assertRegex(source, rf"\b{accessor}\(")
                    self.assertRegex(
                        source, rf"\bshared::(?:{accessor}\b|\{{[^}}]*\b{accessor}\b)"
                    )

        forbidden = re.compile(
            r'\b(?:SERVER|UI|LAUNCHER)_PIPE_PATH\b|\\+azookey_(?:server|ui|launcher)"'
        )
        for source_root in ("crates", "frontend/src-tauri/src"):
            for path in (ROOT / source_root).rglob("*.rs"):
                with self.subTest(path=path.relative_to(ROOT)):
                    self.assertNotRegex(path.read_text(encoding="utf-8"), forbidden)


if __name__ == "__main__":
    unittest.main()
