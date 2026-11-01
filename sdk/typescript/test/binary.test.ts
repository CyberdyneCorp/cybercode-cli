import assert from "node:assert/strict";
import { test } from "node:test";
import { createHash } from "node:crypto";
import { byteOffset } from "../src/index.js";
import { json, mockClient } from "./support.js";

const opened = { document_id: "bin_one", path: "fixture.bin", length: "9007199254741000", revision: "observed", source: "live", within_edit_size_limit: false };
const raw = Buffer.from([0, 255, 128, 27]);
const page = { revision: "observed", source: "live", offset: "9007199254740993", length: "4", base64: raw.toString("base64"), page_sha256: createHash("sha256").update(raw).digest("hex"), eof: false };

test("binary byte helpers preserve every byte and exact offsets without inference", async () => {
  const {client,calls}=mockClient((req)=> {
    if (req.url.pathname.endsWith("/bytes")) return json(200,{data:page});
    if (req.url.pathname.endsWith("/search")) return json(200,{data:{revision:"observed",source:"live",offsets:["9007199254740993"],next_offset:null,scanned:"7"}});
    return json(200,{data:req.method === "DELETE" ? true : opened});
  });
  const doc=await client.binary.open("ses_one","fixture.bin",{expectedRoot:"/work"});
  assert.deepEqual(await doc.bytes(9007199254740993n,4),new Uint8Array(raw));
  assert.equal(calls[1]!.url.searchParams.get("offset"),"9007199254740993");
  assert.equal(calls[1]!.url.searchParams.get("session_id"),"ses_one");
  const matches=await doc.search("00ff","9007199254740993");
  assert.deepEqual(matches.offsets,["9007199254740993"]);
  await doc.close();
  assert.equal(calls[3]!.method,"DELETE");
  assert.equal(calls.length,4);
  assert.equal(JSON.parse(calls[0]!.body!).expected_root,"/work");
});

test("binary offsets reject rounded numbers, overflow and noncanonical spellings",()=>{
  assert.equal(byteOffset((1n<<64n)-1n),"18446744073709551615");
  for(const value of ["01","-1","0x10","18446744073709551616","", "1.2"]){assert.throws(()=>byteOffset(value));}
  // The runtime guard also rejects numbers from untyped JavaScript clients.
  assert.throws(()=>byteOffset(1 as unknown as string));
});

test("binary pages reject stale, oversized, malformed and mismatched evidence",async()=>{
  for(const invalid of [
    {...page,revision:"stale"}, {...page,offset:"1"}, {...page,length:"3"},
    {...page,base64:"AA==".repeat(5)}, {...page,base64:"!!!!"}, {...page,page_sha256:"0".repeat(64)},
  ]){
    const {client}=mockClient(req=>json(200,{data:req.url.pathname.endsWith("/bytes")?invalid:opened}));
    const doc=await client.binary.open("ses_one","fixture.bin");
    await assert.rejects(()=>doc.bytes("9007199254740993",4));
    await assert.rejects(()=>doc.bytes("0",1048577));
  }
});


test("binary page bytes cannot exceed the opened document range", async () => {
  const { client } = mockClient(req => json(200, {
    data: req.url.pathname.endsWith("/bytes") ? page : { ...opened, length: "9007199254740995" },
  }));
  const doc = await client.binary.open("ses_one", "fixture.bin");
  assert.equal("document_id" in doc.metadata, false);
  await assert.rejects(() => doc.bytes("9007199254740993", 4));
});
