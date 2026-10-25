## Two independent observations

Keep `state` (`running`, `waiting_input`, `idle`, etc.) unchanged. Add an optional nullable boolean `interactiveAttached` to the authorized reachable record: true for a live exact Session/Location interactive lease; false when that Session has managed interactive tracking but no unexpired lease; absent/null for unmanaged/older/unsupported sources. Sample under the presence lock using monotonic deadlines. Never use Location-wide/all-event subscribers, native-support flags, or native grants as proof of an attached client. Return no client IDs, device identities or user metadata.

A registered attachment is a client observation, not a claim that a person is watching. Older clients can fail to report attachment; explain this limitation and use “no client reported” detail for detached labels. An unmanaged historical Session remains attachment-unknown until explicit tracking exists. Do not silently reinterpret a missing field as false.

## TUI lifecycle

Use the existing owner-authenticated interactive-client create/renew/delete routes, bound to the exact Session and canonical Location, independently of native computer support. One lease per TUI; renew every 15 seconds against the existing 45-second TTL. On normal exit, release only that TUI's lease with a bounded request; on switching Session/Location release the old lease and create a new one. Fence asynchronous create/renew replies by client generation, release any late-created old lease, and never reuse an expired receipt. On crash/network loss the lease expires; another client's live lease keeps attachment true. Server restart does not restore leases from disk. Existing durable managed-state semantics and owner authentication remain intact.

This registration participates in existing interactive-attendance semantics, so test approval/native safety explicitly. It grants no tools or computer authority. Preserve the existing deliberate-exit native stop and exact stream safety until regressions demonstrate equivalent fencing; do not remove it as an incidental cleanup.

## Projection and labels

Use the same formatter in `/agents` and the sidebar:

| Attachment | Execution | User-facing meaning |
|---|---|---|
| true | idle | `attached · idle` |
| true | running | `attached · running` |
| true | waiting_input | `attached · waiting` |
| false | running | `background · running` with no-client-reported meaning |
| false | waiting_input | `waiting · no client reported` |
| false | idle | `saved · inactive` with no-client-reported meaning |
| unknown | any | existing execution word plus `attachment unknown` |

Keep peer/hub provenance visible and existing fair six-entry/height-aware bounds. Never turn hub `online` into “attached”; hub membership/agent presence is a different source. Offline peer placeholders remain availability records. When a whole source is stale/unavailable its cached rows must not imply fresh attachment status. No row activation sends or stops anything.

## Compatibility and authority

The field is additive and serde-defaulted. Propagate it only when the authorized peer listing supplies it; absent older-peer fields and hub rows remain unknown. Generated OpenAPI and SDK types stay synchronized; no new endpoint or grant scope is introduced. An attachment lease is never proof of organization membership or permission to message/operate a desktop.

## Acceptance

Test independent activity/attachment axes, multiple attachments, release, expiry, restart, old/missing fields, late replies and Session/Location changes. Verify retained history and continued background work after terminal exit, plus no inherited native authority. Refresh actual dark/drawer/light captures with fixture-managed attachment states and label screenshots as synthetic demo data.
