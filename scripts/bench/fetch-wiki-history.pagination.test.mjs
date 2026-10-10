import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
const source=fs.readFileSync(new URL("./fetch-wiki-history.mjs",import.meta.url),"utf8");
const begin=source.indexOf("function revisions(title) {");
const end=source.indexOf("\nconst day =",begin);
function revisions(api){return vm.runInNewContext(source.slice(begin,end)+"\nrevisions",{api});}
test("remaining continuation at the safety cap fails",()=>{
 let calls=0;const fn=revisions(()=>({query:{pages:[{title:"Sample",revisions:[{revid:++calls}]}]},continue:{rvcontinue:"more"}}));
 assert.throws(()=>fn("Sample"),/incomplete|pagination|limit/i);assert.equal(calls,40);
});
test("completed histories preserve the resolved title and all revisions",()=>{
 let calls=0;const fn=revisions(()=>({query:{pages:[{title:"Resolved",revisions:[{revid:++calls}]}]},...(calls<2?{continue:{rvcontinue:"more"}}:{})}));
 const result=fn("Alias");assert.equal(result.real,"Resolved");assert.deepEqual(Array.from(result.revs,r=>r.revid),[1,2]);
});

test("failed histories abort the real CLI before corpus publication",()=>{
 const script=source.replace(/^import .*;$/gm, "");
 const writes=[]; const sentinel=new Error("exit"); let code;
 const context={URL,console:{error(){},log(){}},process:{argv:["node","history","--manifest"],stderr:{write(){}},stdout:{write(){throw new Error("unexpected corpus output");}},exit(n){code=n;throw sentinel;}},fs:{writeFileSync(...a){writes.push(a);}},execFileSync(){return JSON.stringify({query:{pages:[{title:"Sample",revisions:[{revid:1,timestamp:"2024-01-01T00:00:00Z",size:10}]}]},continue:{rvcontinue:"more"}});}};
 assert.throws(()=>vm.runInNewContext(script,context),e=>e===sentinel);assert.equal(code,1);assert.equal(writes.length,0);
});
