import test from "node:test";
import assert from "node:assert/strict";
import {api,cookieHeader} from "./lib.mjs";
test("multiple response cookies survive subsequent requests and refresh",async()=>{
 const original=globalThis.fetch; const seen=[]; let call=0;
 globalThis.fetch=async(_url,init)=>{seen.push(init.headers.cookie);const headers=new Headers();
 for(const c of (call++ === 0 ? ["session=abc; HttpOnly; Path=/", "prefs=dark; Path=/"] : ["session=refreshed; HttpOnly; Path=/"])) headers.append("Set-Cookie",c);
 return new Response("{}",{headers});};
 try {await api("GET","/first");assert.equal(cookieHeader(),"session=abc; prefs=dark");await api("GET","/second");assert.equal(seen[1],"session=abc; prefs=dark");assert.equal(cookieHeader(),"session=refreshed; prefs=dark");}
 finally{globalThis.fetch=original;}
});
test("responses without Set-Cookie retain session state",async()=>{
 const original=globalThis.fetch, before=cookieHeader();globalThis.fetch=async()=>new Response("{}");
 try{await api("GET","/unchanged");assert.equal(cookieHeader(),before);}finally{globalThis.fetch=original;}
});
