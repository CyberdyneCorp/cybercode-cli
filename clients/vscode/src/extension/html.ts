/**
 * The chat webview's HTML shell (`editor-integration` → Extension webview isolation): a
 * per-render nonce Content Security Policy with no `connect-src`, so the webview cannot
 * fetch, open sockets or load remote content.
 */
import { randomBytes } from "node:crypto";

export function nonce(): string {
  return randomBytes(18).toString("base64url");
}

export function csp(cspSource: string, value: string): string {
  return [
    "default-src 'none'",
    `script-src 'nonce-${value}'`,
    `style-src ${cspSource} 'nonce-${value}'`,
    `img-src ${cspSource} data:`,
    `font-src ${cspSource}`,
  ].join("; ");
}

export function html(options: { cspSource: string; script: string; style: string; nonce: string }): string {
  const policy = csp(options.cspSource, options.nonce);
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="${policy}">
<meta name="viewport" content="width=device-width, initial-scale=1">
<link rel="stylesheet" href="${options.style}" nonce="${options.nonce}">
<title>Cyber Code</title>
</head>
<body>
<main id="app" aria-live="polite"></main>
<script nonce="${options.nonce}" src="${options.script}"></script>
</body>
</html>`;
}
