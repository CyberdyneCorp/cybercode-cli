import { createHash } from "node:crypto";
import type { CyberClient } from "./client.js";
import type { OpenedDocument, SearchResult, BinaryStatus, BinaryEdited, BinarySplice, BinarySaveReview, BinaryOperation, BinaryHistoryAction, BinaryVersions, BinaryReview } from "./generated/types.js";
import type { RequestOptions } from "./generated/operations.js";

const MAX_OFFSET = (1n << 64n) - 1n;
const MAX_PAGE = 1024 * 1024;

/** Canonical wire offset. Numbers are deliberately excluded to prevent rounded offsets. */
export function byteOffset(value: bigint | string): string {
  if (typeof value !== "bigint" && typeof value !== "string") throw new TypeError("Use bigint or a decimal string for byte offsets");
  const text = String(value);
  if (!/^(0|[1-9][0-9]{0,19})$/.test(text) || BigInt(text) > MAX_OFFSET) {
    throw new RangeError("Expected a canonical unsigned decimal byte offset within u64");
  }
  return text;
}

export interface BinaryDocument {
  readonly id: string;
  /** A live observed generation, not an immutable whole-file digest. */
  readonly metadata: Omit<OpenedDocument, "document_id">;
  bytes(offset: bigint | string, limit?: number, options?: RequestOptions): Promise<Uint8Array>;
  search(hex: string, offset?: bigint | string, options?: RequestOptions): Promise<SearchResult>;
  status(options?: RequestOptions): Promise<BinaryStatus>;
  versions(after?: string, limit?: number, options?: RequestOptions): Promise<BinaryVersions>;
  versionReview(documentId: string, versionId: string, start?: number, limit?: number, options?: RequestOptions): Promise<BinaryReview>;
  prepare(options?: RequestOptions): Promise<BinaryStatus>;
  patch(operationId: string, batch: { revision: string; splices: BinarySplice[] }, options?: RequestOptions): Promise<BinaryEdited>;
  history(operationId: string, command: { revision: string; action: BinaryHistoryAction }, options?: RequestOptions): Promise<BinaryEdited>;
  review(start?: number, limit?: number, options?: RequestOptions): Promise<BinarySaveReview>;
  save(operationId: string, review: string, options?: RequestOptions): Promise<BinaryEdited>;
  restoreReview(options?: RequestOptions): Promise<BinarySaveReview>;
  restore(operationId: string, review: string, options?: RequestOptions): Promise<BinaryEdited>;
  reconcile(options?: RequestOptions): Promise<BinaryEdited>;
  operation(operationId: string, options?: RequestOptions): Promise<BinaryOperation | null>;
  close(options?: RequestOptions): Promise<void>;
}

/** Open a Session-owned live view; no eager copy, edit preparation, or inference. */
export async function openBinaryDocument(
  client: CyberClient,
  sessionId: string,
  path: string,
  options: RequestOptions & { expectedRoot?: string; readOnly?: boolean } = {},
): Promise<BinaryDocument> {
  const { expectedRoot, readOnly = false, ...requestOptions } = options;
  const openedResponse = await client.request("v1.fs.binaryOpen", {
    body: { session_id: sessionId, path, expected_root: expectedRoot, read_only: readOnly },
  }, requestOptions);
  const opened = openedResponse.data;
  byteOffset(opened.length);
  const { document_id: id, ...fields } = opened;
  return binaryHandle(client, sessionId, id, fields);
}

function metadataFields(status: BinaryStatus): Omit<OpenedDocument, "document_id"> {
  return { path: status.path, length: status.length, revision: status.revision, source: status.source, within_edit_size_limit: status.within_edit_size_limit };
}

/** Reopen retained commands and receipts. Preparing edit authority remains explicit. */
export async function recoverBinaryDocument(client: CyberClient, sessionId: string, id: string,
  options: RequestOptions & { readOnly?: boolean } = {}): Promise<BinaryDocument> {
  const { readOnly = false, ...requestOptions } = options;
  const response = await client.request("v1.fs.binaryRecover", { path: { id }, body: { session_id: sessionId, read_only: readOnly } }, requestOptions);
  return binaryHandle(client, sessionId, id, metadataFields(response.data));
}

function binaryHandle(client: CyberClient, sessionId: string, id: string, fields: Omit<OpenedDocument, "document_id">): BinaryDocument {
  let metadata = Object.freeze(fields);
  let generation = 0;
  const update = (status: BinaryStatus, expected: number): BinaryStatus => {
    if (generation !== expected) throw new Error("Binary response superseded; refresh status before continuing");
    byteOffset(status.length);
    if (status.path !== metadata.path || !["live", "draft"].includes(status.source)) throw new Error("Invalid binary status");
    metadata = Object.freeze(metadataFields(status));
    return status;
  };
  const scope = { id };
  const query = { session_id: sessionId };
  return {
    id,
    get metadata() { return metadata; },
    async bytes(offset, limit = 64 * 1024, options) {
      if (!Number.isSafeInteger(limit) || limit < 1 || limit > MAX_PAGE) {
        throw new RangeError("Byte page limit must be 1–1048576");
      }
      const snapshot = metadata;
      const expected = generation;
      const at = byteOffset(offset);
      const response = await client.request("v1.fs.binaryBytes", {
        path: scope, query: { ...query, revision: snapshot.revision, offset: at, limit },
      }, options);
      const page = response.data;
      if (generation !== expected || page.revision !== snapshot.revision || page.offset !== at || page.source !== snapshot.source) {
        throw new Error("Stale binary page");
      }
      if (page.base64.length > Math.ceil(limit / 3) * 4) throw new RangeError("Binary response exceeds page limit");
      const bytes = Buffer.from(page.base64, "base64");
      if (bytes.length > limit || byteOffset(page.length) !== String(bytes.length) || bytes.toString("base64") !== page.base64
        || BigInt(at) + BigInt(bytes.length) > BigInt(snapshot.length)) {
        throw new Error("Invalid binary page encoding or length");
      }
      if (createHash("sha256").update(bytes).digest("hex") !== page.page_sha256) throw new Error("Binary page fingerprint mismatch");
      return new Uint8Array(bytes);
    },
    async search(hex, offset = 0n, options) {
      if (!/^(?:[0-9a-fA-F]{2}){1,4096}$/.test(hex)) throw new RangeError("Search requires 1–4096 literal bytes encoded as hex");
      const snapshot = metadata;
      const expected = generation;
      const response = await client.request("v1.fs.binarySearch", {
        path: scope,
        body: { session_id: sessionId, revision: snapshot.revision, hex, offset: byteOffset(offset) },
      }, options);
      const result = response.data;
      if (generation !== expected || result.revision !== snapshot.revision || result.source !== snapshot.source || result.offsets.length > 100) throw new Error("Invalid binary search response");
      result.offsets.forEach(byteOffset);
      if (result.next_offset !== null && result.next_offset !== undefined) byteOffset(result.next_offset);
      byteOffset(result.scanned);
      return result;
    },
    async versions(after, limit = 64, options) {
      if (!Number.isSafeInteger(limit) || limit < 1 || limit > 64) throw new RangeError("Invalid binary history page");
      const expected = generation;
      const response = await client.request("v1.fs.binaryVersions", { path: scope, query: { ...query, after, limit } }, options);
      if (generation !== expected) throw new Error("Binary history response superseded");
      for (const version of response.data.versions) {
        if (version.path !== metadata.path) throw new Error("Binary history source changed");
        byteOffset(version.before_length); byteOffset(version.after_length);
        byteOffset(version.original_bytes); byteOffset(version.proposed_bytes);
      }
      return response.data;
    },
    async versionReview(documentId, versionId, start = 0, limit = 128, options) {
      if (!/^bin_[a-zA-Z0-9_]{1,96}$/.test(documentId) || !/^ver_[a-f0-9]{64}$/.test(versionId)) throw new TypeError("Invalid saved binary identity");
      if (!Number.isSafeInteger(start) || start < 0 || !Number.isSafeInteger(limit) || limit < 1 || limit > 128) throw new RangeError("Invalid binary review page");
      const expected = generation;
      const response = await client.request("v1.fs.binaryVersionReview", { path: { ...scope, document: documentId, version: versionId }, query: { ...query, start, limit } }, options);
      if (generation !== expected) throw new Error("Saved binary review superseded");
      return response.data;
    },
    async status(options) {
      const expected = ++generation;
      const response = await client.request("v1.fs.binaryStatus", { path: scope, query }, options);
      return update(response.data, expected);
    },
    async prepare(options) {
      const expected = ++generation;
      const response = await client.request("v1.fs.binaryPrepare", { path: scope, body: { session_id: sessionId, revision: metadata.revision } }, options);
      return update(response.data, expected);
    },
    async patch(operationId, batch, options) {
      operationID(operationId);
      validateSplices(batch.splices);
      const expected = ++generation;
      const response = await client.request("v1.fs.binaryPatch", { path: scope, body: { session_id: sessionId, operation_id: operationId, revision: batch.revision, splices: batch.splices } }, options);
      update(response.data.status, expected);
      return response.data;
    },
    async history(operationId, command, options) {
      operationID(operationId);
      if (!["undo", "redo", "discard"].includes(command.action)) throw new TypeError("Invalid binary history action");
      const expected = ++generation;
      const response = await client.request("v1.fs.binaryHistory", { path: scope, body: { session_id: sessionId, operation_id: operationId, revision: command.revision, action: command.action } }, options);
      update(response.data.status, expected);
      return response.data;
    },
    async review(start = 0, limit = 128, options) {
      if (!Number.isSafeInteger(start) || start < 0 || !Number.isSafeInteger(limit) || limit < 1 || limit > 128) throw new RangeError("Invalid binary review page");
      const expected = generation;
      const revision = metadata.revision;
      const response = await client.request("v1.fs.binaryReview", { path: scope, body: { session_id: sessionId, revision, start, limit } }, options);
      if (generation !== expected || response.data.draft.revision !== revision) throw new Error("Stale binary review");
      return response.data;
    },
    async operation(operationId, options) {
      operationID(operationId);
      const response = await client.request("v1.fs.binaryOperation", { path: { ...scope, operation: operationId }, query }, options);
      return response.data ?? null;
    },
    async save(operationId, review, options) {
      operationID(operationId);
      const expected = ++generation;
      const response = await client.request("v1.fs.binarySave", { path: scope, body: { session_id: sessionId, operation_id: operationId, review } }, options);
      update(response.data.status, expected);
      return response.data;
    },
    async restoreReview(options) {
      const expected = generation;
      const revision = metadata.revision;
      const response = await client.request("v1.fs.binaryRestoreReview", { path: scope, body: { session_id: sessionId, revision } }, options);
      if (generation !== expected || response.data.draft.revision !== revision) throw new Error("Stale binary restore review");
      return response.data;
    },
    async restore(operationId, review, options) {
      operationID(operationId);
      const expected = ++generation;
      const response = await client.request("v1.fs.binaryRestore", { path: scope, body: { session_id: sessionId, operation_id: operationId, review } }, options);
      update(response.data.status, expected);
      return response.data;
    },
    async reconcile(options) {
      const expected = ++generation;
      const response = await client.request("v1.fs.binaryReconcile", { path: scope, body: { session_id: sessionId } }, options);
      update(response.data.status, expected);
      return response.data;
    },
    async close(options) {
      await client.request("v1.fs.binaryClose", { path: scope, query }, options);
    },
  };
}

function operationID(id: string): void {
  if (typeof id !== "string" || !/^[a-zA-Z0-9_-]{1,100}$/.test(id)) throw new TypeError("Use a stable binary operation ID of 1–100 letters, digits, underscores or hyphens");
}
function validateSplices(splices: BinarySplice[]): void {
  if (!Array.isArray(splices) || splices.length < 1 || splices.length > 128) throw new RangeError("Binary patches require 1–128 splices");
  let total = 0;
  for (const splice of splices) {
    byteOffset(splice.offset);
    const deleted = BigInt(byteOffset(splice.delete_length));
    for (const hex of [splice.expected_hex, splice.insert_hex]) {
      if (typeof hex !== "string" || !/^(?:[0-9a-fA-F]{2})*$/.test(hex)) throw new TypeError("Binary splice bytes must be paired hexadecimal digits");
      total += hex.length / 2;
      if (total > 64 * 1024) throw new RangeError("Binary patch byte budget exceeded");
    }
    if (BigInt(splice.expected_hex.length / 2) !== deleted) throw new RangeError("Expected bytes must match delete_length exactly");
  }
}
