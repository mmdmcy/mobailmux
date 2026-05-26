from __future__ import annotations

import os
import tempfile
import unittest
from pathlib import Path

from mobailmux.web import MobailmuxWeb


class WebCommandHandlingTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.default_workdir = Path(self.temp_dir.name) / "default"
        self.project_workdir = Path(self.temp_dir.name) / "project"
        self.default_workdir.mkdir()
        self.project_workdir.mkdir()
        self.previous_env = {
            key: os.environ.get(key)
            for key in (
                "MOBAILMUX_STATE_DIR",
                "MOBAILMUX_WEB_DB",
                "MOBAILMUX_SLOTS",
                "MOBAILMUX_SLOT_ONE_WORKDIR",
            )
        }
        os.environ["MOBAILMUX_STATE_DIR"] = self.temp_dir.name
        os.environ["MOBAILMUX_WEB_DB"] = os.path.join(self.temp_dir.name, "web.sqlite3")
        os.environ["MOBAILMUX_SLOTS"] = f"one:one:{self.default_workdir}"
        os.environ.pop("MOBAILMUX_SLOT_ONE_WORKDIR", None)
        self.runtime = MobailmuxWeb()

    def tearDown(self) -> None:
        for key, value in self.previous_env.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        self.temp_dir.cleanup()

    def test_bare_commands_are_plain_messages(self) -> None:
        self.assertFalse(self.runtime.handle_control_message("one", "help"))
        self.assertFalse(self.runtime.handle_control_message("one", "cd"))

    def test_prefixed_commands_are_controls(self) -> None:
        self.assertTrue(self.runtime.handle_control_message("one", "!help"))
        self.assertTrue(self.runtime.handle_control_message("one", "!cd"))
        self.assertEqual(self.runtime.current_workdir("one"), os.path.expanduser("~"))

    def test_fresh_resets_workdir_to_slot_default(self) -> None:
        self.assertTrue(self.runtime.handle_control_message("one", f"!cd {self.project_workdir}"))
        self.assertEqual(self.runtime.current_workdir("one"), str(self.project_workdir))

        self.assertTrue(self.runtime.handle_control_message("one", "!fresh"))

        self.assertEqual(self.runtime.current_workdir("one"), str(self.default_workdir))

    def test_stayfresh_keeps_current_workdir(self) -> None:
        self.assertTrue(self.runtime.handle_control_message("one", f"!cd {self.project_workdir}"))
        self.runtime.set_session("one", "thread-id", str(self.project_workdir))

        self.assertTrue(self.runtime.handle_control_message("one", "!stayfresh"))

        self.assertEqual(self.runtime.current_workdir("one"), str(self.project_workdir))
        self.assertEqual(self.runtime.current_session("one"), {})
        self.assertIn(f"Folder kept at `{self.project_workdir}`", self.runtime.recent_messages()[-1]["message"])

    def test_status_reports_session_folder(self) -> None:
        self.assertTrue(self.runtime.handle_control_message("one", f"!cd {self.project_workdir}"))
        self.runtime.set_session("one", "thread-id", str(self.project_workdir))

        self.assertTrue(self.runtime.handle_control_message("one", "!status"))

        message = self.runtime.recent_messages()[-1]["message"]
        self.assertIn(f"Current folder: `{self.project_workdir}`", message)
        self.assertIn(f"Session folder: `{self.project_workdir}`", message)


if __name__ == "__main__":
    unittest.main()
