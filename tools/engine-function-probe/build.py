#!/usr/bin/env python3
"""Build the test-only source-bound Bullseye consumer bundle, receipt last."""
import importlib.util
import json
import os
from pathlib import Path
import secrets
import shutil
import subprocess
import sys

ROOT=Path(__file__).resolve().parents[2]
spec=importlib.util.spec_from_file_location('build_contract', ROOT/'scripts/build-khook-runtime.py')
contract=importlib.util.module_from_spec(spec)
spec.loader.exec_module(contract)


def build():
    output=ROOT/'build/engine-function-live'
    if output.is_symlink(): raise contract.BuildError('bundle output is a symlink')
    receipt=output/'engine-function-build.json'
    receipt.unlink(missing_ok=True)
    revision=contract._require_clean_source(ROOT)
    contract._check_prerequisites(ROOT)
    token=secrets.token_hex(32)
    if output.exists(): shutil.rmtree(output)
    output.mkdir(parents=True)
    args=['docker','run','--rm','--platform','linux/amd64','-v',f'{ROOT}:/repo','-w','/repo',
          '-v','s2script-cargo:/usr/local/cargo/registry',
          '-e',f'S2FN_BUILD_TOKEN={token}','-e',f'S2FN_SOURCE_REVISION={revision}']
    for name,flag in [('S2_BUILD_CPUS','--cpus'),('S2_BUILD_MEMORY','--memory')]:
        if os.getenv(name): args += [flag,os.environ[name]]
    for name in ['S2_BUILD_JOBS','CARGO_BUILD_JOBS']:
        value=os.getenv(name)
        if value:
            if not value.isdigit() or value.startswith('0'): raise contract.BuildError(f'{name} must be a positive integer')
            args += ['-e',f'{name}={value}']
    # Trust only this bind mount and its exact initialized submodule directories.
    paths=['/repo']+['/repo/'+p.split()[1] for p in contract._git(ROOT,'submodule','status','--recursive').splitlines()]
    setup='\n'.join('git config --global --add safe.directory '+__import__('shlex').quote(p) for p in paths)
    args += [os.getenv('S2_BUILD_IMAGE','rust:bullseye'),'bash','-c',setup+'\nexec bash tools/engine-function-probe/build-live.sh']
    subprocess.run(args,cwd=ROOT,check=True)
    contract._require_same_source(ROOT,revision)
    fixtures={}
    for name,prefix in [('acceptance','examples/engine-function-acceptance/'),('witness','examples/engine-function-acceptance/witness/')]:
        stage=output/(name+'-src')
        for tracked in contract._git(ROOT,'ls-files','--',prefix).splitlines():
            relative=Path(tracked).relative_to(prefix)
            if name=='acceptance' and relative.parts[0]=='witness': continue
            source_file=ROOT/tracked; target=stage/relative
            if source_file.is_symlink(): raise contract.BuildError('fixture symlink refused')
            target.parent.mkdir(parents=True,exist_ok=True); shutil.copyfile(source_file,target)
        (stage/'src/build_identity.ts').write_text(f'export const REVISION: string = "{revision}";\nexport const TOKEN: string = "{token}";\n')
        (stage/'tsconfig.json').write_text(json.dumps({'extends':str(ROOT/'tsconfig.base.json'),'include':['src',str(ROOT/'packages/sdk/globals.d.ts')]}))
        contract._default_fixture_builder(ROOT,stage)
        built=list((stage/'dist').glob('*.s2sp'))
        if len(built)!=1: raise contract.BuildError('expected one freshly built '+name+' fixture')
        fixtures[name]=built[0]
    addon=output/'addons/s2script'
    shutil.copytree(ROOT/'dist/addons/s2script',addon)
    # Default/base plugins are supplied by the operator, never bundled as evidence.
    shutil.rmtree(addon/'plugins',ignore_errors=True); (addon/'plugins').mkdir()
    for name,archive in fixtures.items():
        shutil.copyfile(archive,addon/('plugins/engine-function-'+name+'.s2sp'))
    shutil.copyfile(output/'native/s2_engine_function_probe.so',addon/'bin/linuxsteamrt64/s2_engine_function_probe.so')
    vdf=output/'addons/metamod/s2_engine_function_probe.vdf'; vdf.parent.mkdir(parents=True)
    vdf.write_text('"Metamod Plugin"\n{\n "alias" "engine_function_probe"\n "file" "addons/s2script/bin/linuxsteamrt64/s2_engine_function_probe"\n}\n')
    files={str(p.relative_to(output/'addons')):contract._sha256(p) for p in (output/'addons').rglob('*') if p.is_file()}
    contract._require_same_source(ROOT,revision)
    receipt.write_text(json.dumps({'schema':1,'kind':'engine-function-build','source':revision,'token':token,'files':files},indent=2)+'\n')
    print(receipt)

if __name__=='__main__':
    try: build()
    except (contract.BuildError,subprocess.CalledProcessError,OSError) as e:
        print(f'error: {e}',file=sys.stderr); sys.exit(1)
