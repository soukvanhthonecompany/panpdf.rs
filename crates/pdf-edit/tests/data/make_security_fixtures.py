# Synthetic R3 PDF fixtures; user=view, owner=master. No corpus input.
# Calibrated against the qpdf-derived /U in pdf-security r3_parameters.
from pathlib import Path
import hashlib, struct
padding=bytes.fromhex('28bf4e5e4e758a4164004e56fffa01082e2e00b6d0683e802f0ca9fe6453697a')
identifier=bytes.fromhex('66d36a30a97e0f16f39955c6221e0c2a')
owner=bytes.fromhex('1d1ff7011663408661b4e24dcc3db7f376f0fb37e02315869a2bc769442d356a')
def rc4(key,data):
 s=list(range(256));j=0
 for i in range(256):
  j=(j+s[i]+key[i%len(key)])%256;s[i],s[j]=s[j],s[i]
 i=j=0;out=bytearray()
 for byte in data:
  i=(i+1)%256;j=(j+s[i])%256;s[i],s[j]=s[j],s[i];out.append(byte^s[(s[i]+s[j])%256])
 return bytes(out)
for p,name in [(-3104,'restricted'),(-4,'modifiable')]:
 key=hashlib.md5((b'view'+padding)[:32]+owner+struct.pack('<i',p)+identifier).digest()
 for _ in range(50):key=hashlib.md5(key).digest()
 u=hashlib.md5(padding+identifier).digest()
 for i in range(20):u=rc4(bytes(b^i for b in key),u)
 if name=='restricted':assert u.hex()=='b1b69d3ca429a6fa7a6f27d64de7cd53'
 objkey=hashlib.md5(key+b'\x04\x00\x00\x00\x00').digest()[:16]
 content=rc4(objkey,b'BT /F1 12 Tf 20 50 Td (Hello) Tj ET')
 objects=[b'<< /Type /Catalog /Pages 2 0 R >>',b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>',b'<< /Length '+str(len(content)).encode()+b' >>\nstream\n'+content+b'\nendstream',b'<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>',f'<< /Filter /Standard /V 2 /R 3 /Length 128 /P {p} /O <{owner.hex()}> /U <{u.hex()}00000000000000000000000000000000> >>'.encode()]
 pdf=bytearray(b'%PDF-1.7\n'); offsets=[]
 for i,o in enumerate(objects,1):offsets.append(len(pdf));pdf.extend(f'{i} 0 obj\n'.encode()+o+b'\nendobj\n')
 xref=len(pdf);pdf.extend(b'xref\n0 7\n0000000000 65535 f \n')
 for offset in offsets:pdf.extend(f'{offset:010} 00000 n \n'.encode())
 pdf.extend(f'trailer\n<< /Size 7 /Root 1 0 R /Encrypt 6 0 R /ID [<{identifier.hex()}> <{identifier.hex()}>] >>\nstartxref\n{xref}\n%%EOF\n'.encode())
 (Path(__file__).parent / f'{name}-r3.pdf').write_bytes(pdf)
