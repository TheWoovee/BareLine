# SPDX-License-Identifier: MPL-2.0
"""Synthetic mapping/producer tests; these are not product acceptance results."""
import copy
import json
from pathlib import Path
import platform
import subprocess
import sys
import tempfile
import unittest

import qualification_cells as cells
import typed_producers as typed

class TypedProducerTests(unittest.TestCase):
    def test_verbose_docstring_test_output_keeps_exact_selector(self):
        raw = b'test_example (suite.Case.test_example)\nA descriptive test ... ok\n\nRan 1 test in 0.001s\n\nOK\n'
        result = typed.test_observations('python_tests', raw, 0, [dict(id='check', selector='suite.Case.test_example')])
        self.assertEqual(result[0]['status'], 'PASS')

    def setUp(self):
        (typed.ROOT/'target').mkdir(exist_ok=True)
        self.temporary = tempfile.TemporaryDirectory(prefix='typed-tests-', dir=typed.ROOT/'target')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def environment(self):
        return {'os_family':'windows' if sys.platform == 'win32' else 'macos' if sys.platform == 'darwin' else 'linux',
            'os_build':platform.version(),'architecture':'arm64' if platform.machine().lower() in {'arm64','aarch64'} else 'x64',
            'hardware':'synthetic test on current host','mode':'headless','theme':'not_applicable','dpi':'not_applicable',
            'renderer':'not_applicable','assistive_technology':'none','build_mode':'preview'}

    def plan(self):
        name = 'test_signing_may_only_change_certificate_fields_and_terminal_padding'
        return {'schema_version':1,'id':'synthetic.pipeline.boundary','producer_kind':'python_tests',
            'command':[sys.executable,'-m','unittest','discover','-s','scripts','-p','test_release_pipeline.py','-k',name,'-v'],
            'checks':[{'id':'signing-boundary','selector':'test_release_pipeline.PipelineTests.'+name}],
            'artifacts':[{'role':'tested-source','kind':'source','path':'scripts/release_pipeline.py'}],
            'environment':self.environment(),'timeout_seconds':90}

    def write(self, name, value):
        path = self.root/name
        path.write_text(json.dumps(value), encoding='utf-8')
        return path

    def test_failed_process_zero_tests_skips_and_ambiguous_output_never_pass(self):
        checks = [{'id':'one','selector':'module::one'}]
        raw = b'running 1 test\ntest module::one ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured;\n'
        self.assertEqual(typed.test_observations('rust_tests',raw,7,checks)[0]['status'],'FAIL')
        empty = b'running 0 tests\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured;\n'
        self.assertEqual(typed.test_observations('rust_tests',empty,0,checks)[0]['status'],'NOT_RUN')
        skipped = b'test module::one ... ignored\ntest result: ok. 0 passed; 0 failed; 1 ignored;\n'
        self.assertEqual(typed.test_observations('rust_tests',skipped,0,checks)[0]['status'],'NOT_RUN')
        with self.assertRaisesRegex(ValueError,'ambiguous'):
            typed.test_observations('rust_tests',raw+raw,0,checks)
        with self.assertRaisesRegex(ValueError,'inconsistent'):
            typed.test_observations('rust_tests',b'test module::one ... ok\n',0,checks)

    def test_real_nested_capture_adaptation_and_tampered_environment(self):
        plan = self.write('plan.json',self.plan()); result = self.root/'result.json'; receipt = self.root/'outer.json'
        command = [sys.executable,str(typed.ROOT/'.github/workflows/run_test_evidence.py'),'--output',str(receipt),'--timeout','120','--',
            sys.executable,str(Path(typed.__file__).resolve()),'execute','--plan',str(plan),'--output',str(result)]
        captured = subprocess.run(command,cwd=typed.ROOT,capture_output=True,text=True,timeout=140)
        self.assertEqual(captured.returncode,0,captured.stderr+'\n'+(receipt.with_suffix('.json.stderr.log').read_text() if receipt.exists() else ''))
        actual = typed.checked_result(result)
        self.assertEqual(actual['status'],'PASS')
        mapping = self.write('mappings.json',{'schema_version':1,'kind':'typed_qualification_mappings','mappings':[{
            'id':'AC-003-01','procedure_id':self.plan()['id'],'producer_kind':'python_tests','required_checks':['signing-boundary'],
            'scope':'SYNTHETIC MAPPING for validator test only; not AC acceptance','reviewed':True,
            'implementer':'synthetic implementer','reviewer':'synthetic reviewer'}]})
        output = self.root/'adapted.json'
        typed.adapt(result,receipt,mapping,'AC-003-01','synthetic-cell',output)
        record = json.loads(output.read_text())['evidence'][0]
        typed.validate_record(record)
        changed = copy.deepcopy(record); changed['os_build'] = 'different'
        with self.assertRaisesRegex(ValueError,'environment'):
            typed.validate_record(changed)
        changed = copy.deepcopy(record); changed['result_sha256'] = '0'*64
        with self.assertRaisesRegex(ValueError,'hash changed'):
            typed.validate_record(changed)
        matrix = {'schema_version':2,'kind':'qualification_environment_matrix','review':{'implementer':'a','reviewer':'b'},
            'cells':[{'id':'synthetic-cell','environment':actual['environment'],'source_identity':actual['source_identity'],
                'artifact_set_sha256':actual['artifact_set_sha256'],'producer_kind':'python_tests'}],
            'requirements':{'AC-003-01':['synthetic-cell']},'exclusions':[]}
        cells.matrix(matrix,{'AC-003-01'}); cells.require_match(record,matrix['cells'][0])
        matrix['cells'][0]['producer_kind'] = 'manual'
        with self.assertRaisesRegex(ValueError,'artifacts/producer'):
            cells.require_match(record,matrix['cells'][0])
        import evidence_bundle
        retained=self.root/'retained'
        exported=evidence_bundle.collect({'schema_version':1,'kind':'evidence_collection_plan','label':'SYNTHETIC typed regression only',
            'files':[{'path':str(output),'kind':'evidence_bundle'}]},retained,typed.ROOT)
        # Every generated producer/result/receipt/mapping input is gone. The
        # verifier must read retained objects rather than the old cache paths.
        for path in self.root.iterdir():
            if path.is_file(): path.unlink()
        relocated=self.root/'relocated retained';retained.rename(relocated)
        rechecked=evidence_bundle.verify_producers(relocated,exported['manifest_sha256'])
        self.assertEqual(rechecked['typed_result_count'],1)
        self.assertEqual(rechecked['typed_evidence_count'],1)
        self.assertFalse(rechecked['semantic_acceptance'])

    def test_actual_host_cannot_be_relabeled_as_a_foreign_vm(self):
        environment = self.environment(); typed.actual_environment(environment)
        environment['os_family'] = 'linux' if environment['os_family'] != 'linux' else 'windows'
        with self.assertRaisesRegex(ValueError,'actual execution host'):
            typed.actual_environment(environment)

    def test_manual_observation_is_a_named_artifact_bound_declaration(self):
        proof = self.root/'synthetic.txt'; proof.write_text('synthetic observation artifact')
        attestation = self.write('manual.json',{'schema_version':1,'kind':'manual_observation',
            'observer':'synthetic observer','observed_at_utc':'2026-09-16T00:00:00Z','environment':self.environment(),
            'source_identity':{'available':True,'head':'a'*40,'working_tree_dirty':False,'source_manifest_sha256':'b'*64},
            'observations':[{'id':'one','status':'PASS','observed':'synthetic only','artifact_roles':['proof']}]})
        artifacts = [{'role':'manual_attestation','kind':'report','path':str(attestation)}, {'role':'proof','kind':'fixture','path':str(proof)}]
        plan = {'environment':self.environment(),'checks':[{'id':'check','selector':'one'}]}
        observations = typed.manual_observations(plan,artifacts)
        self.assertIn('Manual declaration by synthetic observer',observations[0]['observed'])
        data = json.loads(attestation.read_text()); data['observations'][0]['artifact_roles']=[];attestation.write_text(json.dumps(data))
        with self.assertRaisesRegex(ValueError,'retained artifacts'):
            typed.manual_observations(plan,artifacts)

    def test_artifact_identity_is_typed_and_path_independent_but_not_size_ambiguous(self):
        item={'role':'core','kind':'source','path':'somewhere','sha256':'a'*64,'bytes':1}
        self.assertEqual(typed.artifact_set([item]),typed.artifact_set([item|{'path':'relocated'}]))
        self.assertNotEqual(typed.artifact_set([item]),typed.artifact_set([item|{'kind':'windows_executable'}]))
        with self.assertRaisesRegex(ValueError,'identity'):
            typed.artifact_set([item|{'bytes':True}])

    def test_performance_percentiles_are_recomputed_from_captured_measurements(self):
        sys.path.insert(0,str(typed.ROOT/'tests/perf'))
        import perf_suite
        from test_perf_suite import qualification_environment
        font=self.root/'font';font.write_bytes(b'synthetic font')
        executable=self.root/'synthetic.exe';executable.write_bytes(b'nonexecuted fixture')
        qualification=qualification_environment(font)
        source={'available':True,'head':'a'*40,'working_tree_dirty':False,'source_manifest_sha256':'b'*64}
        manifest={'applications':{'bareline':{'sha256':typed.digest(executable)}},'repetitions':1,
            'scenarios':[{'name':'save','comparable_metrics':[]}],'qualification_environment':qualification}
        self.write('provenance.json',{'manifest':manifest,'source_before':source,'source_after':source,'source_changed_during_run':False})
        self.write('source-after.json',{'source_after':source,'source_changed_during_run':False})
        raw={'scenario':'save','pair':0,'position':0,'application':'bareline','metrics':{'latency_us':10},'status':'ok','exit_code':0,
            'error':None,'stdout':json.dumps({'event':'measurement','metrics':{'latency_us':10}})}
        self.write('save-0-bareline.json',raw)
        report=self.root/'report.json';perf_suite.report(self.root,report)
        artifacts=[]
        for role,path,kind in [('performance_report',report,'report'),('performance_provenance',self.root/'provenance.json','report'),
            ('source_after',self.root/'source-after.json','report'),('trial',self.root/'save-0-bareline.json','report'),('editor',executable,'windows_executable')]:
            artifacts.append({'role':role,'kind':kind,'path':str(path),'sha256':typed.digest(path),'bytes':path.stat().st_size})
        environment=self.environment();environment['hardware']=typed.canonical(qualification['hardware']).decode()
        plan={'environment':environment,'checks':[{'id':'latency','selector':'save/bareline/latency_us'}]}
        result=typed.performance_observations(plan,artifacts)
        self.assertEqual(result[0]['status'],'PASS');self.assertIn('timing is informational',result[0]['observed'])
        changed=json.loads(report.read_text());changed['observations'][0]['p50']=1;report.write_text(json.dumps(changed))
        with self.assertRaisesRegex(ValueError,'aggregate differs'):
            typed.performance_observations(plan,artifacts)

if __name__ == '__main__':
    unittest.main()
