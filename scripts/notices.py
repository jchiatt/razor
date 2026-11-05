#!/usr/bin/env python3
"""Print each distinct license text once, with the release packages it covers."""
import json
from pathlib import Path
import subprocess
import sys
from urllib.request import urlopen

target = sys.argv[1]
metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--locked', '--format-version', '1', '--filter-platform', target]))
tree = subprocess.check_output(['cargo', 'tree', '--locked', '-e', 'normal', '--target', target, '--prefix', 'none', '--format', '{p}'], text=True)
closure = {tuple(line.split()[:2]) for line in tree.splitlines()[1:]}
packages = [p for p in metadata['packages'] if (p['name'], 'v' + p['version']) in closure]
print('# Third-party notices\n')
notices = {}
for p in sorted(packages, key=lambda p: (p['name'], p['version'])):
    directory = Path(p['manifest_path']).parent
    files = sorted(f for f in directory.iterdir() if f.is_file() and f.name.lower().startswith(('license', 'copying', 'notice')))
    files = files or sorted(directory.glob('*/LICENSE*'))
    name = f'{p["name"]} {p["version"]}'
    if not files:
        print(f'## {name}\n\nLicense: {p["license"] or "unspecified"}; no license file beside the manifest. Repository: {p["repository"] or "unspecified"}\n')
    for file in files:
        text = file.read_text(encoding='utf-8', errors='replace').strip()
        notices.setdefault(text, set()).add(name)
for url, name in [('vercel-labs/agent-browser/v0.41.1/LICENSE', 'agent-browser 0.41.1 (Apache-2.0)'),
                  ('BurntSushi/ripgrep/15.2.0/LICENSE-MIT', 'ripgrep 15.2.0 (MIT or Unlicense)'),
                  ('PCRE2Project/pcre2/pcre2-10.45/LICENCE.md', 'PCRE2 10.45, built into ripgrep 15.2.0 (BSD-3-Clause)'),
                  ('jemalloc/jemalloc/81034ce1f1373e37dc865038e1bc8eeecf559ce8/COPYING', 'jemalloc 5.3.1, built into ripgrep 15.2.0 on Linux (BSD-2-Clause)'),
                  ('jqlang/jq/jq-1.8.2/COPYING', 'jq 1.8.2 (MIT)'),
                  ('kkos/oniguruma/4ef89209a239c1aea328cf13c05a2807e5c146d1/COPYING', 'Oniguruma, built into jq 1.8.2 (BSD-2-Clause)')]:
    text = urlopen(f'https://raw.githubusercontent.com/{url}', timeout=30).read().decode().strip()
    notices.setdefault(text, set()).add(name)
for text, names in notices.items():
    print(f'## {", ".join(sorted(names))}\n\n```text\n{text}\n```\n')
print('## Claude Agent SDK\n\nFetched from npm at install time under Anthropic’s terms; not included in release archives.')
