#!/usr/bin/env python3
"""Judge tests assert rejection of missing/mixed live evidence, not fake runtime success."""
import importlib.util
from pathlib import Path
import unittest
spec = importlib.util.spec_from_file_location('live', Path(__file__).with_name('live.py'))
live = importlib.util.module_from_spec(spec)
spec.loader.exec_module(live)
class JudgeTests(unittest.TestCase):
    def test_no_records_cannot_pass(self):
        self.assertNotEqual(live.judge([], 'a'*40, 'b'*64, 'run')['result'], 'pass')
    def test_mixed_source_is_failure(self):
        r = {'kind':'engine-function-observation','source':'c'*40,'token':'b'*64,'run':'run','result':'pass'}
        self.assertEqual(live.judge([r], 'a'*40, 'b'*64, 'run')['result'], 'fail')
    def test_failure_cannot_be_erased_by_later_pass(self):
        base = {'kind':'engine-function-observation','source':'a'*40,'token':'b'*64,'run':'run','case':'acquire-compatibility','generation':1,'resident':'same','pid':1}
        records = [dict(base,result='fail'),dict(base,result='pass')]
        self.assertEqual(live.judge(records, 'a'*40, 'b'*64, 'run')['result'], 'fail')
    def test_controlled_is_not_real_engine(self):
        r = {'kind':'engine-function-observation','source':'a'*40,'token':'b'*64,'run':'run','result':'pass','case':'real-acquire','generation':1,'resident':'same','pid':1,'provenance':'controlled-native-abi'}
        self.assertNotEqual(live.judge([r], 'a'*40, 'b'*64, 'run')['result'], 'pass')
if __name__ == '__main__': unittest.main()
