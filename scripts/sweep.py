#!/usr/bin/env python3
import json, subprocess, re, os

original = open('weights.toml').read()
labels = json.load(open('docs/labels.json'))
best = None
for rrf_k in [30.0, 60.0, 90.0]:
    for recency_weight in [0.1, 0.3, 0.5]:
        for body_weight in [2.0, 3.0, 4.0]:
            for snippet_tokens in [12, 18, 24]:
                text = original
                text = re.sub(r'^rrf_k = .*', f'rrf_k = {rrf_k}', text, flags=re.M)
                text = re.sub(r'^recency_weight = .*', f'recency_weight = {recency_weight}', text, flags=re.M)
                text = re.sub(r'^body_weight  = .*', f'body_weight  = {body_weight}', text, flags=re.M)
                text = re.sub(r'^snippet_tokens = .*', f'snippet_tokens = {snippet_tokens}', text, flags=re.M)
                open('weights.toml', 'w').write(text)
                env = dict(os.environ, SWEEP_ID=f'sweep{rrf_k}{recency_weight}{body_weight}{snippet_tokens}')
                r = subprocess.run(['python3', 'scripts/measure.py'], capture_output=True, text=True, env=env)
                recall = 0
                for line in r.stdout.splitlines():
                    if line.startswith('recall@5:'):
                        recall = int(line.split(':')[1].split('/')[0])
                print(f'rrf_k={rrf_k} recency={recency_weight} body={body_weight} snippet={snippet_tokens} recall@5={recall}/{len(labels)}')
                if best is None or recall > best[0]:
                    best = (recall, rrf_k, recency_weight, body_weight, snippet_tokens)
print('best:', best)
# restore original
open('weights.toml', 'w').write(original)