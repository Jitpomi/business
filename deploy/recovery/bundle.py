#!/usr/bin/env python3
"""Manifested recovery packaging. Encryption is performed by the Rust recovery CLI.
Never package the separate archive encryption key into the bundle it unlocks.
"""
import argparse, hashlib, json, os, stat, zipfile
from pathlib import Path, PurePosixPath
MAX = 256 * 1024 * 1024

def pack(directory, output):
    directory, output = Path(directory).resolve(), Path(output).resolve()
    if directory in output.parents:
        raise ValueError('Archive must be outside its source directory')
    entries = list(directory.rglob('*'))
    if any(p.is_symlink() for p in entries):
        raise ValueError('Recovery source cannot contain symlinks')
    files = sorted(p for p in entries if p.is_file())
    if any(p.is_symlink() or p.name == 'MANIFEST.json' or p.name.startswith('recovery-key') for p in files):
        raise ValueError('Unexpected manifest, symlink or separate recovery key in source bundle')
    if not files or len(files) > 5000 or sum(p.stat().st_size for p in files) > MAX:
        raise ValueError('Recovery bundle exceeds bounds or is empty')
    manifest = {p.relative_to(directory).as_posix(): {'sha256': hashlib.sha256(p.read_bytes()).hexdigest(), 'size': p.stat().st_size} for p in files}
    descriptor = os.open(output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, 'wb') as stream, zipfile.ZipFile(stream, 'w', compression=zipfile.ZIP_DEFLATED) as archive:
        for p in files:
            archive.write(p, p.relative_to(directory).as_posix())
        archive.writestr('MANIFEST.json', json.dumps({'version': 1, 'files': manifest}, sort_keys=True))
    output.chmod(0o600)
    return {'files': len(files), 'sha256': hashlib.sha256(output.read_bytes()).hexdigest()}

def unpack(archive, destination):
    destination = Path(destination).resolve()
    if destination.exists():
        raise ValueError('Recovery destination must be new')
    with zipfile.ZipFile(archive) as z:
        infos = z.infolist()
        if len(infos) > 5001 or len({i.filename for i in infos}) != len(infos) or sum(i.file_size for i in infos) > MAX:
            raise ValueError('Invalid recovery archive bounds or duplicate entries')
        if z.getinfo('MANIFEST.json').file_size > 2*1024*1024:
            raise ValueError('Manifest exceeds bounds')
        manifest = json.loads(z.read('MANIFEST.json'))
        if manifest.get('version') != 1 or set(manifest['files']) | {'MANIFEST.json'} != {i.filename for i in infos}:
            raise ValueError('Recovery manifest/file set mismatch')
        contents = {}
        for name, expected in manifest['files'].items():
            path = PurePosixPath(name)
            if path.is_absolute() or '..' in path.parts or '\\' in name or name in ('', '.'):
                raise ValueError('Unsafe recovery path')
            info = z.getinfo(name)
            if stat.S_ISLNK(info.external_attr >> 16) or info.is_dir():
                raise ValueError('Recovery entries must be regular files')
            value = z.read(name)
            if len(value) != expected['size'] or hashlib.sha256(value).hexdigest() != expected['sha256']:
                raise ValueError('Recovery file integrity mismatch')
            contents[name] = value
        destination.mkdir(mode=0o700)
        for name, value in contents.items():
            target = destination / name
            target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
            with target.open('xb') as f:
                f.write(value)
            target.chmod(0o600)
    return {'verified_files': len(contents)}

if __name__ == '__main__':
    p=argparse.ArgumentParser();p.add_argument('command',choices=['pack','unpack']);p.add_argument('source');p.add_argument('destination');a=p.parse_args()
    print(json.dumps(pack(a.source,a.destination) if a.command=='pack' else unpack(a.source,a.destination)))
