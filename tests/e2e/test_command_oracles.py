# SPDX-License-Identifier: MPL-2.0
"""Independent codec and input-validation checks for shared utility vectors."""
import base64
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from urllib.parse import quote_from_bytes, unquote_to_bytes

import command_oracles


class CommandOracleTests(unittest.TestCase):
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


if __name__ == "__main__":
    unittest.main()
