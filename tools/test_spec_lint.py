"""Regression tests for local Markdown link validation (stdlib only)."""
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import spec_lint


class RelativeLinks(unittest.TestCase):
    def test_encoded_unicode_and_space_names_are_resolved(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "Relatório completo.md").write_text("audit", encoding="utf-8")
            for name in ("README.md", "SPEC-OWNERSHIP.md"):
                document = root / name
                document.write_text(
                    "[Audit](Relat%C3%B3rio%20completo.md#status)", encoding="utf-8"
                )
                report = spec_lint.Report()
                with patch.object(spec_lint, "ROOT", root):
                    spec_lint.lint_file(document, {}, report)
                self.assertEqual(report.errors, [])

    def test_missing_encoded_target_is_still_an_error(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            document = root / "README.md"
            document.write_text("[Missing](missing%20report.md)", encoding="utf-8")
            report = spec_lint.Report()
            with patch.object(spec_lint, "ROOT", root):
                spec_lint.lint_file(document, {}, report)
            self.assertEqual(len(report.errors), 1)
            self.assertIn("dangling link", report.errors[0])


if __name__ == "__main__":
    unittest.main()
