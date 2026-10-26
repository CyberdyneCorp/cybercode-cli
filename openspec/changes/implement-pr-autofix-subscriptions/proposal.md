# PR auto-fix subscriptions

## Why

A developer currently has to notice failed CI or review feedback, return to a Session, and ask for another fix. The P3 PR auto-fix requirement exists, but `cyber pr` only checks out a PR and resumes a linked Session.

## What changes

Implement an opt-in durable subscription on a reachable self-hosted Cyber server. Poll GitHub for completed failing checks and authorized review feedback, queue immutable triggers, run isolated bounded fix attempts, verify changes, and push only to the subscribed same-repository PR branch. Expose enable, status, pause, resume and disable in the CLI, authenticated API, SDK and TUI command surface. Keep attempt evidence after restart and stop on closed PRs or exhausted attempts.

Use existing Session admission, sandbox, permissions and workflow budget controls. Missing strict budget support must block configurations advertised as hard caps. Review text, logs and repository files remain untrusted input.

## Scope and dependencies

First release supports same-repository GitHub PRs and a running self-hosted server using an owner-configured GitHub credential. Fork writes, hosted Runner failover, GitHub App provisioning, public inbound webhooks and general Channel infrastructure remain deferred. An offline server does not execute work; restart catches up against the current PR head. This implements a subset of the existing target requirement and must be documented as such.

No automatic merge, force push, privileged workflow execution or repository configuration edits to suppress verification. No subscription starts before explicit owner opt-in. Specification review is pending; implementation has not started.
