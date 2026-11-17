-- Compatibility marker: older binaries do not understand snapshot-worker
-- admission fences or origin-bound acknowledgements. The existing unknown-
-- migration gate must keep those binaries read-only after this upgrade.
SELECT 1;
