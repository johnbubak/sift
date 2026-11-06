#!/usr/bin/env python3
import json, subprocess, sys

labels = json.load(open('docs/labels.json'))
hits = 0
for i, item in enumerate(labels, 1):
    q = item['query']
    expected = item['expected_doc_id']
    import os
    sid = os.environ.get('SWEEP_ID', 'measure')
    out = subprocess.check_output(
        ['./target/release/sift', 'search', q, '--limit', '5', '--scope', f'{sid}{i}', '--json'],
        text=True, stderr=subprocess.DEVNULL)
    data = json.loads(out[out.index('['):])
    ids = [r['doc_id'] for r in data]
    ok = expected in ids
    hits += int(ok)
    print(f"{q!r}: {'HIT' if ok else 'MISS'} (expected {expected}, got {ids})")
print(f"\nrecall@5: {hits}/{len(labels)}")
sys.exit(0 if hits == len(labels) else 1)