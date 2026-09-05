"""Exercise downloaded packages and launcher behavior without touching the desktop."""
import hashlib
import io
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='ramdog-install-test-')
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.commands = self.base / 'commands'
        self.commands.mkdir()
        self.dest = self.base / 'custom destination'
        self.package = self.base / 'package.tar.gz'
        with tarfile.open(self.package, 'w:gz') as archive:
            data = b'#!/bin/sh\necho launched > "$RAMDOG_TEST_MARKER"\n'
            info = tarfile.TarInfo('./ramdog')
            info.size, info.mode = len(data), 0o755
            archive.addfile(info, io.BytesIO(data))
            archive.add(ROOT / 'linux/ramdog-launch', arcname='./ramdog-launch')
        digest = hashlib.sha256(self.package.read_bytes()).hexdigest()
        self.checksums = self.base / 'SHA256SUMS.txt'
        self.checksums.write_text(f'{digest}  RamDog-linux-x86_64.tar.gz\n')
        self.env = dict(os.environ, PATH=f'{self.commands}:{os.environ["PATH"]}',
                        RAMDOG_HOME=str(self.dest), RAMDOG_VERSION='v0.9.0',
                        RAMDOG_NO_LAUNCH='1', RAMDOG_TEST_PACKAGE=str(self.package),
                        RAMDOG_TEST_CHECKSUMS=str(self.checksums),
                        RAMDOG_TEST_MARKER=str(self.base / 'launched'),
                        RAMDOG_TEST_URLS=str(self.base / 'urls'))
        self.mock('uname', 'case "$1" in -s) echo Linux;; -m) echo x86_64;; esac')
        self.mock('curl', '''
while [ "$#" -gt 0 ]; do
  case "$1" in
    https://*) url="$1";;
    -o) shift; output="$1";;
  esac
  shift
done
printf '%s\\n' "$url" >> "$RAMDOG_TEST_URLS"
case "$url" in
  */SHA256SUMS.txt) cp "$RAMDOG_TEST_CHECKSUMS" "$output";;
  *.tar.gz) cp "$RAMDOG_TEST_PACKAGE" "$output";;
  *) exit 22;;
esac
''')
        self.mock('systemctl', 'exit 1')

    def mock(self, name, body):
        path = self.commands / name
        path.write_text('#!/bin/sh\nset -eu\n' + body + '\n')
        path.chmod(0o755)

    def install(self):
        return subprocess.run(['sh', str(ROOT / 'install.sh')], env=self.env,
                              capture_output=True, text=True, timeout=10)

    def test_verified_package_custom_destination_and_no_launch(self):
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(os.access(self.dest / 'ramdog', os.X_OK))
        self.assertTrue(os.access(self.dest / 'ramdog-launch', os.X_OK))
        self.assertFalse((self.base / 'launched').exists())
        self.assertIn('/download/v0.9.0/', (self.base / 'urls').read_text())

    def test_corrupt_checksum_preserves_existing_binary(self):
        self.dest.mkdir()
        binary = self.dest / 'ramdog'
        binary.write_text('previous version')
        self.checksums.write_text('0' * 64 + '  RamDog-linux-x86_64.tar.gz\n')
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(binary.read_text(), 'previous version')
        self.assertFalse((self.base / 'launched').exists())

    def test_launcher_uses_adjacent_binary_without_user_systemd(self):
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        result = subprocess.run([str(self.dest / 'ramdog-launch')], env=self.env,
                                capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.base / 'launched').exists())

    def test_launcher_passes_custom_binary_and_arguments_to_systemd(self):
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.mock('systemctl', '''case "$*" in *is-active*) exit 3;; *) exit 0;; esac''')
        self.mock('systemd-run', '''printf '%s\\n' "$@" > "$RAMDOG_TEST_MARKER"''')
        result = subprocess.run([str(self.dest / 'ramdog-launch'), '--example', 'two words'],
                                env=self.env, capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        args = (self.base / 'launched').read_text().splitlines()
        self.assertEqual(args[-3:], [str(self.dest / 'ramdog'), '--example', 'two words'])

    def test_source_fallback_installs_launcher_without_opening(self):
        self.mock('curl', 'exit 22')
        self.mock('git', '''for arg do destination="$arg"; done
mkdir -p "$destination/linux"
cp "$RAMDOG_TEST_SOURCE_LAUNCHER" "$destination/linux/ramdog-launch"
printf '[package]\\n' > "$destination/Cargo.toml"''')
        self.mock('cargo', '''for arg do manifest="$arg"; done
mkdir -p "$(dirname "$manifest")/target/release"
printf '#!/bin/sh\\nexit 0\\n' > "$(dirname "$manifest")/target/release/ramdog"''')
        self.env['RAMDOG_TEST_SOURCE_LAUNCHER'] = str(ROOT / 'linux/ramdog-launch')
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(os.access(self.dest / 'ramdog-launch', os.X_OK))
        self.assertFalse((self.base / 'launched').exists())


if __name__ == '__main__':
    unittest.main()
