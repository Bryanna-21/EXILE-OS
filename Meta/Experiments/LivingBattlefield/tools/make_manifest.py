#!/usr/bin/env python3
"""Write MANIFEST.csv/json: filename, dimensions, format, size, SHA-256 for every binary asset/preview."""
import csv, hashlib, json, os, sys
from PIL import Image
root = sys.argv[1] if len(sys.argv) > 1 else '.'
rows = []
for d in ('assets', 'previews'):
    for f in sorted(os.listdir(os.path.join(root, d))):
        p = os.path.join(root, d, f)
        if not os.path.isfile(p): continue
        h = hashlib.sha256(open(p, 'rb').read()).hexdigest()
        if f.endswith('.png'):
            im = Image.open(p); dim = f'{im.width}x{im.height}'; fmt = f'PNG {im.mode}'
        else:
            dim = '-'; fmt = f.rsplit('.', 1)[-1].upper()
        rows.append(dict(file=f'{d}/{f}', dimensions=dim, format=fmt, bytes=os.path.getsize(p), sha256=h))
with open(os.path.join(root, 'MANIFEST.csv'), 'w', newline='') as fh:
    w = csv.DictWriter(fh, fieldnames=list(rows[0])); w.writeheader(); w.writerows(rows)
json.dump(rows, open(os.path.join(root, 'MANIFEST.json'), 'w'), indent=1)
print(len(rows), 'entries')
