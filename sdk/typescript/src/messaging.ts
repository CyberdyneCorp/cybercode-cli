import type { Groups, RequestOptions } from "./generated/operations.js";
import type { Direction, MessageRecord, Page_MessageRecord } from "./generated/types.js";

/** What `client.messaging.history()` reads (`cross-session-messaging` → Messaging API). */
export interface MessageHistoryFilter {
  /** Only received (`in`) or sent (`out`) records; both without it. */
  direction?: Direction;
  /** By creation time; newest first (`desc`) by default. */
  order?: "asc" | "desc";
  /** Records per page, 1–200 (default 50); further pages are read as the iteration reaches them. */
  limit?: number;
  signal?: AbortSignal;
}

/** What the history helper needs from its client. */
export interface HistoryCaller {
  readonly session: {
    messaging(
      sessionID: string,
      query?: { cursor?: string; limit?: number; direction?: string; order?: string },
      options?: RequestOptions,
    ): Promise<Page_MessageRecord>;
  };
}

/**
 * `client.messaging`: the generated `sessions()`, `send()` and `watch()`, plus `history()`,
 * which yields a Session's sent and received messages across pages.
 */
export type MessagingGroup = Groups["messaging"] & {
  /** A Session's sent and received messages with their outcomes, following `cursor.next` lazily. */
  history(sessionID: string, filter?: MessageHistoryFilter): AsyncIterableIterator<MessageRecord>;
};

export async function* messageHistory(
  client: HistoryCaller,
  sessionID: string,
  filter: MessageHistoryFilter = {},
): AsyncIterableIterator<MessageRecord> {
  const { signal, ...query } = filter;
  let cursor: string | undefined;
  do {
    const page = await client.session.messaging(sessionID, { ...query, ...(cursor !== undefined && { cursor }) }, { signal });
    yield* page.data;
    cursor = page.cursor.next ?? undefined;
  } while (cursor !== undefined);
}

export function messagingGroup(client: HistoryCaller, generated: Groups["messaging"]): MessagingGroup {
  return { ...generated, history: (sessionID, filter) => messageHistory(client, sessionID, filter) };
}
