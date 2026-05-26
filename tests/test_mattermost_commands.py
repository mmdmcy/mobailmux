from __future__ import annotations

import importlib
import os
import sys
import tempfile
import unittest
from pathlib import Path


class MattermostCommandHandlingTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.default_workdir = Path(self.temp_dir.name) / "default"
        self.project_workdir = Path(self.temp_dir.name) / "project"
        self.default_workdir.mkdir()
        self.project_workdir.mkdir()
        self.previous_env = {
            key: os.environ.get(key)
            for key in (
                "MOBAILMUX_MATTERMOST_URL",
                "MOBAILMUX_BOT_TOKEN",
                "MOBAILMUX_TEAM_NAME",
                "MOBAILMUX_STATE_DIR",
                "MOBAILMUX_SLOTS",
                "MOBAILMUX_SLOT_ONE_WORKDIR",
            )
        }
        os.environ["MOBAILMUX_MATTERMOST_URL"] = "http://mattermost.test"
        os.environ["MOBAILMUX_BOT_TOKEN"] = "test-token"
        os.environ["MOBAILMUX_TEAM_NAME"] = "agents"
        os.environ["MOBAILMUX_STATE_DIR"] = self.temp_dir.name
        os.environ["MOBAILMUX_SLOTS"] = f"one:one:{self.default_workdir}"
        os.environ.pop("MOBAILMUX_SLOT_ONE_WORKDIR", None)
        sys.modules.pop("mobailmux.app", None)
        self.app = importlib.import_module("mobailmux.app")
        self.posts: list[str] = []
        self.app.post_slot = lambda slot, channel, message: self.posts.append(message)

    def tearDown(self) -> None:
        sys.modules.pop("mobailmux.app", None)
        for key, value in self.previous_env.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        self.temp_dir.cleanup()

    def test_fresh_resets_workdir_to_slot_default(self) -> None:
        self.app.set_workdir("one", str(self.project_workdir))
        self.app.set_session("one", "thread-id", str(self.project_workdir))

        self.assertTrue(self.app.handle_control_message("one", "channel-id", "!fresh"))

        self.assertEqual(self.app.current_workdir("one"), str(self.default_workdir))
        self.assertEqual(self.app.current_session("one"), {})
        self.assertIn(f"Folder reset to `{self.default_workdir}`", self.posts[-1])

    def test_stayfresh_keeps_current_workdir(self) -> None:
        self.app.set_workdir("one", str(self.project_workdir))
        self.app.set_session("one", "thread-id", str(self.project_workdir))

        self.assertTrue(self.app.handle_control_message("one", "channel-id", "!stayfresh"))

        self.assertEqual(self.app.current_workdir("one"), str(self.project_workdir))
        self.assertEqual(self.app.current_session("one"), {})
        self.assertIn(f"Folder kept at `{self.project_workdir}`", self.posts[-1])


if __name__ == "__main__":
    unittest.main()
