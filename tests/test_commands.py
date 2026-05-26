from __future__ import annotations

import unittest

from mobailmux.commands import cd_target_arg, parse_command_message, queue_request_arg, unknown_command_text


class CommandParsingTest(unittest.TestCase):
    def test_bare_cd_defaults_to_home(self) -> None:
        self.assertEqual(cd_target_arg("cd"), "~")
        self.assertEqual(cd_target_arg("cd   "), "~")

    def test_cd_with_target_preserves_target(self) -> None:
        self.assertEqual(cd_target_arg("cd Documents/github"), "Documents/github")
        self.assertEqual(cd_target_arg("!cd Documents/github"), None)

    def test_folder_and_workdir_still_require_targets(self) -> None:
        self.assertIsNone(cd_target_arg("folder"))
        self.assertEqual(cd_target_arg("folder /tmp"), "/tmp")
        self.assertEqual(cd_target_arg("workdir ~/code"), "~/code")

    def test_command_prefix_is_removed(self) -> None:
        command = parse_command_message("!cd Documents")
        self.assertTrue(command.explicit)
        self.assertEqual(command.text, "cd Documents")
        self.assertEqual(cd_target_arg(command.text), "Documents")

    def test_prefixed_bare_cd_defaults_to_home(self) -> None:
        command = parse_command_message("!cd")
        self.assertTrue(command.explicit)
        self.assertEqual(cd_target_arg(command.text), "~")

    def test_normal_message_is_not_explicit(self) -> None:
        command = parse_command_message("please explain !cd")
        self.assertFalse(command.explicit)
        self.assertEqual(command.text, "please explain !cd")

    def test_queue_request_arg(self) -> None:
        self.assertEqual(queue_request_arg("next run tests"), "run tests")
        self.assertEqual(queue_request_arg("queue fix lint"), "fix lint")
        self.assertEqual(queue_request_arg(parse_command_message("!next run tests").text), "run tests")
        self.assertIsNone(queue_request_arg("queue"))

    def test_unknown_command_mentions_normal_message_escape(self) -> None:
        text = unknown_command_text("wat")
        self.assertIn("!help", text)
        self.assertIn("without `!`", text)


if __name__ == "__main__":
    unittest.main()
