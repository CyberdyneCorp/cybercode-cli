import assert from "node:assert/strict";
import { test } from "node:test";
import { CyberClientError } from "../src/index.js";
import { json, mockClient } from "./support.js";

test("generated native owner controls keep exact review scope and Location", async () => {
 const {client,calls}=mockClient(()=>json(200,{data:{id:"cug_fixture"}}));
 await client.at("/native-project").session.computerAllow("ses_owner",{preview_id:"cup_review",confirm:true,acknowledge_unknown:false});
 assert.equal(calls[0]?.url.pathname,"/api/v1/sessions/ses_owner/computer/allow");assert.equal(calls[0]?.headers.get("x-cyber-directory"),"%2Fnative-project");
 assert.deepEqual(JSON.parse(calls[0]?.body??"{}"),{preview_id:"cup_review",confirm:true,acknowledge_unknown:false});
});
test("generated native frame reads return private bounded bytes over authenticated HTTP", async () => {
 const png=new Uint8Array([137,80,78,71,13,10,26,10]);const {client,calls}=mockClient(()=>new Response(png,{headers:{"content-type":"image/png"}}));
 const bytes=await client.at("/native-project").session.computerFrame("ses_owner","cuf_fixture");assert.deepEqual(bytes,png);assert.equal(calls[0]?.headers.get("authorization"),`Basic ${btoa("cyber:secret")}`);assert.equal(calls[0]?.headers.get("x-cyber-directory"),"%2Fnative-project");
});
test("private frame transport rejects other media and oversized PNG payloads without replay", async () => {
 for(const response of [new Response("<html>bad</html>",{headers:{"content-type":"text/html"}}),new Response(new Uint8Array(8*1024*1024+1),{headers:{"content-type":"image/png"}})]) {
  const {client,calls}=mockClient(()=>response);await assert.rejects(client.session.computerFrame("ses_owner","cuf_fixture"),(error:unknown)=>error instanceof CyberClientError&&error.reason==="MalformedResponse");assert.equal(calls.length,1);
 }
});
