import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, rmSync, mkdirSync, writeFileSync, copyFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import { execFileSync } from 'node:child_process';
const script = resolve('scripts/framework-version.sh');
function repo(t) {
 const cwd=mkdtempSync(tmpdir()+'/s2-version-'); t.after(()=>rmSync(cwd,{recursive:true,force:true}));
 const git=(...a)=>execFileSync('git',a,{cwd,stdio:'pipe'}).toString().trim();
 git('init'); git('config','user.email','test@example.com'); git('config','user.name','Test'); git('commit','--allow-empty','-m','initial');
 const version=(arg='',env={})=>execFileSync('bash',[script,arg],{cwd,env:{...process.env,VERSION:'',GITHUB_REF_NAME:'',GITHUB_REF_TYPE:'',...env}}).toString().trim();
 return {git,version};
}
test('release overrides and tags resolve consistently',t=>{const {git,version}=repo(t);assert.equal(version('v1.2.3'),'1.2.3');assert.equal(version('',{VERSION:'v2.0.0'}),'2.0.0');git('tag','v0.5.12');assert.equal(version(),'0.5.12');});
test('untagged commits share a concrete development version',t=>{const {git,version}=repo(t);git('tag','v0.5.12');git('commit','--allow-empty','-m','next');assert.match(version(),/^0\.5\.12-dev\.1\.g[0-9a-f]+$/);assert.equal(version('',{GITHUB_REF_TYPE:'branch',GITHUB_REF_NAME:'main'}),version());});
test('repositories without release tags use an explicit dev version',t=>{const {version}=repo(t);assert.match(version(),/^0\.0\.0-dev\.1\.g[0-9a-f]+$/);assert.throws(()=>version('main'));});

test('canonical versions include build metadata but reject leading zeros',t=>{const {version}=repo(t);assert.equal(version('1.2.3-rc.1+build.7'),'1.2.3-rc.1+build.7');for(const bad of ['01.2.3','1.02.3','1.2.03','1.2.3-01'])assert.throws(()=>version(bad));});

test('release packaging refuses a mismatched compiled framework version before staging',t=>{
 const cwd=mkdtempSync(tmpdir()+'/s2-release-');t.after(()=>rmSync(cwd,{recursive:true,force:true}));
 mkdirSync(cwd+'/scripts');
 for(const name of ['package-release.sh','framework-version.sh'])copyFileSync(resolve('scripts',name),cwd+'/scripts/'+name);
 for(const name of ['s2script/bin/linuxsteamrt64/s2script.so','s2script/bin/linuxsteamrt64/libs2script_core.so','metamod/s2script.vdf','s2script/VERSION']) {
  const path=cwd+'/dist/addons/'+name;mkdirSync(resolve(path,'..'),{recursive:true});writeFileSync(path,name.endsWith('VERSION')?'1.0.0\n':'fixture');
 }
 assert.throws(()=>execFileSync('bash',['scripts/package-release.sh','2.0.0'],{cwd,stdio:'pipe'}),error=>error.stderr.toString().includes('packaged framework version does not match 2.0.0'));
});
