import { createHash } from "node:crypto";
import type { CyberClient } from "./client.js";
import type { OpenedDocument, SearchResult } from "./generated/types.js";
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
  close(options?: RequestOptions): Promise<void>;
}

/** Open a Session-owned live view; no eager copy, edit preparation, or inference. */
export async function openBinaryDocument(
  client: CyberClient,
  sessionId: string,
  path: string,
  options: RequestOptions & { expectedRoot?: string } = {},
): Promise<BinaryDocument> {
  const { expectedRoot, ...requestOptions } = options;
  const openedResponse = await client.request("v1.fs.binaryOpen", {
    body: { session_id: sessionId, path, expected_root: expectedRoot },
  }, requestOptions);
  const opened = openedResponse.data;
  byteOffset(opened.length);
  const { document_id: id, ...fields } = opened;
  const metadata = Object.freeze(fields);
  const scope = { id };
  const query = { session_id: sessionId };
  return {
    id,
    metadata,
    async bytes(offset, limit = 64 * 1024, options) {
      if (!Number.isSafeInteger(limit) || limit < 1 || limit > MAX_PAGE) {
        throw new RangeError("Byte page limit must be 1–1048576");
      }
      const at = byteOffset(offset);
      const response = await client.request("v1.fs.binaryBytes", {
        path: scope, query: { ...query, revision: metadata.revision, offset: at, limit },
      }, options);
      const page = response.data;
      if (page.revision !== metadata.revision || page.offset !== at || page.source !== "live") {
        throw new Error("Stale binary page");
      }
      if (page.base64.length > Math.ceil(limit / 3) * 4) throw new RangeError("Binary response exceeds page limit");
      const bytes = Buffer.from(page.base64, "base64");
      if (bytes.length > limit || byteOffset(page.length) !== String(bytes.length) || bytes.toString("base64") !== page.base64
        || BigInt(at) + BigInt(bytes.length) > BigInt(metadata.length)) {
        throw new Error("Invalid binary page encoding or length");
      }
      if (createHash("sha256").update(bytes).digest("hex") !== page.page_sha256) throw new Error("Binary page fingerprint mismatch");
      return new Uint8Array(bytes);
    },
    async search(hex, offset = 0n, options) {
      if (!/^(?:[0-9a-fA-F]{2}){1,4096}$/.test(hex)) throw new RangeError("Search requires 1–4096 literal bytes encoded as hex");
      const response = await client.request("v1.fs.binarySearch", {
        path: scope,
        body: { session_id: sessionId, revision: metadata.revision, hex, offset: byteOffset(offset) },
      }, options);
      const result = response.data;
      if (result.revision !== metadata.revision || result.source !== "live" || result.offsets.length > 100) throw new Error("Invalid binary search response");
      result.offsets.forEach(byteOffset);
      if (result.next_offset !== null && result.next_offset !== undefined) byteOffset(result.next_offset);
      byteOffset(result.scanned);
      return result;
    },
    async close(options) {
      await client.request("v1.fs.binaryClose", { path: scope, query }, options);
    },
  };
}
