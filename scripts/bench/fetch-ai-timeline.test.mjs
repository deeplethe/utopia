import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
const s=fs.readFileSync(new URL('./fetch-ai-timeline.mjs',import.meta.url),'utf8').replace(/^#!.*\n/,'').replace(/^import .*;\n/m,'');
for(const extract of ['',undefined,'Real timeline body']) test(`extract ${JSON.stringify(extract)}`,()=>{
 let output='';
 vm.runInNewContext(s,{URL,execFileSync:()=>JSON.stringify({query:{pages:[{title:'Owned',extract}]}}),process:{stderr:{write:()=>{}},stdout:{write:x=>output+=x}}});
 const docs=JSON.parse(output).docs;
 assert.equal(docs.length,extract?15:0);
});
