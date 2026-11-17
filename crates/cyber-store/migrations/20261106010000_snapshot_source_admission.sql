-- Compatibility marker: source-admission phases and pre-effect cancellation
-- receipts must not be interpreted by older snapshot writers. Preserve all
-- existing evidence and use the unknown-migration gate to require an upgrade.
SELECT 1;
