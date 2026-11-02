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

test("binary draft helpers retain explicit retry revisions and never convert bytes to text", async () => {
  const draft = { ...opened, document_id: undefined, length: "4", revision: "draft_a", source: "draft", within_edit_size_limit: true,
    read_only: false, prepared: true, dirty: false, can_undo: false, can_redo: false, base_sha256: "a".repeat(64), pending_operation: null, edit_supported: true, edit_decision: "allow" };
  const receipt = { id: "patch_once", kind: "patch", payload_sha256: "b".repeat(64), outcome: "acknowledged", revision: "draft_b", file_effect: "none", reason: null, notification_pending: false };
  const { client, calls } = mockClient(req => {
    if (req.url.pathname.endsWith("/prepare") || req.url.pathname.endsWith("/recover")) return json(200, { data: draft });
    if (req.url.pathname.endsWith("/patches")) return json(200, { data: { operation: receipt, status: { ...draft, revision: "draft_b", dirty: true, can_undo: true } } });
    if (req.url.pathname.endsWith("/bytes")) return json(200, { data: { ...page, revision: "draft_b", source: "draft", offset: "0" } });
    if (req.url.pathname.includes("/operations/")) return json(200, { data: receipt });
    return json(200, { data: { ...opened, length: "4", within_edit_size_limit: true } });
  });
  const doc = await client.binary.open("ses_one", "fixture.bin", { readOnly: false });
  await doc.prepare();
  assert.equal(doc.metadata.revision, "draft_a");
  const batch = { revision: "draft_a", splices: [{ offset: "0", delete_length: "1", expected_hex: "00", insert_hex: "ff" }] };
  await doc.patch("patch_once", batch);
  assert.equal(doc.metadata.revision, "draft_b");
  assert.deepEqual(await doc.bytes(0n, 4), new Uint8Array(raw));
  await doc.patch("patch_once", batch);
  const patches = calls.filter(call => call.url.pathname.endsWith("/patches"));
  assert.equal(patches.length, 2);
  assert.equal(patches[0]!.body, patches[1]!.body, "retry changed its revision after acknowledgement");
  assert.deepEqual(await doc.operation("patch_once"), receipt);
  const recovered = await client.binary.recover("ses_one", doc.id, { readOnly: true });
  assert.equal(recovered.id, doc.id);
  assert.equal(recovered.metadata.source, "draft");
  assert.equal(JSON.parse(calls.at(-1)!.body!).read_only, true);
  const before = calls.length;
  await assert.rejects(() => doc.patch("bad/id", batch));
  await assert.rejects(() => doc.patch("bad_bytes", { ...batch, splices: [{ ...batch.splices[0]!, expected_hex: "0" }] }));
  await assert.rejects(() => doc.patch("wrong_length", { ...batch, splices: [{ ...batch.splices[0]!, delete_length: "2" }] }));
  assert.equal(calls.length, before, "invalid mutation reached the server");
});

test("binary pages in flight before preparation cannot replace the new draft view", async () => {
  let resolvePage!: (response: Response) => void;
  let started!: () => void;
  const running = new Promise<void>(resolve => { started = resolve; });
  const { client } = mockClient(req => {
    if (req.url.pathname.endsWith("/bytes")) {
      started();
      return new Promise<Response>(resolve => { resolvePage = resolve; });
    }
    if (req.url.pathname.endsWith("/prepare")) return json(200, { data: { ...opened, revision: "draft_new", source: "draft" } });
    return json(200, { data: opened });
  });
  const doc = await client.binary.open("ses_one", "fixture.bin");
  const pending = doc.bytes("9007199254740993", 4);
  await running;
  await doc.prepare();
  resolvePage(json(200, { data: page }));
  await assert.rejects(() => pending, /Stale binary page/);
  assert.equal(doc.metadata.revision, "draft_new");
});

test("binary mutation transport failure requires explicit recovery instead of automatic retry", async () => {
  const { client, calls } = mockClient(() => { throw new TypeError("response lost"); }, { retry: { retries: 4, delayMs: 0 } });
  await assert.rejects(() => client.binary.open("ses_one", "fixture.bin"));
  assert.equal(calls.length, 1, "lost open response allocated another document automatically");
});
