/**
 * Transport choice (`editor-integration` → Extension server connection): the Unix socket
 * on macOS and Linux when the registration names one, otherwise TCP only to a loopback
 * literal. Anything else is refused before any credential is sent.
 */

export const NOT_LOOPBACK = "Cyber Code server is not on loopback; the VS Code extension supports local servers only";

export interface Registration {
  id?: string;
  version?: string;
  build?: string;
  url?: string;
  socket?: string | null;
  pid?: number;
}

export type Transport =
  | { kind: "socket"; path: string }
  | { kind: "tcp"; url: string }
  | { kind: "refused"; message: string };

const LOOPBACK_HOSTS = new Set(["127.0.0.1", "[::1]", "localhost"]);

export function chooseTransport(registration: Registration, platform: NodeJS.Platform): Transport {
  const socket = registration.socket;
  if ((platform === "darwin" || platform === "linux") && typeof socket === "string" && socket.startsWith("/")) {
    return { kind: "socket", path: socket };
  }
  const url = loopbackUrl(registration.url);
  return url ? { kind: "tcp", url } : { kind: "refused", message: NOT_LOOPBACK };
}

/** `http://127.0.0.1:<port>`, `http://[::1]:<port>` or `http://localhost:<port>`, normalized. */
export function loopbackUrl(value: string | undefined): string | undefined {
  if (!value) return undefined;
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return undefined;
  }
  const plain = url.protocol === "http:" && !url.username && !url.password && url.port !== "";
  const bare = (url.pathname === "/" || url.pathname === "") && !url.search && !url.hash;
  if (!plain || !bare || !LOOPBACK_HOSTS.has(url.hostname)) return undefined;
  return `http://${url.hostname}:${url.port}`;
}
