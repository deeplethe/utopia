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
test("class counts must be nonnegative integers", () => {
 for (const n of ["-1", "1.5", "", " "]) {const r=subset("@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\nx:A a rdfs:Class .\n",n); assert.equal(r.status,2,JSON.stringify(n));}
});
test("zero remains a valid empty subset",()=>assert.equal(subset("@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\nx:A a rdfs:Class .\n","0").status,0));
