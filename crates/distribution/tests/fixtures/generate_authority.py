# SPDX-License-Identifier: MPL-2.0
"""Public deterministic test-only Ed25519 seeds; never use these for shipping."""
import base64
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile

OPENSSL = shutil.which('openssl') or r'C:\Program Files\Git\usr\bin\openssl.exe'
ROOT = Path(__file__).parent / 'authority'

def generate():
    ROOT.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='bareline-public-authority-vectors-') as scratch:
        task = Path(scratch)
        def material(number):
            path = task / f'public-test-seed-{number}.der'
            path.write_bytes(bytes.fromhex('302e020100300506032b657004220420') + bytes([number])*32)
            public = subprocess.check_output([OPENSSL, 'pkey', '-inform', 'DER', '-in', str(path), '-pubout', '-outform', 'DER'])[-32:]
            return path, base64.b64encode(b'Ed' + bytes([number])*8 + public).decode()
        keys = {n: material(n) for n in range(21, 27)}
        def sign(data, number):
            def ed(data):
                (task/'message').write_bytes(data)
                subprocess.run([OPENSSL, 'pkeyutl', '-sign', '-rawin', '-inkey', str(keys[number][0]), '-keyform', 'DER', '-in', str(task/'message'), '-out', str(task/'signature')], check=True)
                return (task/'signature').read_bytes()
            sig = ed(hashlib.blake2b(data).digest())
            comment = b'PUBLIC TEST ONLY'
            return ('untrusted comment: public deterministic fixture\n' + base64.b64encode(b'ED'+bytes([number])*8+sig).decode() + '\ntrusted comment: ' + comment.decode() + '\n' + base64.b64encode(ed(sig+comment)).decode() + '\n')
        def encoded(value):
            return json.dumps(value, sort_keys=True, separators=(',', ':')).encode()
        def write(name, value, signer):
            data = encoded(value)
            (ROOT/(name+'.json')).write_bytes(data)
            (ROOT/(name+'.minisig')).write_text(sign(data, signer), encoding='ascii')
        for name, number in [('root',21),('next-root',22),('release',23),('catalog',24),('next-release',25),('next-catalog',26)]:
            (ROOT/(name+'.txt')).write_text(keys[number][1],encoding='ascii')
        base = dict(schema_version=1,root_version=1,expires_unix=4102444800,minimum_metadata_version=3,
                    release_public_key=keys[23][1],catalog_public_key=keys[24][1],
                    publisher_certificate_sha256='07'*32,revoked_release_keys=[],revoked_publishers=[])
        write('initial',base,21)
        rotated = base | dict(root_version=2,minimum_metadata_version=5,release_public_key=keys[25][1],
                             catalog_public_key=keys[26][1],publisher_certificate_sha256='09'*32,revoked_release_keys=[keys[23][1],keys[24][1]])
        write('rotated',rotated,22)
        write('revoked',base | dict(revoked_release_keys=[keys[23][1]]),21)
        write('expired',base | dict(expires_unix=99),21)
        write('zero-version',base | dict(root_version=0),21)
        root_bytes=base64.b64decode(keys[21][1]); alias=base64.b64encode(root_bytes[:2]+b'ALIASED!'+root_bytes[10:]).decode()
        write('aliased-root',base | dict(release_public_key=alias),21)
        release_bytes=base64.b64decode(keys[23][1]); alias=base64.b64encode(release_bytes[:2]+b'ALIASED!'+release_bytes[10:]).decode()
        write('aliased-revoked',base | dict(revoked_release_keys=[alias]),21)
        transition = encoded(dict(schema_version=1,version=2,old_root_key=keys[21][1],new_root_key=keys[22][1],expires_unix=4102444800))
        chain=[dict(payload=transition.decode(),old_signature=sign(transition,21),new_signature=sign(transition,22))]
        (ROOT/'transitions.json').write_bytes(encoded(chain))
        chain[0]['new_signature']=chain[0]['old_signature']
        (ROOT/'bad-transitions.json').write_bytes(encoded(chain))
        (ROOT/'config.json').write_bytes(encoded({'trust':{'offline_root_public_key':keys[21][1],'minimum_root_version':1}}))

if __name__ == '__main__':
    generate()
