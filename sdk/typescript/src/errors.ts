import type { ErrorBody } from "./generated/types.js";

/** The server answered with a tagged error body (`{ _tag, message, ... }`). */
export class CyberApiError extends Error {
  override readonly name = "CyberApiError";
  /** The server `_tag`, e.g. `SessionBusyError`. */
  readonly tag: string;
  readonly status: number;
  readonly body: ErrorBody;

  constructor(status: number, body: ErrorBody) {
    super(`${body._tag}: ${body.message}`);
    this.tag = body._tag;
    this.status = status;
    this.body = body;
  }
}

export type ClientErrorReason = "Transport" | "UnexpectedStatus" | "MalformedResponse" | "Timeout";

/** The request did not produce a usable server answer. */
export class CyberClientError extends Error {
  override readonly name = "CyberClientError";
  readonly reason: ClientErrorReason;
  /** The HTTP status for `UnexpectedStatus`. */
  readonly status: number | undefined;
  /** Retrying cannot help (e.g. the embedded server exited); streams stop instead of reconnecting. */
  readonly permanent: boolean;

  constructor(reason: ClientErrorReason, message: string, options: { cause?: unknown; status?: number; permanent?: boolean } = {}) {
    super(message, { cause: options.cause });
    this.reason = reason;
    this.status = options.status;
    this.permanent = options.permanent ?? false;
  }
}

/** Whether a stream failure is worth reconnecting for: transport problems and 5xx answers. */
export function isRetryable(err: unknown): boolean {
  if (err instanceof CyberApiError) return err.status >= 500;
  if (err instanceof CyberClientError) {
    if (err.permanent) return false;
    return err.reason === "Transport" || err.reason === "Timeout" || (err.status !== undefined && err.status >= 500);
  }
  // Anything else (a reset socket while reading a body) is a transport failure.
  return true;
}

/** Map a non-2xx response body to the error it describes. */
export function errorFromResponse(status: number, text: string): CyberApiError | CyberClientError {
  try {
    const body = JSON.parse(text) as Partial<ErrorBody> | null;
    if (body && typeof body._tag === "string") {
      return new CyberApiError(status, { ...body, _tag: body._tag, message: String(body.message ?? "") });
    }
  } catch {
    // Not JSON; fall through.
  }
  return new CyberClientError("UnexpectedStatus", `unexpected HTTP ${status}`, { status });
}
