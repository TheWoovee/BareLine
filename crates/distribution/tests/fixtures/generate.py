# SPDX-License-Identifier: MPL-2.0
"""Regenerate public, TEST-ONLY Minisign vectors using OpenSSL Ed25519.
The deterministic 0..31 seed is deliberately public and MUST NEVER be a release key.
No production key is read. Outputs contain public keys and signed test metadata only.
"""
import base64, hashlib, json, pathlib, subprocess, tempfile
OPENSSL = r"C:\Program Files\Git\usr\bin\openssl.exe"
root = pathlib.Path(__file__).parent
key_id = b"TESTONLY"
with tempfile.TemporaryDirectory(prefix="bareline-signature-fixture-") as task:
    task = pathlib.Path(task)
    key = task / "test-key.der"
    key.write_bytes(bytes.fromhex("302e020100300506032b657004220420") + bytes(range(32)))
    public = subprocess.check_output([OPENSSL, "pkey", "-inform", "DER", "-in", str(key), "-pubout", "-outform", "DER"])[-32:]
    (root / "public-key.txt").write_text(base64.b64encode(b"Ed" + key_id + public).decode(), encoding="ascii")
    def sign(data):
        source, output = task / "message", task / "signature"
        source.write_bytes(data)
        subprocess.run([OPENSSL, "pkeyutl", "-sign", "-rawin", "-inkey", str(key), "-keyform", "DER", "-in", str(source), "-out", str(output)], check=True)
        return output.read_bytes()
    base = dict(schema_version=1, metadata_version=3, version="1.0.0", channel="stable", artifact_type="bareline-x64", platform="windows-x64", publisher="test-publisher", length=4, sha256=hashlib.sha256(b"test").hexdigest(), minimum_protocol=1, expires_unix=200)
    cases = dict(valid={}, expired={"expires_unix":99}, rollback={"metadata_version":2}, channel={"channel":"preview"}, publisher={"publisher":"other"}, platform={"platform":"linux-x64"}, protocol={"minimum_protocol":2}, artifact={"artifact_type":"other"}, length={"length":4097}, hash={"sha256":"0"*64})
    for name, changes in cases.items():
        data = json.dumps(base | changes, separators=(",", ":"), sort_keys=True).encode()
        signature = sign(hashlib.blake2b(data).digest())
        comment = b"TEST ONLY; never authorize production updates"
        global_sig = sign(signature + comment)
        (root / (name + ".json")).write_bytes(data)
        (root / (name + ".minisig")).write_text("untrusted comment: public test fixture\n" + base64.b64encode(b"ED" + key_id + signature).decode() + "\ntrusted comment: " + comment.decode() + "\n" + base64.b64encode(global_sig).decode() + "\n", encoding="ascii")
