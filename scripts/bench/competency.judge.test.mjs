import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
const s=fs.readFileSync(new URL('./competency.mjs',import.meta.url),'utf8');
const main=s.slice(s.indexOf('async function main()'),s.indexOf('\nmain().catch'));
for(const verdict of [true,false,'false',1]) test(`judge correct=${JSON.stringify(verdict)}`,async()=>{
 let result;
 const api=async(method,url,body)=>{
  if(method==='POST'){result=body;return {};}
  if(url.endsWith('/ontology'))return {entity_types:[],relation_types:[]};
  if(url.endsWith('/report'))return {questions:{},proposals:{}};
  return [{id:'q',status:'accepted',question:'question',expected_answer:'expected',last_result:{answer:'answer'}}];
 };
 await vm.runInNewContext(main+';main()', {login:async()=>{},args:{rejudge:true},KB:'owned',api,judgeEndpoint:()=>({}),judgeChat:async()=>JSON.stringify({correct:verdict}),JUDGE:'owned',log:()=>{},console:{log:()=>{}}});
 assert.equal(result.answered,verdict===true);
});
