# Real browser acceptance

Build the production binary and static assets before running these tests:

```sh
npm ci --prefix web
npm --prefix web run build
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=line-tables-only cargo build --locked -p cyber-cli --bins
cd web
npx playwright install chromium --only-shell
npm run test:e2e
```

`CYBER_E2E_BINARY` may select an already-built binary. Every test creates its own temporary home, configuration, repository, local password and actual server process. Its OpenAI-compatible scripted provider runs on loopback and uses no account, provider credential or native credential vault. Git changes and approved tool writes are confined to the temporary checkout. Cleanup terminates fixture processes and removes their files.

Playwright traces/videos are deliberately disabled: network traces would persist the owner Authorization header. Server logs are redacted before attaching them to a failure report. Browser storage and URL credential absence are asserted. Desktop/mobile screenshots contain only fixture data; inspect the generated `test-results/**/desktop-1280-approval.png` and `mobile-360-approval.png` artifacts after the responsive test.

The tests use a real terminal CLI, a second Chromium browser context, actual pending requests and production HTTP/SSE routes. Network interception introduces lost acknowledgements/offline periods; it does not fabricate successful API responses. Two expiry tests wait through the real 45-second lease window. The TCP forward reproduces an SSH local-forward transport without starting an SSH daemon or modifying the machine's network configuration.

Physical second-device/private-network certificate acceptance remains a separate manual check. Headless Chromium and a fixture TLS endpoint do not establish that a physical phone trusts an operator's production certificate.

The integration submission scenario runs both with its original page and after an
immediate navigation away from the response. The assertion uses retained HTTP
status metadata: Chromium can evict the response body on navigation. The navigated
case reconnects through fresh owner attachment and loads the durable queue entry
before continuing the real host verification and no-publication assertions.
