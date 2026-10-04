## 1. Workspace

- [x] 1.1 Cargo workspace with six crates, shared lints (unsafe denied, clippy all, cognitive complexity above 15 denied).
- [x] 1.2 Build information (version, channel, git SHA, target) and `cyber --version`.

## 2. Foundations

- [x] 2.1 XDG paths with `CYBER_HOME`, specific-variable precedence, derived directories and channel database names.
- [x] 2.2 JSONC parsing, layering, substitution, profiles, `--config` overrides, validation and redaction.
- [x] 2.3 Trust gating, trust store and `cyber trust`.

## 3. Store

- [x] 3.1 Durability pragmas, migrations, newer-database read-only open, network-filesystem rejection.
- [x] 3.2 Event store, registry, projectors, upcasters, bounded writer, fail-stop, tail, ownership lock, read-only query.
- [x] 3.3 Process-kill, disk-full, contention and gapless replay tests.

## 4. Evaluation and CLI

- [x] 4.1 Deterministic coding manifest with fixture and hidden grader; recovery manifest linked to requirements.
- [x] 4.2 `debug`, `db`, `trust` commands; strict parsing; unavailable commands; JSON error envelope.

## 5. Delivery

- [x] 5.1 Spec corrections (`--config` long-only, `autoupdate` key).
- [x] 5.2 CI workflow; README and ROADMAP updates.
