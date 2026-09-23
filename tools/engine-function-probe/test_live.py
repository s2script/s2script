#!/usr/bin/env python3
"""Judge tests assert rejection of missing/mixed live evidence, not fake runtime success."""
import importlib.util
from pathlib import Path
import unittest
import copy
spec = importlib.util.spec_from_file_location('live', Path(__file__).with_name('live.py'))
live = importlib.util.module_from_spec(spec)
spec.loader.exec_module(live)

def complete_records():
    identity = {'source':'a'*40,'token':'b'*64,'run':'run'}
    records=[]
    for generation in (1,2):
        base=dict(identity,generation=generation)
        native=dict(base,kind='engine-function-observation',resident='same',pid=1,result='pass')
        for case in live.CONTROLLED:
            records.append(dict(native,case=case,provenance='controlled-native-abi'))
        invocations=[{'id':i,'operation':1,'slot':3,'pawn':8,'method':0,'defIndex':47,'result':0,'skipped':False} for i in (1,2)]
        records.append(dict(native,case='real-acquire',provenance='real-engine-acquire',facts={
            'outer_pre':1,'deliberate_nested_pre':1,'non_skipped_completions':2,'peer_pre':2,'peer_post':2,
            'direct_body_counter':False,'validation_receipt':'verified','nested_result':0,'outer_result':0,'invocations':invocations}))
        for event in ('arm','unloaded','acquire-stimulus'):
            records.append(dict(base,kind='engine-function-script',event=event,facts={'itemCreated':True,'operation':1,'botSlot':3,'pawn':8}))
        records.append(dict(base,kind='engine-function-witness',event='armed',witnessGeneration=1,facts={}))
        for invocation in invocations:
            for event in ('acquire-pre','acquire-post'):
                records.append(dict(base,kind='engine-function-witness',event=event,witnessGeneration=1,facts=dict(invocation)))
    records.append(dict(identity,generation=2,kind='engine-function-witness',event='unloaded',witnessGeneration=1,facts={}))
    return records

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
    def test_complete_correlated_witnesses_are_accepted(self):
        self.assertEqual(live.judge(complete_records(),'a'*40,'b'*64,'run')['result'],'pass')
    def test_missing_public_pre_or_post_cannot_pass(self):
        for absent in ('acquire-pre','acquire-post'):
            with self.subTest(absent=absent):
                records=[r for r in complete_records() if r.get('event')!=absent]
                self.assertNotEqual(live.judge(records,'a'*40,'b'*64,'run')['result'],'pass')
    def test_wrong_public_generation_invocation_or_result_cannot_pass(self):
        for field,value in (('generation',99),('id',99),('operation',99),('result',7),('skipped',True)):
            with self.subTest(field=field):
                records=copy.deepcopy(complete_records())
                row=next(r for r in records if r.get('event')=='acquire-post')
                (row if field=='generation' else row['facts'])[field]=value
                self.assertNotEqual(live.judge(records,'a'*40,'b'*64,'run')['result'],'pass')
    def test_witness_generation_must_remain_resident(self):
        records=complete_records()
        for r in records:
            if r.get('kind')=='engine-function-witness' and r.get('generation')==2: r['witnessGeneration']=2
        self.assertNotEqual(live.judge(records,'a'*40,'b'*64,'run')['result'],'pass')
if __name__ == '__main__': unittest.main()
