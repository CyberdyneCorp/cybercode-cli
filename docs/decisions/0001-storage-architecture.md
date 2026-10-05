# Storage architecture

Status: accepted, 2026-10-03. Local persistence and backups are implemented. The initial FULL-durability workload measurements are published in [P0 exit evidence](../measurements/p0-exit-evidence.md).

## Decision

Use SQLite for each local Cyber server and Runner's execution state. Use PostgreSQL for hosted control-plane services when multiple service instances must write shared state. A single-instance self-hosted control plane may use SQLite. Store large attachments, screenshots, overflow output and snapshot objects outside relational rows, with content hashes and references in the database.

The local CLI must work without an account, database installation or network connection. One server already owns execution for an OS user. SQLite fits that ownership model and provides atomic event append plus projection updates, relational queries, migrations and online backup. Multiple concurrent Sessions do not require multiple simultaneous database writers: provider and tool work runs outside short database transactions.

## Alternatives

| Option | Fit | Decision |
|---|---|---|
| SQLite | Embedded local state, one writer at a time, concurrent readers in WAL mode | Default for local runtime and single-instance self-hosting |
| PostgreSQL | Shared service state with concurrent writers, row locking and independently deployed server instances | Hosted multi-instance control plane in P3 |
| Embedded key-value storage such as redb or RocksDB | Could implement an event log, but requires additional indexing, relational projections and operational tooling for this design | No demonstrated advantage for this workload; reconsider only with measurements |
| DuckDB | Analytical queries and exported evaluation/usage datasets | Optional analytics consumer, not the authoritative Session store |
| Plain JSONL files | Human-readable export and diagnostics | Not the transactional source of truth for events plus projections |

SQLite's documentation explicitly identifies embedded application storage as a good fit and many concurrent writers or direct network database access as reasons to prefer a client/server database. PostgreSQL's MVCC and locking facilities fit shared hosted coordination. DuckDB supports analytical concurrency, but adding it does not solve the local runtime's need for atomic operational state transitions more simply than SQLite.

## Local persistence rules

- Use `journal_mode=WAL`, `synchronous=FULL`, foreign keys and a bounded writer queue. Acknowledge admission only after commit. Every writer connection must configure its durability settings.
- Keep network calls, model inference, tools and approval waits outside transactions. Readers page results and release snapshots so they do not indefinitely prevent WAL checkpoints.
- Store the database on a supported local filesystem. Do not place the live database on NFS/SMB or copy only its main file while WAL is active. Clients connect to Cyber's API.
- FULL durability assumes the OS, filesystem and storage device honor synchronization. It does not protect against lost disks, hardware that lies about flushes, or missing backups. Live stream fragments are not durable until their completion event commits.
- A committed tool intent does not prove that an external effect happened exactly once. Recovery uses unknown outcomes, stable operation keys and reconciliation.
- Database-only backups are explicitly incomplete for artifact recovery. Full bundles pin and hash referenced artifacts; restore validates before replacing existing data.
- Stop new mutation dispatch on storage failure. Never switch silently to ephemeral state.

## Hosted boundary

PostgreSQL stores shared ownership epochs, leases, scheduling, quotas and control-plane metadata. Local execution history remains on its owning runtime unless the user authorizes transfer or sharing. Transfer uses a versioned manifest and ownership protocol, not shared SQLite files or database-file replication.

Keep transactional operations behind focused repository interfaces: append with expected sequence, idempotent admission, apply projection, reserve budget and claim execution ownership. Avoid a generic database abstraction or implementing a second local backend in P0. Backend-specific locking and migration details remain in adapters. When PostgreSQL is introduced, run the same behavioral contract suite against both supported control-plane backends.

## Evidence required before reconsidering

Benchmark representative event payloads, indexes and replay queries with FULL durability. Start with 1, 8 and 32 concurrently active Sessions, each producing 10 durable events per second, while clients replay history. Report p50/p95/p99 admission and commit latency, writer queue depth, WAL growth, disk usage and backup duration on named hardware. These are workload probes, not a claim of supported capacity.

Use a provisional p95 admission target of 100 ms on the published reference machine at 8 Sessions. If the sustained workload misses it, profile transaction duration, indexes, batching and slow readers first. Choose PostgreSQL when the deployment requires multiple active writer hosts or measured contention persists at the required load. Do not switch based on Session count alone.

## Sources

- [SQLite: appropriate uses](https://www.sqlite.org/whentouse.html)
- [SQLite: WAL](https://www.sqlite.org/wal.html)
- [SQLite: synchronous pragma](https://www.sqlite.org/pragma.html#pragma_synchronous)
- [SQLite: online backup](https://www.sqlite.org/backup.html)
- [PostgreSQL: concurrency control](https://www.postgresql.org/docs/current/mvcc-intro.html)
- [DuckDB: concurrency](https://duckdb.org/docs/current/connect/concurrency)
