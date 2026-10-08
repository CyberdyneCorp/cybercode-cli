## Context

Project executable configuration is currently withheld until checkout trust is approved. Configuration merges and validates before building runtime consumers. Hooks have a canonical target specification but no execution implementation.

## Decisions

- Start with one typed hook configuration model shared by validation and future execution. Validate known events, four P1 handler types, required handler fields, timeout/control types and selector syntax. Configuration validation performs no effects.
- Keep untrusted project/local definitions withheld before interpretation. Checkout trust is not a substitute for the required individual handler-digest approvals.
- Store handler approvals separately from workspace configuration approvals in the same private trust file. Hash canonical effective handler definitions (including defaults and all execution options); invocation approvals retain only inspected digests in memory. Serialize all shared-file mutations with a native sidecar lock, and checkout revocation clears both approval classes. Runtime dispatch must still recheck trust before effects.
- Add ordered scope contributions with per-definition provenance before dispatch; ordinary last-value replacement must not discard earlier hooks.
- Accumulate event arrays inside named profiles as well as top-level hooks. On profile selection, remap indexed hook origins into their new appended positions; ordinary profile settings retain their existing profile labels. Source paths remain necessary for later scope classification and individual handler trust.
- Runtime admission owns prompt hooks; the tool host owns tool hooks and revalidation. Permission denies and protected ceilings remain independent of hook allows. Every execution needs a durable receipt and cancellation owner.
- Construct immutable envelopes from the owning runtime's captured Session/Location/agent/Mode identity and Unix-millisecond timestamp. Event-specific data cannot replace any common envelope field. Chained tool rewrites clone only tool input, preserving identity; core shape validation is not proof of runtime authority or ownership.
- Compile selectors at catalog construction and keep matching side-effect free. Decision parsing reports ignored fields separately for later debug logging. Declared-order merging retains restrictive decisions, rewrite/context accumulation and stop flags; the future dispatcher must feed each rewritten input to the next handler and the tool host must revalidate it before effects.
- Implement process-tree ownership, sandbox selection and bounded IO before command execution. HTTP, evaluator and MCP handlers share the event and decision model while retaining their own authentication and usage boundaries.
- Command transport accepts an already authorized process under the caller's retained execution owner. Feed JSON stdin and drain stdout/stderr concurrently, retaining at most 1 MiB per output stream while draining overflow. Explicit stop attempts terminate the owned tree and wait at most two seconds for acknowledgement; missing acknowledgement remains explicit. The stdin-preserving container adapter is separate from shell tools' EOF adapter. Capture remains transient and is not automatically persisted; runtime receipts must honor hook IO logging policy.
- Deliver the remaining lifecycle events, trust/viewer/test CLI, plugin process protocol/package and MCP OAuth/search as subsequent increments under this same change.

## Validation

Real configuration-loading tests cover rejection paths, all four handler definitions, defaults, invalid selectors, trust withholding and changed definitions. Later increments require actual dispatch/side effects, denial/rewrite/merge semantics, durability, client surfaces and native ownership/cancellation evidence. Passing configuration tests alone cannot accept hooks or M1.3.
