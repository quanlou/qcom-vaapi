#!/usr/bin/env python3
"""Package an already-built ARM64 AV1 driver and its companion; no root needed."""
import argparse
import hashlib
import re
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parent.parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--driver', type=Path, required=True)
    parser.add_argument('--companion', type=Path, required=True)
    parser.add_argument('--ffmpeg-source', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--qualification', type=Path)
    args = parser.parse_args()
    version = tomllib.loads((ROOT / 'rust/Cargo.toml').read_text())['package']['version']
    deb_version = version.replace('-rc.', '~rc.')
    driver, companion = args.driver.resolve(), args.companion.resolve()
    for library in (driver, companion):
        data = library.read_bytes()
        if data[:6] != b'\x7fELF\x02\x01' or struct.unpack_from('<H', data, 18)[0] != 183:
            parser.error(f'{library}: expected a little-endian ARM64 ELF library')
    if f'qcom-vaapi {version}:'.encode() not in driver.read_bytes():
        parser.error('driver label does not match Cargo version')
    if subprocess.check_output(['dpkg', '--print-architecture'], text=True).strip() != 'arm64':
        parser.error('build this package on ARM64 so dependency detection uses the target libraries')
    args.output.mkdir(parents=True, exist_ok=True)
    output = args.output.resolve() / f'qcom-vaapi_{deb_version}_arm64.deb'
    if output.exists():
        parser.error(f'refusing to overwrite {output}')
    with tempfile.TemporaryDirectory(prefix='qcom-vaapi-deb-') as scratch:
        work = Path(scratch)
        package = work / 'package'
        metadata = package / 'DEBIAN'
        libraries = package / 'usr/lib/aarch64-linux-gnu'
        docs = package / 'usr/share/doc/qcom-vaapi'
        metadata.mkdir(parents=True)
        (libraries / 'dri').mkdir(parents=True)
        docs.mkdir(parents=True)
        for source, target in [(driver, libraries / 'dri/msm_drv_video.so'),
                               (companion, libraries / 'libiris_av1_complete.so')]:
            shutil.copyfile(source, target)
            target.chmod(0o755)
        shutil.copyfile(ROOT / 'LICENSE', docs / 'LICENSE.driver-MIT')
        shutil.copyfile(ROOT / 'producers/LICENSE-GPL-2.0-or-later', docs / 'LICENSE.producer-patches')
        for name in ['COPYING.GPLv2', 'COPYING.GPLv3', 'COPYING.LGPLv2.1', 'COPYING.LGPLv3', 'LICENSE.md']:
            shutil.copyfile(args.ffmpeg_source / name, docs / ('FFmpeg-' + name))
        release_doc = ROOT / 'docs/releases' / f'{version}.md'
        shutil.copyfile(release_doc if release_doc.exists() else ROOT / 'README.md', docs / 'README')
        if args.qualification:
            shutil.copyfile(args.qualification, docs / 'qualification.json')
        (docs / 'copyright').write_text(
            'Rust driver and original companion: MIT; see LICENSE.driver-MIT.\n'
            'Producer patches: GPL-2.0-or-later; see LICENSE.producer-patches.\n'
            'Linked FFmpeg code retains its licenses; see FFmpeg-LICENSE.md and COPYING files.\n'
            f'Corresponding sources: https://github.com/quanlou/qcom-vaapi/releases/tag/v{version}\n')
        (work / 'debian').mkdir()
        (work / 'debian/control').write_text('Source: qcom-vaapi\nSection: video\nPriority: optional\n'
            'Maintainer: qcom-vaapi contributors\nStandards-Version: 4.7.0\n\n'
            'Package: qcom-vaapi\nArchitecture: arm64\nDescription: Qualcomm Iris VA-API driver\n')
        dependency_output = subprocess.check_output(
            ['dpkg-shlibdeps', '-O', '-e' + str(driver), '-e' + str(companion)], cwd=work, text=True)
        dependencies = next(line.removeprefix('shlibs:Depends=') for line in dependency_output.splitlines()
                            if line.startswith('shlibs:Depends='))
        # The libva driver ABI targets 1.24 even when linked symbols are older.
        match = re.search(r'libva2 \(>= ([^)]+)\)', dependencies)
        if match and subprocess.run(['dpkg', '--compare-versions', match[1], 'lt', '2.24']).returncode == 0:
            dependencies = dependencies.replace(match[0], 'libva2 (>= 2.24)')
        (metadata / 'control').write_text(
            f'Package: qcom-vaapi\nVersion: {deb_version}\nArchitecture: arm64\n'
            'Maintainer: qcom-vaapi contributors\nSection: video\nPriority: optional\n'
            f'Depends: {dependencies}\n'
            'Description: Qualcomm Iris VA-API driver with experimental AV1\n'
            ' H.264, HEVC, VP9 and experimental 8-bit AV1 on Snapdragon X Elite X1E80100.\n'
            ' Includes the AV1 CBS companion; compatible Iris kernel and firmware required.\n')
        (metadata / 'md5sums').write_text(''.join(
            hashlib.md5(p.read_bytes()).hexdigest() + '  ' + str(p.relative_to(package)) + '\n'
            for p in sorted((package / 'usr').rglob('*')) if p.is_file()))
        subprocess.run(['dpkg-deb', '--root-owner-group', '--build', str(package), str(output)], check=True)
    print(output)


if __name__ == '__main__':
    main()
