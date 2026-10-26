-- Retain request identity across partial receipts and late billing reconciliation.
-- No FK: usage and reservation evidence survive Session/transcript deletion.
ALTER TABLE usage_step ADD COLUMN reservation_id TEXT;
CREATE INDEX usage_step_reservation ON usage_step(reservation_id) WHERE reservation_id IS NOT NULL;
