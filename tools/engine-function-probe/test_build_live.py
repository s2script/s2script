#!/usr/bin/env python3
"""Exercise build-mode control flow; stubs are not native compile evidence."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).with_name('build-live.sh')

class BuildModeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        script = self.root/'tools/engine-function-probe/build-live.sh'
        script.parent.mkdir(parents=True)
        shutil.copyfile(SCRIPT, script)
        self.script = script
        self.bin = self.root/'bin'
        self.bin.mkdir()
        self.env = dict(os.environ, PATH=str(self.bin)+':'+os.environ['PATH'],
                        S2FN_SOURCE_REVISION='a'*40, S2FN_BUILD_TOKEN='b'*64)
        self.stub('git', "printf '%s\\n' '"+'a'*40+"'")
        self.stub('uname', 'if [ "$1" = -s ]; then echo Linux; else echo x86_64; fi')
        self.stub('cmake', 'exit 0')
        self.stub('ldd', 'echo resolved')
        self.stub('nm', 'exit 0')
        self.stub('objdump', 'echo GLIBC_2.39')

    def stub(self, name, body):
        path = self.bin/name
        path.write_text('#!/bin/sh\n'+body+'\n')
        path.chmod(0o755)

    def run_build(self, *args):
        return subprocess.run(['bash',str(self.script),*args],env=self.env,text=True,
                              stdout=subprocess.PIPE,stderr=subprocess.STDOUT)

    def test_strict_probe_rejects_newer_libc(self):
        result = self.run_build('--probe-only')
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn('probe exceeds GLIBC_2.31', result.stdout)

    def test_explicit_host_compile_reports_but_does_not_package(self):
        result = self.run_build('--probe-only','--compile-only')
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn('GLIBC maximum: 2.39', result.stdout)
        self.assertIn('not a deployable bundle', result.stdout)
        self.assertFalse(list(self.root.rglob('engine-function-build.json')))

    def test_compile_flag_cannot_enter_bundle_or_unknown_path(self):
        for args in [('--compile-only',),('--compile-only','--probe-only'),
                     ('--probe-only --compile-only',),('--probe-only','--unknown')]:
            with self.subTest(args=args):
                result = self.run_build(*args)
                self.assertEqual(result.returncode, 2, result.stdout)
                self.assertIn('usage:', result.stdout)

    def test_compile_only_keeps_dependency_and_export_guards(self):
        self.stub('ldd', 'echo "libffi.so.8 => /usr/lib/libffi.so.8"')
        self.assertNotEqual(self.run_build('--probe-only','--compile-only').returncode, 0)
        self.stub('ldd', 'echo resolved')
        self.stub('nm', 'echo "0000 T ffi_call"')
        self.assertNotEqual(self.run_build('--probe-only','--compile-only').returncode, 0)

if __name__ == '__main__':
    unittest.main()
