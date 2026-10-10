import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
const script = new URL("./subset.mjs", import.meta.url);
function subset(text, n) {
 const dir=fs.mkdtempSync(path.join(os.tmpdir(), "utopia-subset-"));
 try { const src=path.join(dir,"schema.ttl"); fs.writeFileSync(src,text); return spawnSync(process.execPath,[script.pathname,src,n],{encoding:"utf8"}); }
 finally {fs.rmSync(dir,{recursive:true,force:true});}
}
test("prefix-only ontology retains its final declaration",()=>{
 const text="@prefix schema: <https://schema.org/> .\n@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .";
 const r=subset(text,"1"); assert.equal(r.status,0,r.stderr); assert.ok(r.stdout.includes(text),r.stdout);
});
test("empty source remains valid",()=>assert.equal(subset("","1").status,0));
