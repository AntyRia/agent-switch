"""Offline installer checks, including the documented pipe-to-sh invocation.

Run with: python3 -m unittest discover -s tests -p 'test_install.py'
"""

import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
import zipfile


INSTALLER = Path(__file__).resolve().parents[1] / "scripts" / "install.sh"
ASSET = "agent-switch-0.2.1-aarch64-apple-darwin.zip"


class InstallTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="agent-switch-install-test-")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.bin = self.root / "tools"
        self.bin.mkdir()
        self.destination = self.root / "installed"
        self.env = dict(os.environ, PATH=f"{self.bin}:/usr/bin:/bin",
                        AGENT_SWITCH_INSTALL_DIR=str(self.destination))
        self.metadata = self.root / "release.json"
        self.metadata.write_text(json.dumps({
            "tag_name": "v0.2.1",
            "assets": [
                {"browser_download_url": "https://example.invalid/other.zip"},
                {"browser_download_url": f"https://example.invalid/{ASSET}"},
            ],
        }, indent=2))
        self.releases = self.root / "releases.json"
        self.releases.write_text(json.dumps([json.loads(self.metadata.read_text())], indent=2))
        (self.root / "v0.2.1.json").write_text(self.metadata.read_text())
        with zipfile.ZipFile(self.root / "asset.zip", "w") as archive:
            archive.writestr("agent-switch", '#!/bin/sh\nprintf "agent-switch 0.2.1\\n"\n')
        self.tool("uname", 'case "$1" in -s) echo Darwin ;; -m) echo arm64 ;; esac\n')
        self.tool("curl", '''
fixture_dir=$(dirname "$0")/..
output=
url=
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o) output=$2; shift 2 ;;
    *) url=$1; shift ;;
  esac
done
if [ -n "$output" ]; then
  [ ! -f "$fixture_dir/fail-download" ] || exit 22
  cp "$fixture_dir/asset.zip" "$output"
else
  case "$url" in
    */releases/latest) cat "$fixture_dir/release.json" ;;
    */releases/tags/*) cat "$fixture_dir/${url##*/}.json" ;;
    */releases\?*page=1) cat "$fixture_dir/releases.json" ;;
    */releases\?*) echo '[]' ;;
    *) exit 22 ;;
  esac
fi
''')

    def tool(self, name, body):
        target = self.bin / name
        target.write_text("#!/bin/sh\nset -eu\n" + body)
        target.chmod(0o755)

    def run_installer(self):
        # stdin is a pipe, as in `curl ... | sh`, not a script filename.
        return subprocess.run(["/bin/sh"], input=INSTALLER.read_text(),
                              env=self.env, capture_output=True, text=True, timeout=15)

    def test_pipe_install_on_apple_silicon(self):
        result = self.run_installer()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        installed = self.destination / "agent-switch"
        self.assertTrue(os.access(installed, os.X_OK))
        version = subprocess.check_output([str(installed), "--version"], text=True)
        self.assertEqual(version.strip(), "agent-switch 0.2.1")
        self.assertIn(ASSET, result.stdout)

    def test_missing_platform_has_actionable_error(self):
        self.metadata.write_text(json.dumps({"tag_name": "v0.2.1", "assets": []}, indent=2))
        self.releases.write_text("[]")
        result = self.run_installer()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("No published package", result.stderr)
        self.assertNotIn("syntax error", result.stderr)
        self.assertFalse((self.destination / "agent-switch").exists())

    def test_newer_release_for_other_platform_does_not_hide_mac_package(self):
        self.metadata.write_text(json.dumps({
            "tag_name": "v0.2.2", "assets": [
                {"browser_download_url": "https://example.invalid/agent-switch-0.2.2-x86_64-pc-windows-msvc.zip"},
            ],
        }, indent=2))
        result = self.run_installer()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(ASSET, result.stdout)

    def test_prerelease_package_is_not_installed(self):
        candidate = json.loads(self.metadata.read_text())
        candidate["prerelease"] = True
        self.releases.write_text(json.dumps([candidate], indent=2))
        (self.root / "v0.2.1.json").write_text(json.dumps(candidate, indent=2))
        self.metadata.write_text(json.dumps({"tag_name": "v0.2.2", "assets": []}, indent=2))
        result = self.run_installer()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.destination / "agent-switch").exists())

    def test_failed_download_preserves_existing_install(self):
        self.destination.mkdir()
        installed = self.destination / "agent-switch"
        installed.write_text("existing installation")
        (self.root / "fail-download").touch()
        result = self.run_installer()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Downloading", result.stdout)
        self.assertEqual(installed.read_text(), "existing installation")

    def test_unsupported_architecture_exits_before_install(self):
        self.tool("uname", 'case "$1" in -s) echo Darwin ;; -m) echo unsupported ;; esac\n')
        result = self.run_installer()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unsupported architecture", result.stderr)
        self.assertFalse(self.destination.exists())


if __name__ == "__main__":
    unittest.main()
