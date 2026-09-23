#!/usr/bin/env python3
"""Drive an already deployed resident probe. Missing evidence always fails closed."""
import argparse
import hashlib
import json
import re
import secrets
import subprocess
import sys
import time
from pathlib import Path

CONTROLLED={'generation-armed','ignite-scalar-compatibility','acquire-compatibility','hud-compatibility','novel-reentry-peer','typed-suppression-peer','removal-before-free'}

def judge(records, source, token, run):
    errors=[]; missing=[]
    # RCON snapshots and server logs repeat rows; exact retransmissions are not new callbacks.
    relevant=list({json.dumps(r,sort_keys=True):r for r in records if r.get('kind') in {'engine-function-observation','engine-function-script','engine-function-witness'} and r.get('run')==run}.values())
    for r in relevant:
        if r.get('source')!=source or r.get('token')!=token: errors.append('mixed build identity')
        if r.get('result')=='fail': errors.append('observed failure: '+r.get('case','unknown'))
    observations=[r for r in relevant if r['kind']=='engine-function-observation']
    residents={(r.get('resident'),r.get('pid')) for r in observations}
    if len(residents)!=1 or any(not a or not b for a,b in residents): missing.append('one unchanged resident native process')
    generations=sorted({r.get('generation',0) for r in observations if r.get('case')=='generation-armed'})
    if len(generations)!=2 or any(not isinstance(g,int) or g<=0 for g in generations): missing.append('two armed generations')
    witnesses=[r for r in relevant if r['kind']=='engine-function-witness']
    witness_generations={r.get('witnessGeneration') for r in witnesses}
    if len(witness_generations)!=1 or any(not isinstance(g,int) or g<=0 for g in witness_generations): missing.append('one live witness owner across stimulus reload')
    if any(r.get('generation') not in generations for r in witnesses): errors.append('stale or wrong witness generation')
    if any(r.get('event')=='invalid-scope' for r in witnesses): errors.append('public invocation marker unavailable')
    if generations and not any(r.get('event')=='unloaded' and r.get('generation')==generations[-1] for r in witnesses): missing.append('witness owner teardown')
    for generation in generations:
        rows=[r for r in observations if r.get('generation')==generation and r.get('result')=='pass']
        for case in CONTROLLED:
            if not any(r.get('case')==case and r.get('provenance')=='controlled-native-abi' for r in rows): missing.append(f'{generation}:{case}')
        real=[r for r in rows if r.get('case')=='real-acquire' and r.get('provenance')=='real-engine-acquire']
        if not real: missing.append(f'{generation}:real engine acquisition')
        if len(real)>1: errors.append('ambiguous native acquisition evidence')
        for r in real:
            f=r.get('facts',{})
            if any(f.get(k)!=v for k,v in {'outer_pre':1,'deliberate_nested_pre':1,'non_skipped_completions':2,'peer_pre':2,'peer_post':2,'direct_body_counter':False}.items()) or not f.get('validation_receipt'):
                errors.append('real engine completion evidence invalid')
        scripts=[r for r in relevant if r['kind']=='engine-function-script' and r.get('generation')==generation]
        for event in ['arm','unloaded','acquire-stimulus']:
            if not any(r.get('event')==event for r in scripts): missing.append(f'{generation}:script {event}')
        if not any(r.get('event')=='acquire-stimulus' and r.get('facts',{}).get('itemCreated') is True for r in scripts): missing.append(f'{generation}:owned bot item')
        public=[r for r in witnesses if r.get('generation')==generation]
        if not any(r.get('event')=='armed' for r in public): missing.append(f'{generation}:witness arm')
        invocations=real[0].get('facts',{}).get('invocations',[]) if real else []
        if len(invocations)!=2 or {i.get('id') for i in invocations}!={1,2}: missing.append(f'{generation}:scoped native invocation identities')
        if real and invocations:
            observed_results={i.get('id'):i.get('result') for i in invocations}
            if observed_results.get(1)!=real[0]['facts'].get('outer_result') or observed_results.get(2)!=real[0]['facts'].get('nested_result'): errors.append('native invocation results disagree with outer/nested completion')
        callbacks=[r for r in public if r.get('event') in ('acquire-pre','acquire-post')]
        for invocation in invocations:
            identity={k:invocation.get(k) for k in ('id','operation','slot','pawn','method','defIndex')}
            if any(not isinstance(v,int) for v in identity.values()) or identity['operation']<=0: errors.append('invalid scoped invocation identity')
            if not any(r.get('event')=='acquire-stimulus' and all(r.get('facts',{}).get(k)==v for k,v in {'operation':identity['operation'],'botSlot':identity['slot'],'pawn':identity['pawn']}.items()) for r in scripts): missing.append(f'{generation}:invocation stimulus correlation')
            for event in ('acquire-pre','acquire-post'):
                matches=[r for r in callbacks if r.get('event')==event and all(r.get('facts',{}).get(k)==v for k,v in identity.items())]
                if len(matches)!=1: missing.append(f'{generation}:{identity["id"]}:unique public {event}')
                elif event=='acquire-post' and (matches[0]['facts'].get('result')!=invocation.get('result') or matches[0]['facts'].get('skipped') is not False or invocation.get('skipped') is not False): errors.append('public POST differs from observed native completion')
        for row in callbacks:
            if not any(all(row.get('facts',{}).get(k)==invocation.get(k) for k in ('id','operation','slot','pawn','method','defIndex')) for invocation in invocations): errors.append('wrong or stale public invocation')

    return {'result':'fail' if errors else 'pending' if missing else 'pass','errors':sorted(set(errors)),'missing':sorted(set(missing))}

def rows(text):
    result=[]; decoder=json.JSONDecoder()
    for line in text.splitlines():
        start=line.find('{"kind":"engine-function-')
        if start<0: continue
        try: row,_=decoder.raw_decode(line[start:])
        except ValueError: continue
        result.append(row)
    return result


def drive(args):
    root=Path(__file__).resolve().parents[2]
    bundle=Path(args.bundle).resolve(); manifest=json.loads((bundle/'engine-function-build.json').read_text())
    source=manifest['source']; token=manifest['token']
    if not re.fullmatch('[0-9a-f]{40}',source) or not re.fullmatch('[0-9a-f]{64}',token): raise RuntimeError('invalid bundle identity')
    for rel,digest in manifest['files'].items():
        p=bundle/'addons'/rel
        if p.is_symlink() or not p.is_file() or hashlib.sha256(p.read_bytes()).hexdigest()!=digest: raise RuntimeError('bundle artifact mismatch: '+rel)
    run=secrets.token_hex(8); evidence=root/'.gate/engine-functions'/source/run; evidence.mkdir(parents=True)
    records=[]; raw=[]
    compose=['docker','compose','-f',args.docker]
    def rcon(command):
        out=subprocess.check_output([sys.executable,args.rcon,command],cwd=root,text=True,stderr=subprocess.STDOUT)
        raw.append(out); records.extend(rows(out)); (evidence/'rcon.log').write_text('\n'.join(raw)); return out
    def collect_logs():
        out=subprocess.check_output(compose+['logs','--no-color','--since',started,'cs2'],cwd=root,text=True,stderr=subprocess.STDOUT)
        (evidence/'server.log').write_text(out); return rows(out)
    def wait_for(predicate, command='s2_engine_probe runtime'):
        end=time.monotonic()+30
        while time.monotonic()<end:
            rcon(command)
            records.extend(collect_logs())
            if predicate(records): return
            time.sleep(.5)
        raise RuntimeError('required observation timeout: '+command)
    def mapped(runtime):
        pid=runtime.get('pid')
        if not isinstance(pid,int) or pid<=0: raise RuntimeError('runtime pid absent')
        maps=subprocess.check_output(compose+['exec','-T','cs2','cat',f'/proc/{pid}/maps'],cwd=root,text=True)
        (evidence/f'maps-{len(raw)}.txt').write_text(maps)
        # Verify each consumer already loaded, not merely the path on disk.
        for name in ['s2script.so','libs2script_core.so','s2_engine_function_probe.so']:
            rel='s2script/bin/linuxsteamrt64/'+name
            expected=manifest['files'][rel]
            matches=[line.split(maxsplit=5) for line in maps.splitlines() if line.rstrip().endswith('/'+name)]
            paths={m[5] for m in matches}
            if len(paths)!=1: raise RuntimeError('loaded module missing/ambiguous: '+name)
            path=next(iter(paths))
            stat=subprocess.check_output(compose+['exec','-T','cs2','stat','-Lc','%d %i',path],cwd=root,text=True).split()
            dev,ino=map(int,stat); major=((dev>>8)&0xfff)|((dev>>32)&~0xfff); minor=(dev&0xff)|((dev>>12)&~0xff)
            if any(int(m[4])!=ino or tuple(int(x,16) for x in m[3].split(':'))!=(major,minor) for m in matches): raise RuntimeError('mapped inode differs from installed file: '+name)
            digest=subprocess.check_output(compose+['exec','-T','cs2','sha256sum',path],cwd=root,text=True).split()[0]
            if digest!=expected: raise RuntimeError('loaded consumer differs from source-bound bundle: '+name)
        for name in ('acceptance','witness'):
            rel=f's2script/plugins/engine-function-{name}.s2sp'
            installed=str(Path(path).parents[3]/rel)
            digest=subprocess.check_output(compose+['exec','-T','cs2','sha256sum',installed],cwd=root,text=True).split()[0]
            if digest!=manifest['files'][rel]: raise RuntimeError('installed fixture archive differs from source-bound bundle: '+name)
        # The provider remains the installed official host; no second host load.
        hosts={m.split(maxsplit=5)[5] for m in maps.splitlines() if m.rstrip().endswith('/addons/metamod/bin/linuxsteamrt64/metamod.2.cs2.so')}
        if len(hosts)!=1: raise RuntimeError('one installed Metamod provider required')
    started=time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime())
    rcon('s2_engine_probe runtime')
    runtimes=[r for r in records if r.get('kind')=='engine-function-runtime']
    if not runtimes or runtimes[-1].get('source')!=source or runtimes[-1].get('token')!=token: raise RuntimeError('resident probe is not this bundle')
    runtime=runtimes[-1]; mapped(runtime)
    rcon(f'bot_quota_mode normal; bot_join_after_player 0; mp_limitteams 0; bot_add_ct s2fn_{run}')
    try:
        for attempt in range(2):
            rcon(f's2_engine_accept arm {run}')
            wait_for(lambda rs: len({r.get('generation') for r in rs if r.get('case')=='generation-armed' and r.get('run')==run})==attempt+1)
            generation=max(r['generation'] for r in records if r.get('case')=='generation-armed' and r.get('run')==run)
            rcon(f's2_engine_witness arm {run} {generation} {token}')
            wait_for(lambda rs:any(r.get('kind')=='engine-function-witness' and r.get('event')=='armed' and r.get('generation')==generation and r.get('run')==run for r in rs))
            rcon(f's2_engine_probe exercise {run}')
            # Give the freshly spawned bot a real pawn before the single stimulus.
            time.sleep(2)
            rcon(f's2_engine_accept acquire {run}')
            wait_for(lambda rs:any(r.get('case')=='real-acquire' and r.get('generation')==generation and r.get('run')==run for r in rs),'s2_engine_probe runtime')
            rcon('sm plugins unload @example/engine-function-acceptance')
            wait_for(lambda rs:any(r.get('case')=='removal-before-free' and r.get('generation')==generation and r.get('run')==run for r in rs),f's2_engine_probe collect {run}')
            if attempt==0: rcon('sm plugins load @example/engine-function-acceptance')
        rcon('sm plugins unload @example/engine-function-witness')
        wait_for(lambda rs:any(r.get('kind')=='engine-function-witness' and r.get('event')=='unloaded' and r.get('run')==run for r in rs))
        records.extend(collect_logs()); mapped(runtime)
        rcon('s2_engine_probe runtime')
        if any(r.get('resident')!=runtime['resident'] or r.get('pid')!=runtime['pid'] for r in records if r.get('kind')=='engine-function-runtime'): raise RuntimeError('native resident changed during script reload')
        result=judge(records,source,token,run)
        (evidence/'records.json').write_text(json.dumps(records,indent=2)+'\n')
        (evidence/'result.json').write_text(json.dumps(result,indent=2)+'\n')
        print(json.dumps(result)); print(evidence)
        return 0 if result['result']=='pass' else 1
    finally:
        # Keep native modules/server resident. Remove only our specifically named bot.
        rcon(f'bot_kick s2fn_{run}')

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--docker',required=True); p.add_argument('--rcon',required=True)
    p.add_argument('--bundle',default='build/engine-function-live')
    try: sys.exit(drive(p.parse_args()))
    except (OSError,ValueError,KeyError,RuntimeError,subprocess.CalledProcessError) as e:
        print('live proof failed: '+str(e),file=sys.stderr); sys.exit(1)
