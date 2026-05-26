from __future__ import annotations

import os
import tempfile
import unittest

from mobailmux.web import MobailmuxWeb


class WebCommandHandlingTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.previous_env = {
            key: os.environ.get(key)
            for key in ("MOBAILMUX_STATE_DIR", "MOBAILMUX_WEB_DB", "MOBAILMUX_SLOTS")
        }
        os.environ["MOBAILMUX_STATE_DIR"] = self.temp_dir.name
        os.environ["MOBAILMUX_WEB_DB"] = os.path.join(self.temp_dir.name, "web.sqlite3")
        os.environ["MOBAILMUX_SLOTS"] = "one:one"
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


if __name__ == "__main__":
    unittest.main()
