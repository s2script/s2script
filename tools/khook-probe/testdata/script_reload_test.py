#!/usr/bin/env python3
"""Compile the actual resident-probe reload driver with the engine boundary mocked."""
from pathlib import Path
import os
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[3]
source = (ROOT / 'tools/khook-probe/plugin.cpp').read_text()
start = source.index('static void R6DriveScriptReload()')
end = source.index('\n}', start) + 2
collect_start = source.index('static void CollectScriptReload()')
collect_end = source.index('\n}', collect_start) + 2
program = r'''
#include "acceptance_observer.h"
#include <cassert>
#include <map>
#include <string>
struct CEntityInstance { int index, serial; } target{42,7}, marker{43,8};
std::map<int,CEntityInstance*> entities;
std::map<std::string,std::string> cvars;
s2khook::ScriptReloadObservation g_script_reload;
std::string g_run_id="run", g_artifact_identity="digest";
const char* kScriptReloadExpected="{}";
std::string emitted_result, emitted_actual;
std::string JsonBool(bool value){return value?"true":"false";}
void PushPending(const char*,const char*,const char*,const std::string&,const char*){emitted_result="pending";}
void PushRec(const char*,const char*,const char*,const char* result,const std::string&,const std::string& actual,const char*){
 emitted_result=result;emitted_actual=actual;
}
int invokes=0, copies=1;
bool suppress=false, missing_post=false, stale=false;
bool JsAcceptPresent(){return true;}
std::string ProbeCvarStr(const char* name){return cvars[name];}
int ProbeCvarInt(const char* name,int fallback){return cvars.count(name)?std::stoi(cvars[name]):fallback;}
bool ProbeSetCvarString(const char* name,const std::string& value){cvars[name]=value;return true;}
std::string JsonEscape(const char* s){return s;}
CEntityInstance* EntByIndex(int index){return entities.count(index)?entities[index]:nullptr;}
bool EntIndexSerial(CEntityInstance* e,int* index,int* serial){if(!e)return false;if(index)*index=e->index;if(serial)*serial=e->serial;return true;}
bool R6EntityIsTriggerPush(CEntityInstance* e){return e==&target;}
s2khook::OriginalObservation R6InvokeTouch(CEntityInstance* e){
 assert(e==&target);++invokes;
 const auto gen=cvars["s2_khook_accept_live"];
 std::string trace;
 if(stale)trace+="1:pre,1:post,";
 for(int i=0;i<copies;++i){trace+=gen+":pre,";if(!missing_post)trace+=gen+":post,";}
 cvars["s2_khook_accept_reload_trace"]=trace;
 s2khook::OriginalObservation value;value.Pre();value.Post(suppress);return value;
}
void before(){
 entities={{42,&target},{43,&marker}};cvars.clear();invokes=0;copies=1;suppress=missing_post=stale=false;
 g_script_reload={};g_script_reload.target_index=42;g_script_reload.target_serial=7;g_script_reload.old_generation=1;g_script_reload.armed=true;
 cvars={{"s2_khook_accept_run","run"},{"s2_khook_accept_artifact","digest"},{"s2_khook_accept_live","1"},
 {"s2_khook_accept_reload_ready","1"},{"s2_khook_accept_reload_marker","43"},{"s2_khook_accept_reload_unloaded","0"}};
}
'''
program += source[start:end]
program += source[collect_start:collect_end]
program += r'''
void after(bool keep_marker=false,bool same_generation=false){
 cvars["s2_khook_accept_live"]=same_generation?"1":"2";cvars["s2_khook_accept_reload_ready"]=same_generation?"1":"2";
 cvars["s2_khook_accept_reload_unloaded"]="1";
 if(!keep_marker)entities.erase(43);
 for(int i=0;i<4;++i)R6DriveScriptReload();
}
int main(){
 before();R6DriveScriptReload();assert(invokes==1&&g_script_reload.before_seen);
 R6DriveScriptReload();assert(invokes==1&&!g_script_reload.after_seen);
 after();assert(invokes==2&&g_script_reload.Passed());R6DriveScriptReload();assert(invokes==2);
 CollectScriptReload();assert(emitted_result=="pass");
 assert(cvars["s2_khook_accept_reload_ack"].find("\"stage\":\"after\"")!=std::string::npos);
 before();R6DriveScriptReload();stale=true;after();assert(g_script_reload.after.stale==2&&!g_script_reload.Passed());
 before();R6DriveScriptReload();copies=2;after();assert(g_script_reload.after.pre==2&&!g_script_reload.Passed());
 before();R6DriveScriptReload();missing_post=true;after();assert(g_script_reload.after.post==0&&!g_script_reload.Passed());
 before();R6DriveScriptReload();copies=0;after();assert(!g_script_reload.Passed());
 before();R6DriveScriptReload();suppress=true;after();assert(g_script_reload.after.original.original==0&&!g_script_reload.Passed());
 before();R6DriveScriptReload();after(true);assert(g_script_reload.after_seen&&!g_script_reload.old_resource_removed&&!g_script_reload.Passed());
 CollectScriptReload();assert(emitted_result=="fail"&&emitted_actual.find("\"old_resource_removed\":false")!=std::string::npos);
 before();R6DriveScriptReload();after(false,true);assert(g_script_reload.after_seen&&!g_script_reload.Passed());
 before();cvars["s2_khook_accept_artifact"]="other";R6DriveScriptReload();assert(invokes==0);
 before();cvars["s2_khook_accept_run"]="old";R6DriveScriptReload();assert(invokes==0);
 before();R6DriveScriptReload();target.serial=9;after();assert(!g_script_reload.Passed());
}
'''
with tempfile.TemporaryDirectory(prefix='khook-script-reload-') as d:
    cpp=Path(d)/'reload.cpp';cpp.write_text(program)
    exe=Path(d)/'reload'
    subprocess.run([os.environ.get('CXX','g++'),'-std=c++17','-O1','-g','-fsanitize=address,undefined','-fno-sanitize-recover=all','-I',str(ROOT/'tools/khook-probe'),str(cpp),'-o',str(exe)],check=True)
    subprocess.run([str(exe)],check=True)
print('PASS: actual native script reload driver and negative witnesses')
