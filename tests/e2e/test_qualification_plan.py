# SPDX-License-Identifier: MPL-2.0
import json
import base64
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from urllib.parse import quote_from_bytes, unquote_to_bytes
import command_oracles
import qualification_plan as plan


class PlanTests(unittest.TestCase):
    def test_historical_inventory_has_complete_source_bound_draft_procedures(self):
        coverage=json.loads((plan.ROOT/'docs/implementation/production-readiness/COMMAND_COVERAGE.json').read_text(encoding='utf-8-sig'))
        commands=sorted({row['command'] for row in coverage['outcomes']})
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'inventory.json'
            path.write_text(json.dumps(dict(schema_version=2,commands=commands)))
            result=plan.definitions(path,True)
        self.assertEqual(result['authoring_summary']['bound_commands'],499)
        self.assertEqual(result['authoring_summary']['unbound_commands'],[])
        self.assertEqual(len(result['command_outcomes']),1497)
        for row in result['command_outcomes']:
            binding=row['oracle_binding']
            self.assertEqual((binding['command'],binding['outcome']),(row['command'],row['outcome']))
            self.assertFalse(row['complete_case_mapping'])
            self.assertEqual(row['status'],'NOT_RUN')
            for source in binding.get('authority',[]):
                self.assertEqual(plan.sha(plan.ROOT/source['path']),source['sha256'])
        self.assertEqual(len({r['procedure']['fixture_and_actions'] for r in result['acceptance']}),81)
        for row in result['acceptance']+result['minimum_scenarios']:
            self.assertEqual(row['procedure']['review'],'pending')
            self.assertFalse(row['procedure']['complete_case_mapping'])
            self.assertTrue(row['procedure']['authority'])
        for row in result['minimum_scenarios']:
            self.assertEqual(row['procedure']['required_scenario'],row['criterion'])
            self.assertEqual(len(row['procedure']['fixture_variants']),3)

    def test_full_authority_and_each_command_outcome_remain_unqualified(self):
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'inventory.json';path.write_text(json.dumps(dict(schema_version=2,commands=['file.open','custom.fixture'])))
            result=plan.definitions(path)
            self.assertEqual((len(result['acceptance']),len(result['minimum_scenarios']),len(result['command_outcomes'])),(81,199,6))
            self.assertTrue(result['planning_only']);self.assertFalse(result['release_approved'])
            self.assertTrue(all(r['status']=='NOT_RUN' and not r['complete_case_mapping'] for r in result['acceptance']+result['command_outcomes']))
            path.write_text(json.dumps(dict(schema_version=2,commands=['file.open','file.open'])))
            with self.assertRaises(ValueError):plan.definitions(path)

    def test_conversion_fixtures_match_independent_codecs(self):
        document,_=command_oracles.catalog()
        transforms={
            'utilities.base64Encode':lambda value:base64.b64encode(value),
            'utilities.base64Decode':lambda value:base64.b64decode(value,validate=True),
            'utilities.urlEncode':lambda value:quote_from_bytes(value,safe='-._~').encode(),
            'utilities.urlDecode':unquote_to_bytes,
        }
        for row in document['commands']:
            for case in row['success']:
                with self.subTest(command=row['command'],fixture=case['id']):
                    before=case['before'].encode();start,end=case['range']
                    actual=before[:start]+transforms[row['command']](before[start:end])+before[end:]
                    self.assertEqual(actual,case['after'].encode())
                    self.assertNotEqual(actual,before)

    def test_concrete_outcomes_remain_unqualified_and_unknown_stays_unbound(self):
        document,digest=command_oracles.catalog()
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'inventory.json'
            commands=[row['command'] for row in document['commands']]+['custom.fixture']
            path.write_text(json.dumps(dict(schema_version=2,commands=commands)))
            result=plan.definitions(path)
        bound=[row for row in result['command_outcomes'] if isinstance(row['oracle_binding'],dict)]
        self.assertEqual(len(bound),12)
        for row in bound:
            self.assertEqual(row['oracle_binding']['catalogue_sha256'],digest)
            self.assertGreaterEqual(len(row['action']),4)
            self.assertEqual(row['status'],'NOT_RUN')
            self.assertEqual(row['review'],'pending')
            self.assertFalse(row['complete_case_mapping'])
        self.assertEqual(len([row for row in result['command_outcomes'] if isinstance(row['oracle_binding'],str)]),3)

    def test_duplicate_and_split_utf8_fixture_ranges_are_rejected(self):
        raw=json.loads(command_oracles.CATALOG.read_text(encoding='utf-8'))
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'catalogue.json'
            with patch.object(command_oracles,'CATALOG',path):
                raw['commands'].append(raw['commands'][0])
                path.write_text(json.dumps(raw),encoding='utf-8')
                with self.assertRaises(ValueError):command_oracles.catalog()
                raw['commands'].pop()
                raw['commands'][0]['success'][1]['range'][0]=1
                path.write_text(json.dumps(raw),encoding='utf-8')
                with self.assertRaises(UnicodeDecodeError):command_oracles.catalog()


if __name__=='__main__':unittest.main()
