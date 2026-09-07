import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync, mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import ts from 'typescript';
const prelude=readFileSync(new URL('../../../core/js/prelude.js',import.meta.url),'utf8');
const source=prelude.slice(prelude.indexOf('  var __s2_cookie_defs'),prelude.indexOf('  // --- @s2script/http'));
function fixture() {
  let admitted=true; const calls=[];
  const context=vm.createContext({
    __s2_client_matches:(slot,token)=>slot===3 && token==='current', __s2_client_token:c=>c.token,
    __s2_cookie_session:(...args)=>{calls.push(args);return JSON.stringify(admitted);},
    __s2_cookie_set_authid:(...args)=>{calls.push(args);return admitted;},
  });
  vm.runInContext(source,context);
  return { cookies:context.__s2pkg_cookies.Cookies,calls,reject:()=>{admitted=false;} };
}
test('shipped cookie wrapper exposes host admission and fails closed for stale/bot identities',()=>{
  const f=fixture(), cookie=f.cookies.register('k');
  const client={slot:3,token:'current',steamId:'account'};
  assert.equal(f.cookies.set(client,cookie,'v'),true);
  assert.equal(f.cookies.setAuthId('account',cookie,'v'),true);
  f.reject();
  assert.equal(f.cookies.set(client,cookie,'v'),false);
  assert.equal(f.cookies.setAuthId('account',cookie,'v'),false);
  const n=f.calls.length;
  assert.equal(f.cookies.set({...client,token:'stale'},cookie,'v'),false);
  assert.equal(f.cookies.setAuthId('0',cookie,'v'),false);
  assert.equal(f.calls.length,n);
});
test('SDK contract permits boolean structural mocks and prevents old void mocks',()=>{
  const dir=mkdtempSync(join(tmpdir(),'s2-cookie-types-'));
  try {
    const path=join(dir,'check.ts');
    writeFileSync(path,`import {Cookies} from ${JSON.stringify(new URL('../cookies',import.meta.url).pathname)};
const mock: Pick<typeof Cookies, 'set'|'setAuthId'>={set:()=>false,setAuthId:()=>true};
const accepted: boolean = mock.setAuthId('account', {name:'k',access:0,default:''},'v');
// @ts-expect-error a void mock cannot fulfill synchronous admission reporting
const obsolete: Pick<typeof Cookies,'set'>={set:()=>{}};
`);
    const program=ts.createProgram([path],{strict:true,noEmit:true,skipLibCheck:true,target:ts.ScriptTarget.ES2022,moduleResolution:ts.ModuleResolutionKind.Bundler,module:ts.ModuleKind.ESNext});
    assert.deepEqual(ts.getPreEmitDiagnostics(program).map(d=>ts.flattenDiagnosticMessageText(d.messageText,'\n')),[]);
  } finally {rmSync(dir,{recursive:true,force:true});}
});
