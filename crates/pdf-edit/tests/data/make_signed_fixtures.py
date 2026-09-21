# Synthetic signed fixtures. No corpus input.
#
#   signed-by-the-test-chain.pdf      an ordinary signed document
#   signed-and-protected-r3.pdf       the same, in a document encrypted with
#                                     RC4-128 (R3) and an empty user password
#
# Both are signed by the chain in crates/pdf-security/tests/data/chain-*.der,
# whose private keys are deliberately not kept: the fixtures are checked, never
# re-signed. Run with the key directory as the first argument to rebuild them.
#
# The point of the second fixture is one rule: the value of a signature's
# /Contents is not encrypted, because the signature covers the file's bytes as
# they lie and would have to be written before it could be computed. Every
# other string in it is ciphertext.
from pathlib import Path
import hashlib, struct, subprocess, sys, tempfile

HOLE = 4000
PADDING = bytes.fromhex('28bf4e5e4e758a4164004e56fffa01082e2e00b6d0683e802f0ca9fe6453697a')
IDENTIFIER = bytes.fromhex('66d36a30a97e0f16f39955c6221e0c2a')
OWNER = bytes.fromhex('1d1ff7011663408661b4e24dcc3db7f376f0fb37e02315869a2bc769442d356a')
PERMISSIONS = -4


def rc4(key, data):
    s = list(range(256))
    j = 0
    for i in range(256):
        j = (j + s[i] + key[i % len(key)]) % 256
        s[i], s[j] = s[j], s[i]
    i = j = 0
    out = bytearray()
    for byte in data:
        i = (i + 1) % 256
        j = (j + s[i]) % 256
        s[i], s[j] = s[j], s[i]
        out.append(byte ^ s[(s[i] + s[j]) % 256])
    return bytes(out)


def file_key():
    key = hashlib.md5(PADDING + OWNER + struct.pack('<i', PERMISSIONS) + IDENTIFIER).digest()
    for _ in range(50):
        key = hashlib.md5(key).digest()
    return key


def user_value(key):
    u = hashlib.md5(PADDING + IDENTIFIER).digest()
    for i in range(20):
        u = rc4(bytes(b ^ i for b in key), u)
    return u


def object_key(key, number):
    return hashlib.md5(key + struct.pack('<i', number)[:3] + b'\x00\x00').digest()[:16]


def assemble(objects, trailer_extra, range_text, hole):
    signature = (
        b"<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached "
        b"/Name " + objects['name'] + b" /M " + objects['when'] + b" "
        b"/Reason " + objects['reason'] + b" /Location (Vientiane) "
        b"/ByteRange " + range_text + b" /Contents <" + hole + b"> >>"
    )
    body = objects['body'] + [signature] + objects['after']
    out = bytearray(b"%PDF-1.7\n")
    offsets = []
    for index, obj in enumerate(body):
        offsets.append(len(out))
        out += b"%d 0 obj\n" % (index + 1) + obj + b"\nendobj\n"
    xref = len(out)
    out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(body) + 1)
    for offset in offsets:
        out += b"%010d 00000 n \n" % offset
    out += (b"trailer\n<< /Size %d /Root 1 0 R " % (len(body) + 1)) + trailer_extra + \
        b" >>\nstartxref\n%d\n%%%%EOF\n" % xref
    return bytes(out)


def build(objects, trailer_extra, keys, out_path):
    placeholder = b"[0000000000 0000000000 0000000000 0000000000]"
    hole = b"0" * (HOLE * 2)
    draft = assemble(objects, trailer_extra, placeholder, hole)
    start = draft.index(b"/Contents <") + len(b"/Contents <")
    first = start - 1
    second = start + HOLE * 2 + 1
    more = len(draft) - second
    real = b"[%010d %010d %010d %010d]" % (0, first, second, more)
    assert len(real) == len(placeholder)
    final = assemble(objects, trailer_extra, real, hole)
    covered = final[:first] + final[second:second + more]

    with tempfile.TemporaryDirectory() as room:
        room = Path(room)
        (room / 'covered.bin').write_bytes(covered)
        subprocess.run([
            'openssl', 'cms', '-sign', '-binary', '-in', str(room / 'covered.bin'),
            '-signer', f'{keys}/leaf.pem', '-inkey', f'{keys}/leaf.key',
            '-certfile', f'{keys}/chain.pem', '-md', 'sha256', '-outform', 'DER',
            '-out', str(room / 'signed.der'), '-nosmimecap',
        ], check=True)
        blob = (room / 'signed.der').read_bytes()
    assert len(blob) <= HOLE, len(blob)
    signed = bytearray(final)
    signed[start:start + HOLE * 2] = blob.hex().encode() + b"0" * ((HOLE - len(blob)) * 2)
    Path(out_path).write_bytes(bytes(signed))
    print('wrote', out_path, len(signed), 'bytes')


def plain(keys, here):
    objects = {
        'body': [
            b"<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R /Perms << /DocMDP 7 0 R >> >>",
            b"<< /Type /Pages /MediaBox [0 0 300 300] /Kids [3 0 R] /Count 1 >>",
            b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << >> >>",
            b"<< /Length 0 >>\nstream\n\nendstream",
            b"<< /Fields [6 0 R] /SigFlags 3 >>",
            b"<< /Type /Annot /Subtype /Widget /FT /Sig /T (Signature1) /V 7 0 R "
            b"/Rect [0 0 0 0] /P 3 0 R >>",
        ],
        'after': [],
        'name': b"(Phan Somchai)",
        'reason': b"(I agree)",
        'when': b"(D:20260919120000+07'00')",
    }
    build(objects, b"", keys, here / 'signed-by-the-test-chain.pdf')


def protected(keys, here):
    key = file_key()
    # Object 7 is the signature dictionary, so its strings are encrypted
    # under object 7's key -- all but /Contents, which is not encrypted at all.
    seven = object_key(key, 7)
    six = object_key(key, 6)
    hexed = lambda data: b"<" + rc4(seven, data).hex().encode() + b">"
    objects = {
        'body': [
            b"<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R /Perms << /DocMDP 7 0 R >> >>",
            b"<< /Type /Pages /MediaBox [0 0 300 300] /Kids [3 0 R] /Count 1 >>",
            b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << >> >>",
            b"<< /Length 0 >>\nstream\n\nendstream",
            b"<< /Fields [6 0 R] /SigFlags 3 >>",
            b"<< /Type /Annot /Subtype /Widget /FT /Sig /T <" +
            rc4(six, b"Signature1").hex().encode() +
            b"> /V 7 0 R /Rect [0 0 0 0] /P 3 0 R >>",
        ],
        'after': [
            b"<< /Filter /Standard /V 2 /R 3 /Length 128 /P %d /O <%s> /U <%s00000000000000000000000000000000> >>"
            % (PERMISSIONS, OWNER.hex().encode(), user_value(key).hex().encode()),
        ],
        'name': hexed(b"Phan Somchai"),
        'reason': hexed(b"I agree"),
        'when': hexed(b"D:20260919120000+07'00'"),
    }
    identifier = IDENTIFIER.hex().encode()
    trailer = b"/Encrypt 8 0 R /ID [<%s> <%s>]" % (identifier, identifier)
    build(objects, trailer, keys, here / 'signed-and-protected-r3.pdf')


if __name__ == '__main__':
    keys = sys.argv[1] if len(sys.argv) > 1 else '.'
    here = Path(__file__).parent
    plain(keys, here)
    protected(keys, here)
