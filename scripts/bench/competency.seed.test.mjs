import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
const s=fs.readFileSync(new URL('./competency.mjs',import.meta.url),'utf8');
const main=s.slice(s.indexOf('async function main()'),s.indexOf('\nmain().catch'));
test('seed deduplicates new questions in the same input',async()=>{
 const created=[];
 const api=async(method,url,body)=>{
  if(method==='POST'){created.push(body);return {};}
  if(url.endsWith('/report'))return {questions:{},proposals:{}};
  return [];
 };
 await vm.runInNewContext(main+';main()', {login:async()=>{},args:{seed:'owned','only-report':true},KB:'owned',api,fs:{readFileSync:()=>JSON.stringify([{question:'New question'},{question:' new QUESTION '},{question:'Different'}])},log:()=>{},console:{log:()=>{}}});
 assert.equal(created.length,2);
});
