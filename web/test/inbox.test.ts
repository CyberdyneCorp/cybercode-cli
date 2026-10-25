import assert from "node:assert/strict";
import { test } from "node:test";
import type { InboxRow, MessageRecord } from "../../sdk/typescript/src/generated/types.js";
import { MessageInbox, type InboxContext } from "../src/inbox.js";
import { ApiFault, type Port, type RequestOptions } from "../src/transport.js";

const row = (id = "msgx_one"): InboxRow => ({ message_id: id, parts: [{ type: "text", text: "<script>untrusted()</script>" }], delivery: "hold", source: "session", status: "held", digest: `digest-${id}`, admitted_seq: 1 });
const message = (id = "msgx_one"): MessageRecord => ({ message_id: id, session_id: "ses_one", direction: "in", from: "frontend", to: "backend", origin_kind: "session", outcome: "held", size: 30, sha256: `sha-${id}`, text: "<script>untrusted()</script>", depth: 0, hops: 1, created_at: "2026-10-24T00:00:00Z", updated_at: "2026-10-24T00:00:00Z", seq: 1 });
function harness() {
  let context: InboxContext | undefined = { session: "ses_one", directory: "/project", actionable: true, signal: new AbortController().signal };
  const fake = {
    rows: [row()], messages: [message()], posts: [] as { path: string; options: RequestOptions }[], fail: false, commit: false, deny: false,
    hook: undefined as ((path: string) => Promise<unknown> | undefined) | undefined,
    async request<T>(path: string, options: RequestOptions = {}): Promise<T> {
      const custom = this.hook?.(path); if (custom) return await custom as T;
      if (options.method === "POST") {
        this.posts.push({ path, options: structuredClone({ ...options, signal: undefined }) });
        if (this.deny) throw new ApiFault("refused", 403);
        if (this.commit || !this.fail) { this.rows = []; this.messages = []; }
        if (this.fail) { this.fail = false; throw new ApiFault("network"); }
        return undefined as T;
      }
      return structuredClone(path.endsWith("/inbox") ? { data: this.rows } : { data: this.messages, cursor: {} }) as T;
    },
  };
  const inbox = new MessageInbox(fake as unknown as Port, () => context, () => {});
  return { inbox, fake, context: () => context!, switch: (value?: InboxContext) => { context = value; inbox.reset(); } };
}

test("release requires reviewed exact content and queues the immutable incoming message", async () => {
  const { inbox, fake } = harness(); await inbox.refresh(); const held = inbox.held[0]!;
  await inbox.decide(held, "release"); assert.equal(fake.posts.length, 0);
  inbox.review(held, true); await inbox.decide(held, "release");
  assert.equal(fake.posts.length, 1); assert.equal(fake.posts[0]!.path, "/sessions/ses_one/inbox/msgx_one/release");
  assert.deepEqual(fake.posts[0]!.options.body, { delivery: "queue" }); assert.ok(fake.posts[0]!.options.key);
  assert.equal(inbox.held.length, 0); assert.equal(inbox.reviewed.size, 0);
});

test("changed sender, target provenance or content invalidates review and stale cards", async () => {
  const { inbox, fake } = harness(); await inbox.refresh(); const old = inbox.held[0]!; inbox.review(old, true);
  fake.messages[0]!.from = "different-agent"; await inbox.refresh();
  assert.equal(inbox.reviewed.size, 0); await inbox.decide(old, "drop"); assert.equal(fake.posts.length, 0);
  const current = inbox.held[0]!; inbox.review(current, true); fake.rows[0]!.parts = [{ type: "text", text: "changed content" }]; await inbox.refresh();
  assert.equal(inbox.reviewed.size, 0); await inbox.decide(current, "release"); assert.equal(fake.posts.length, 0);
});

test("uncertain uncommitted decision retries the original request identity and payload", async () => {
  const { inbox, fake } = harness(); await inbox.refresh(); const item = inbox.held[0]!; inbox.review(item, true); fake.fail = true;
  await inbox.decide(item, "release"); assert.equal(inbox.decision?.status, "uncertain");
  await inbox.refresh(); await inbox.retry();
  assert.equal(fake.posts.length, 2); assert.equal(fake.posts[0]!.options.key, fake.posts[1]!.options.key);
  assert.deepEqual(fake.posts[0]!.options.body, fake.posts[1]!.options.body); assert.equal(fake.posts[0]!.path, fake.posts[1]!.path);
});

test("lost committed acknowledgement reconciles without another release", async () => {
  const { inbox, fake } = harness(); await inbox.refresh(); const item = inbox.held[0]!; inbox.review(item, true); fake.fail = true; fake.commit = true;
  await inbox.decide(item, "release"); assert.equal(inbox.decision?.status, "uncertain");
  await inbox.refresh(); await inbox.retry();
  assert.equal(fake.posts.length, 1); assert.equal(inbox.decision, undefined); assert.equal(inbox.held.length, 0);
});

test("offline and missing-provenance inputs cannot be acted on", async () => {
  const h = harness(); await h.inbox.refresh(); const item = h.inbox.held[0]!; h.inbox.review(item, true); h.context().actionable = false;
  await h.inbox.decide(item, "release"); assert.equal(h.fake.posts.length, 0); assert.equal(h.inbox.decision, undefined);
  h.fake.messages[0]!.origin_kind = "collaboration"; await h.inbox.refresh(); assert.equal(h.inbox.held.length, 0);
  assert.equal(h.inbox.pendingMetadata, 1);
});

test("authorization refusal never becomes release success and clears the settled command", async () => {
  const { inbox, fake } = harness(); await inbox.refresh(); const item = inbox.held[0]!; inbox.review(item, true); fake.deny = true;
  await inbox.decide(item, "release"); assert.equal(fake.posts.length, 1); assert.equal(inbox.decision, undefined);
  assert.match(inbox.notice, /refused/); assert.equal(inbox.held.length, 1);
  assert.equal(inbox.reviewed.size, 0);
});

test("failed reads clear stale sender content and consent", async () => {
  const { inbox, fake } = harness(); await inbox.refresh(); inbox.review(inbox.held[0]!, true);
  fake.hook = () => Promise.reject(new ApiFault("refused", 403)); await inbox.refresh();
  assert.equal(inbox.held.length, 0); assert.equal(inbox.reviewed.size, 0); assert.match(inbox.notice, /refused/);
  fake.hook = undefined; await inbox.refresh(); assert.equal(inbox.held.length, 1); assert.equal(inbox.notice, "");
});

test("late read after Session switch or logout cannot restore previous content", async () => {
  const h = harness(); let finish!: (value: unknown) => void;
  h.fake.hook = (path) => path.endsWith("/inbox") ? new Promise((resolve) => { finish = resolve; }) : undefined;
  const pending = h.inbox.refresh(); h.switch(); finish({ data: [row()] }); await pending;
  assert.equal(h.inbox.rows.length, 0); assert.equal(h.inbox.messages.size, 0); assert.equal(h.inbox.loading, false);
});

test("loading older provenance retains reviewed held content and resolves older cards", async () => {
  const { inbox, fake } = harness(); fake.rows.push(row("msgx_old"));
  fake.hook = (path) => path.includes("/messaging?") ? Promise.resolve(path.includes("cursor=") ? { data: [message("msgx_old")], cursor: {} } : { data: [message()], cursor: { next: "older" } }) : undefined;
  await inbox.refresh(); assert.equal(inbox.held.length, 1); assert.equal(inbox.pendingMetadata, 1);
  const first = inbox.held[0]!; inbox.review(first, true); await inbox.refresh(true);
  assert.equal(inbox.held.length, 2); assert.equal(inbox.reviewed.get(first.row.message_id), first.identity); assert.equal(inbox.cursor, undefined);
  await inbox.refresh(); assert.equal(inbox.cursor, undefined, "a routine refresh must not reset completed provenance pagination");
});

test("dropping names only the selected immutable message and never releases it", async () => {
  const { inbox, fake } = harness(); await inbox.refresh(); await inbox.decide(inbox.held[0]!, "drop");
  assert.equal(fake.posts.length, 1); assert.equal(fake.posts[0]!.path, "/sessions/ses_one/inbox/msgx_one/drop"); assert.equal(fake.posts[0]!.options.body, undefined);
});

test("collaboration provenance is retained and a changed membership generation invalidates consent", async () => {
  const { inbox, fake } = harness();
  fake.messages[0]!.origin_kind = "collaboration";
  fake.messages[0]!.collaboration = { hub_id: "hub_one", project_id: "cprj_one", member_id: "cmem_one", agent_id: "cagt_one", member_generation: 1, membership_kind: "guest", remote_message_id: "cmsg_one", target_agent_id: "cagt_receiver" };
  await inbox.refresh(); const item = inbox.held[0]!; inbox.review(item, true);
  assert.equal(item.message.collaboration!.member_id, "cmem_one");
  fake.messages[0]!.collaboration!.member_generation = 2; await inbox.refresh();
  assert.equal(inbox.reviewed.size, 0); await inbox.decide(item, "release"); assert.equal(fake.posts.length, 0);
});
