-- Append-only audit log for security-relevant system events.
-- No UPDATE or DELETE should ever be issued on this table.

CREATE TABLE IF NOT EXISTS audit_log (
    id              BIGSERIAL PRIMARY KEY,
    event_id        BIGINT NOT NULL,
    event_type      VARCHAR(64) NOT NULL,
    source_module   VARCHAR(64) NOT NULL,
    source_component VARCHAR(64) NOT NULL,
    category        VARCHAR(32) NOT NULL,
    session_id      UUID,
    thread_id       UUID,
    job_id          UUID,
    user_id         VARCHAR(255),
    payload         JSONB NOT NULL DEFAULT '{}',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Indexes for common query patterns
CREATE INDEX IF NOT EXISTS idx_audit_log_created_at ON audit_log (created_at DESC);
CREATE INDEX IF NOT EXISTS idx_audit_log_job_id ON audit_log (job_id) WHERE job_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_audit_log_session_id ON audit_log (session_id) WHERE session_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_audit_log_user_id ON audit_log (user_id) WHERE user_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_audit_log_event_type ON audit_log (event_type);
