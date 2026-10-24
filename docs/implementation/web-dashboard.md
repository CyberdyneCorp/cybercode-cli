# Dashboard implementation evidence

Implementation is in progress under `implement-local-web-dashboard`. The first production-browser acceptance pass succeeds on macOS with Playwright 1.64.1 / Chromium 156. Full recovery and cross-platform CI are still running.

Verified locally: browser prompt → real pending permission → approval → completed tool, observed by a second browser and the actual terminal CLI; explicit owner headers, no URL/storage/cookie credentials, hostile transcript content rendered inert, logout/reload authentication, and responsive approval layouts at 360 and 1280 CSS pixels. Workflow pause/resume/stop, Job output/stop, Goal resume/pause/clear, question replies and permission races/retries pass against the real server. The [desktop](../screenshots/web-dashboard-desktop.png) and [mobile](../screenshots/web-dashboard-mobile.png) screenshots contain only isolated fixture data.

Backend checks pass: 70 HTTP integration cases, 10 browser-presence cases, 18 existing request cases, three production-owner WebSocket transport cases (plain, verified TLS and connection limits), TLS path validation and listener-reuse/upgrade binding units. Frontend typecheck/build freshness and 14 state/transport unit cases pass. SDK generation/typecheck and 112 unit cases pass; eight process acceptance cases were skipped without `CYBER_BIN` and will be rerun against the final binary. OpenSpec strict validation passes 87 items; spec lint reports zero errors and 23 warnings. Full checks and CI are pending.

The acceptance suite uses the real CLI/server, an isolated home and a scripted local model. It must cover approval completion visible in two browsers and a terminal/API observer, questions, request races and uncertainty, reconnect and lease expiry, inert untrusted content, credentials remaining in page memory, mobile/desktop layouts, actual TLS and an SSH-style TCP forward.

Physical access from a second device has not been exercised in this headless development environment. Local TLS and forward tests do not replace that acceptance item. CI/platform results and exact browser checks will be appended after execution.
