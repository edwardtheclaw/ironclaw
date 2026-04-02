-- Audit log for inbound webhook deliveries.
-- Records timestamp, channel, HMAC result, payload hash, and user_id.

CREATE TABLE IF NOT EXISTS webhook_events (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    received_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    channel     TEXT        NOT NULL,
    hmac_valid  BOOLEAN,
    payload_hash TEXT       NOT NULL,
    user_id     TEXT        NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_webhook_events_user_received
    ON webhook_events(user_id, received_at DESC);
