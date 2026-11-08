"""Package upstream notices with the APK; don't include machine-specific paths."""
import json
import pathlib
import shutil
import subprocess
import sys
import argparse

repo = pathlib.Path(__file__).resolve().parent.parent
parser = argparse.ArgumentParser()
parser.add_argument('destination', type=pathlib.Path)
parser.add_argument('--manifest', type=pathlib.Path, default=repo / 'android/native/Cargo.toml')
parser.add_argument('--platform', default='aarch64-linux-android')
parser.add_argument('--features')
args = parser.parse_args()
destination = args.destination
command = [
    'cargo', 'metadata', '--manifest-path', str(args.manifest),
    '--format-version', '1', '--locked', '--filter-platform', args.platform,
]
if args.features:
    command.extend(['--features', args.features])
metadata = json.loads(subprocess.check_output(command, text=True, encoding='utf-8'))
resolved = {node['id'] for node in metadata['resolve']['nodes']}
index = []
for package in metadata['packages']:
    if package['id'] not in resolved or package['source'] is None:
        continue
    name = f"{package['name']}-{package['version']}"
    source = pathlib.Path(package['manifest_path']).parent
    notices = [p for p in source.iterdir() if p.is_file() and p.name.upper().startswith(('LICENSE', 'COPYING', 'NOTICE', 'COPYRIGHT'))]
    target = destination / 'rust' / name
    target.mkdir(parents=True, exist_ok=True)
    for notice in notices:
        shutil.copyfile(notice, target / notice.name)
    index.append({'name': package['name'], 'version': package['version'], 'license': package['license'], 'repository': package['repository'], 'notices': [p.name for p in notices]})
(destination / 'rust-dependencies.json').write_text(json.dumps(index, indent=2) + '\n', encoding='utf-8')
print(f'Packaged Rust notices: {len(index)} dependencies')
