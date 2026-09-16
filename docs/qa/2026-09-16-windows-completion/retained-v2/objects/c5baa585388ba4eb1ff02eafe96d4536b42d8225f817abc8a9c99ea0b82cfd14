# SPDX-License-Identifier: MPL-2.0
"""Configured Windows release handoff. Never signs, executes payloads or publishes."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import struct
import sys
import tomllib
import zipfile

import release_config as config_api

EXES = ('bareline.exe', 'bareline-update-helper.exe', 'bareline-extension-host.exe')
COMPONENTS = ('json-tools', 'xml-tools', 'hex-view')
MAX_FILE = 1024**3

def require(condition, message):
    if not condition:
        raise ValueError(message)

def regular(path):
    path = Path(path).absolute()
    for ancestor in (path, *path.parents):
        require(not ancestor.is_symlink() and not ancestor.is_junction(), f'reparse path: {ancestor}')
    require(path.is_file() and path.stat().st_size <= MAX_FILE, f'bounded regular file required: {path}')
    return path

def record(path):
    path = regular(path)
    digest, size = config_api.sha256_file(path)
    return {'sha256': digest, 'bytes': size}

def read_json(path):
    path = regular(path)
    require(path.stat().st_size <= 16*1024*1024, 'JSON limit')
    return config_api.load_json(path)

def write_json(path, data):
    with Path(path).open('x', encoding='utf-8', newline='\n') as stream:
        json.dump(data, stream, indent=2, sort_keys=True)
        stream.write('\n')

def copy_checked(source, target):
    before = record(source)
    with regular(source).open('rb') as incoming, Path(target).open('xb') as outgoing:
        shutil.copyfileobj(incoming, outgoing, 65536)
    require(record(target) == before == record(source), 'input changed during copy')
    return before

def new_directory(path):
    path = Path(path).absolute()
    require(not path.exists(), 'use a new output directory; incomplete runs are retained')
    for ancestor in path.parents:
        require(not ancestor.is_symlink() and not ancestor.is_junction(), 'reparse output parent')
    path.mkdir(parents=True)
    return path

def checked_replica(root, config):
    root = Path(root).resolve(strict=True)
    manifest_path = root/'build-capabilities.json'
    manifest = read_json(manifest_path)
    require(manifest.get('build_mode') == 'configured', 'preview/fixture is not a shipping candidate')
    require(manifest.get('configuration_sha256') == hashlib.sha256(config_api.canonical_bytes(config)).hexdigest(), 'replica config mismatch')
    config_api.verify_manifest(manifest_path, root/'unsigned-executables')
    environment = read_json(root/'build-environment.json')
    require(environment.get('schema_version') == 1 and environment.get('kind') == 'configured_build_environment'
            and environment.get('clean_target') is True and environment.get('components') is True,
            'complete clean configured build receipt required')
    for name in ('run_id', 'host', 'os', 'compiler', 'target', 'started_utc', 'completed_utc'):
        require(isinstance(environment.get(name), str) and environment[name], f'missing build {name}')
    artifacts = {f'unsigned/{name}': root/'unsigned-executables'/name for name in EXES}
    artifacts.update({f'components/{name}.blex': root/'components'/f'{name}.blex' for name in COMPONENTS})
    # A component inventory must bind exactly the same config/source as the exes.
    inventory = read_json(root/'components/components-inventory.json')
    require(inventory.get('inventory') == 'components' and inventory.get('signed') is False, 'unsigned component inventory required')
    for key in ('configuration_sha256', 'source_sha256', 'source_revision', 'build_mode', 'features', 'distribution_version'):
        require(inventory.get(key) == manifest[key], f'component provenance mismatch: {key}')
    entries = inventory.get('artifacts')
    require(isinstance(entries, list) and len(entries) == 3, 'three first-party component records required')
    require({entry.get('role') for entry in entries} == set(COMPONENTS), 'component role mismatch')
    for entry in entries:
        require(entry.get('file') == entry['role']+'.blex', 'component filename mismatch')
        require(record(root/'components'/entry['file']) == {key: entry[key] for key in ('sha256', 'bytes')}, 'component digest mismatch')
    return root, manifest, environment, artifacts

def compare(config_path, first, second, output):
    config = config_api.validate(Path(config_path), {'configured'})
    a, am, ae, af = checked_replica(first, config)
    b, bm, be, bf = checked_replica(second, config)
    require(a != b and ae['run_id'] != be['run_id'], 'distinct build runs required')
    require(am == bm and ae['compiler'] == be['compiler'], 'replica source/config/compiler/capabilities differ')
    for name in af:
        require(not os.path.samefile(af[name], bf[name]), 'replicas share the same artifact file')
        require(record(af[name]) == record(bf[name]), f'unsigned replica mismatch: {name}')
    output = new_directory(output)
    (output/'unsigned').mkdir()
    (output/'components').mkdir()
    records = {name: copy_checked(source, output/name) for name, source in af.items()}
    for label, root in (('first', a), ('second', b)):
        for name in ('build-capabilities.json', 'build-environment.json'):
            target = f'{label}-{name}'
            records[target] = copy_checked(root/name, output/target)
        target = f'{label}-components-inventory.json'
        records[target] = copy_checked(root/'components/components-inventory.json', output/target)
    write_json(output/'public-release-config.json', config)
    records['public-release-config.json'] = record(output/'public-release-config.json')
    write_json(output/'handoff.json', {
        'schema_version': 1, 'kind': 'configured_unsigned_handoff', 'state': 'awaiting_external_signing',
        'configuration_sha256': am['configuration_sha256'], 'source_sha256': am['source_sha256'],
        'replica_bytes_match': True, 'independent_host_review': 'pending', 'release_approved': False,
        'files': records,
    })
    return output

def verify_handoff(root):
    root = Path(root).resolve(strict=True)
    handoff = read_json(root/'handoff.json')
    require(handoff.get('schema_version') == 1 and handoff.get('kind') == 'configured_unsigned_handoff'
            and handoff.get('state') == 'awaiting_external_signing' and handoff.get('release_approved') is False,
            'invalid unsigned handoff')
    files = handoff.get('files')
    expected = {f'unsigned/{name}' for name in EXES} | {f'components/{name}.blex' for name in COMPONENTS}
    expected |= {f'{which}-{name}.json' for which in ('first', 'second') for name in ('build-capabilities', 'build-environment')}
    expected |= {f'{which}-components-inventory.json' for which in ('first', 'second')}
    expected.add('public-release-config.json')
    require(isinstance(files, dict) and set(files) == expected, 'handoff file set mismatch')
    for name, identity in files.items():
        require(record(root/name) == identity, f'handoff changed: {name}')
    config = config_api.validate(root/'public-release-config.json', {'configured'})
    require(hashlib.sha256(config_api.canonical_bytes(config)).hexdigest() == handoff['configuration_sha256'], 'handoff config changed')
    first = read_json(root/'first-build-capabilities.json')
    require(first == read_json(root/'second-build-capabilities.json'), 'handoff replicas disagree')
    config_api.verify_manifest(root/'first-build-capabilities.json', root/'unsigned')
    require(first['source_sha256'] == handoff['source_sha256'], 'handoff source changed')
    environments = [read_json(root/f'{which}-build-environment.json') for which in ('first', 'second')]
    require(environments[0]['run_id'] != environments[1]['run_id']
            and environments[0]['compiler'] == environments[1]['compiler'], 'retained build runs differ or repeat')
    for which, environment in zip(('first', 'second'), environments):
        require(environment.get('clean_target') is True and environment.get('components') is True,
                'retained build did not include clean components')
        inventory = read_json(root/f'{which}-components-inventory.json')
        require(inventory.get('inventory') == 'components' and inventory.get('signed') is False, 'retained components must be unsigned')
        for key in ('configuration_sha256', 'source_sha256', 'source_revision', 'build_mode', 'features', 'distribution_version'):
            require(inventory.get(key) == first[key], 'retained component provenance differs: '+key)
        entries = inventory.get('artifacts', [])
        require(len(entries) == 3 and {entry.get('role') for entry in entries} == set(COMPONENTS), 'retained component set differs')
        for entry in entries:
            require(entry['file'] == entry['role']+'.blex'
                    and record(root/'components'/entry['file']) == {key:entry[key] for key in ('sha256','bytes')},
                    'retained component bytes differ')
    return config, handoff

def pe_offsets(data):
    require(len(data) >= 64 and data[:2] == b'MZ', 'PE image required')
    pe = struct.unpack_from('<I', data, 60)[0]
    require(pe <= len(data)-176 and data[pe:pe+4] == b'PE\0\0', 'invalid PE header')
    optional = pe+24
    require(struct.unpack_from('<H', data, optional)[0] == 0x20b, 'Windows x64 PE32+ image required')
    require(struct.unpack_from('<H', data, pe+4)[0] == 0x8664, 'x64 machine required')
    require(struct.unpack_from('<H', data, pe+20)[0] >= 152, 'truncated PE optional header')
    require(struct.unpack_from('<I', data, optional+108)[0] >= 5, 'missing security directory')
    return optional+64, optional+112+4*8

def verify_signed_bytes(unsigned, signed):
    """Allow only Authenticode checksum/directory/certificate append changes.

    Signature authenticity and the publisher pin are checked separately by Windows.
    """
    before, after = record(unsigned), record(signed)
    with regular(unsigned).open('rb') as source, regular(signed).open('rb') as candidate:
        original_header, signed_header = source.read(1024*1024), candidate.read(1024*1024)
        checksum, security = pe_offsets(original_header)
        require(pe_offsets(signed_header) == (checksum, security), 'signed PE headers changed')
        require(struct.unpack_from('<II', original_header, security) == (0, 0), 'original must be unsigned')
        start, size = struct.unpack_from('<II', signed_header, security)
        require(start == (before['bytes']+7)//8*8 and size >= 8 and start+size == after['bytes'], 'certificate append bounds invalid')
        candidate.seek(before['bytes'])
        require(candidate.read(start-before['bytes']) == b'\0'*(start-before['bytes']), 'nonzero signing padding')
        source.seek(0); candidate.seek(0)
        position = 0
        while position < before['bytes']:
            count = min(65536, before['bytes']-position)
            left, right = bytearray(source.read(count)), bytearray(candidate.read(count))
            require(len(left) == count and len(right) == count, 'truncated signing input')
            for offset, length in ((checksum, 4), (security, 8)):
                lo, hi = max(position, offset), min(position+count, offset+length)
                if lo < hi:
                    left[lo-position:hi-position] = b'\0'*(hi-lo)
                    right[lo-position:hi-position] = b'\0'*(hi-lo)
            require(left == right, 'signed executable code differs from the compared unsigned candidate')
            position += count
    require(record(unsigned) == before and record(signed) == after, 'signing input changed during verification')
    return after

def metadata(handoff_root, signed_dir, expiry, output):
    config, handoff = verify_handoff(handoff_root)
    require(type(expiry) is int and 0 < expiry < 2**63, 'explicit expiry required')
    root, signed_dir = Path(handoff_root), Path(signed_dir)
    signed = {name: verify_signed_bytes(root/'unsigned'/name, signed_dir/name) for name in EXES}
    output = new_directory(output)
    for name in EXES:
        copy_checked(signed_dir/name, output/name)
    entries = []
    for name in COMPONENTS:
        source = root/'components'/f'{name}.blex'
        identity = record(source)
        with zipfile.ZipFile(source) as archive:
            item = archive.getinfo('manifest.toml')
            require(item.file_size <= 65536, 'component manifest limit')
            manifest = tomllib.loads(archive.read(item).decode('utf-8'))
        require(manifest.get('id') == 'org.bareline.'+name and manifest.get('publisher') == config['trust']['publisher']
                and manifest.get('version') == config['distribution']['version'], 'component manifest differs from release config')
        copy_checked(source, output/(identity['sha256']+'.blex'))
        entries.append({
            'id': manifest['id'], 'version': manifest['version'], 'publisher': config['trust']['publisher'],
            'artifact_type': 'extension', 'platform': 'windows-x64', 'channel': config['distribution']['channel'],
            'length': identity['bytes'], 'sha256': identity['sha256'], 'minimum_protocol': 1, 'maximum_protocol': 1,
            'capabilities': manifest.get('capabilities', []),
        })
    updates, trust = config['updates'], config['trust']
    common = {'schema_version': 1, 'metadata_version': updates['metadata_version'],
        'version': config['distribution']['version'], 'channel': config['distribution']['channel'],
        'platform': 'windows-x64', 'publisher': trust['publisher'], 'minimum_protocol': 1, 'expires_unix': expiry}
    for name, filename, artifact in [('bareline.exe', 'bareline.update.json', config['distribution']['core_artifact_type']),
        ('bareline-extension-host.exe', 'runtime.json', config['distribution']['runtime_artifact_type'])]:
        write_json(output/filename, common | {'artifact_type': artifact, 'length': signed[name]['bytes'], 'sha256': signed[name]['sha256']})
    write_json(output/'catalog.json', {'schema_version': 1, 'metadata_version': updates['metadata_version'],
        'expires_unix': expiry, 'entries': entries})
    write_json(output/'bareline.release-authority.json', {'schema_version': 1, 'root_version': trust['minimum_root_version'],
        'expires_unix': expiry, 'minimum_metadata_version': updates['minimum_metadata_version'],
        'release_public_key': trust['release_public_key'], 'catalog_public_key': trust['catalog_public_key'],
        'publisher_certificate_sha256': trust['publisher_certificate_sha256'], 'revoked_release_keys': [], 'revoked_publishers': []})
    write_json(output/'signing-request.json', {'schema_version': 1, 'kind': 'metadata_signing_request',
        'state': 'awaiting_signature_verification', 'release_approved': False,
        'unsigned_handoff_sha256': record(root/'handoff.json')['sha256'],
        'signed_executables': signed, 'publisher_verification': 'required_before_packaging',
        'signatures': {'bareline.update.json': 'release_public_key', 'runtime.json': 'release_public_key',
                       'catalog.json': 'catalog_public_key', 'bareline.release-authority.json': 'offline_root_public_key'}})
    return output

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    compare_args = sub.add_parser('compare')
    for name in ('config', 'first', 'second', 'output'):
        compare_args.add_argument('--'+name, type=Path, required=True)
    check = sub.add_parser('verify-handoff'); check.add_argument('--handoff', type=Path, required=True)
    signed_check = sub.add_parser('verify-signed')
    signed_check.add_argument('--handoff', type=Path, required=True)
    signed_check.add_argument('--signed-dir', type=Path, required=True)
    meta = sub.add_parser('prepare-metadata')
    for name in ('handoff', 'signed-dir', 'output'):
        meta.add_argument('--'+name, type=Path, required=True)
    meta.add_argument('--expires-unix', type=int, required=True)
    args = parser.parse_args()
    try:
        if args.command == 'compare':
            compare(args.config, args.first, args.second, args.output)
        elif args.command == 'verify-handoff':
            verify_handoff(args.handoff)
        elif args.command == 'verify-signed':
            verify_handoff(args.handoff)
            for name in EXES:
                verify_signed_bytes(args.handoff/'unsigned'/name, args.signed_dir/name)
        else:
            metadata(args.handoff, args.signed_dir, args.expires_unix, args.output)
    except (ValueError, OSError, KeyError, zipfile.BadZipFile, config_api.ConfigurationError) as error:
        print(f'release handoff refused: {error}', file=sys.stderr)
        return 2
    print('PASS: requested handoff step completed; release approval remains pending')
    return 0

if __name__ == '__main__':
    raise SystemExit(main())
