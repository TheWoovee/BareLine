# SPDX-License-Identifier: MPL-2.0
"""Synthetic closure records exercise the validator, never release approval."""
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

import release_closure as closure
import typed_producers as typed

class ClosureTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory(prefix='closure-test-',dir=typed.ROOT/'target')
        self.addCleanup(self.temp.cleanup);self.root=Path(self.temp.name)
        self.source={'available':True,'head':'a'*40,'working_tree_dirty':False,'source_manifest_sha256':'b'*64}
        self.expected={'source_identity':self.source,'inventory_sha256':'c'*64,'inventory_binary_sha256':'d'*64,
            'index_sha256':'e'*64,'environment_matrix_sha256':'f'*64,'required_count':1,'resolved':{'AC-003-01':'PASS'},
            'cell_coverage':[{'id':'AC-003-01','cell_id':'synthetic','status':'PASS'}]}
        self.prior=self.write('prior.json',{'schema_version':1,'stage':'prerequisites','evidence_complete':True,'unresolved':[],
            'closure_ids_deferred':sorted(closure.CLOSURE_IDS),**self.expected})
        stdout=self.root/'stdout';stdout.write_text(json.dumps({'coverage_report':str(self.prior),'sha256':typed.digest(self.prior)}))
        stderr=self.root/'stderr';stderr.write_text('')
        self.receipt=self.write('receipt.json',{'schema_version':1,'status':'completed','exit_code':0,'top_level_total':1,
            'top_level_passed':1,'top_level_failed':0,'summary_lines_are_not_aggregate_counts':True,'source_changed_during_run':False,
            'source_before':self.source,'source_after':self.source,
            'command':[sys.executable,str(typed.ROOT/'tests/e2e/runner.py'),'resolve','--stage','prerequisites','--output',str(self.prior)],
            'stdout':self.ref(stdout),'stderr':self.ref(stderr)})
        performance=self.write('performance.json',{'synthetic':'performance validation is independently covered in typed producer tests'})
        self.readiness=self.write('readiness.json',{'schema_version':1,'kind':'release_readiness_record',
            'prerequisite_report_sha256':typed.digest(self.prior),'parity_index_sha256':'e'*64,
            'correctness_issues':[],'performance_evidence':[self.ref(performance)],'limitations':['Synthetic validator test only'],
            'parity_claims':[{'claim':'Synthetic limited claim','evidence_ids':['AC-003-01']}],'release_approved':False})
        self.document={'schema_version':1,'kind':'release_acceptance_closure',
            'review':{'implementer':'synthetic implementer','reviewer':'synthetic reviewer','reviewed_at_utc':'2026-09-16T00:00:00Z'},
            'prerequisite_report':self.ref(self.prior),'prerequisite_receipt':self.ref(self.receipt),'readiness_record':self.ref(self.readiness)}
        self.path=self.write('closure.json',self.document)

    def write(self,name,data):
        path=self.root/name;path.write_text(json.dumps(data),encoding='utf-8');return path
    def ref(self,path):return {'path':str(path),'sha256':typed.digest(path)}

    def test_exact_prerequisite_review_closes_only_the_two_meta_cases(self):
        with patch.object(typed,'checked_result',return_value={'producer_kind':'performance'}):
            self.assertEqual(closure.validate(self.path,self.expected),{key:'PASS' for key in closure.CLOSURE_IDS})
        changed=self.expected|{'source_identity':self.source|{'source_manifest_sha256':'0'*64}}
        with self.assertRaisesRegex(ValueError,'identity differs'):
            closure.validate(self.path,changed)
        changed=self.expected|{'resolved':{'AC-003-01':'FAIL'}}
        with self.assertRaisesRegex(ValueError,'identity differs'):
            closure.validate(self.path,changed)

    def test_self_review_and_unresolved_critical_issue_block_closure(self):
        self.document['review']['reviewer']='SYNTHETIC IMPLEMENTER'
        self.write('closure.json',self.document)
        with self.assertRaisesRegex(ValueError,'(?i)different|independent'):
            closure.validate(self.path,self.expected)
        self.document['review']['reviewer']='synthetic reviewer'
        readiness=json.loads(self.readiness.read_text());readiness['correctness_issues']=[{
            'id':'DATA-1','severity':'P1','status':'open','description':'Synthetic data loss','disposition':'not fixed'}]
        self.write('readiness.json',readiness)
        self.document['readiness_record']=self.ref(self.readiness);self.write('closure.json',self.document)
        with self.assertRaisesRegex(ValueError,'unresolved P0/P1'):
            closure.validate(self.path,self.expected)

    def test_final_report_cannot_be_its_own_prerequisite(self):
        prior=json.loads(self.prior.read_text());prior['stage']='final';self.write('prior.json',prior)
        self.document['prerequisite_report']=self.ref(self.prior);self.write('closure.json',self.document)
        with self.assertRaisesRegex(ValueError,'nonrecursive prerequisite'):
            closure.validate(self.path,self.expected)

if __name__ == '__main__':unittest.main()
