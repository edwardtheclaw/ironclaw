# IronClaw Incident Response Runbook

## Triage Matrix

| Severity | Definition | Initial Response | Escalation Window |
|----------|------------|-----------------|-------------------|
| **SEV-1** | Full service outage, data loss risk, security breach, credential compromise | Immediate — page on-call | 15 minutes |
| **SEV-2** | Partial outage, primary LLM circuit open, database connection failure, container escapes | Within 30 minutes | 1 hour |
| **SEV-3** | Degraded performance, SSE/WebSocket disruption, stuck jobs, migration warning | Within 2 hours | 4 hours |
| **SEV-4** | Non-critical errors, log noise, single job failure, slow LLM responses | Next business day | N/A |

**First responder checklist:**

1. Check service health: `ironclaw status` or `systemctl --user status ironclaw`
2. Tail logs: `journalctl --user -u ironclaw -f` (Linux) or `log stream --predicate 'subsystem == "com.ironclaw.daemon"'` (macOS)
3. Determine blast radius: is the outage affecting all users or one session?
4. Identify the subsystem: DB, LLM, sandbox, secrets, SSE gateway
5. Assign severity level and begin the appropriate section below

---

## Database Incidents

### libSQL / Turso Connection Failure

**Symptoms:** Log lines containing `libSQL` + `Connection failed` or `busy_timeout`; jobs stuck in `InProgress`.

**Diagnosis:**

```bash
# Check if local libSQL file is accessible
ls -lh ~/.ironclaw/*.db

# Test Turso connectivity (if using remote)
curl -sf "https://api.turso.io/v1/organizations" \
  -H "Authorization: Bearer $TURSO_AUTH_TOKEN" | jq .

# Verify LIBSQL_URL and LIBSQL_AUTH_TOKEN are set
printenv | grep -E "LIBSQL|DATABASE"
```

**Recovery:**

1. If using local file mode and the file is locked, identify which process holds the lock:
   ```bash
   fuser ~/.ironclaw/ironclaw.db
   ```
2. If no legitimate process holds the lock (stale lock from crashed process), remove it:
   ```bash
   rm -f ~/.ironclaw/ironclaw.db-shm ~/.ironclaw/ironclaw.db-wal
   ```
3. If using Turso remote and the auth token expired, rotate via `turso auth token` and update `LIBSQL_AUTH_TOKEN`.
4. Restart the service: `systemctl --user restart ironclaw` or `launchctl kickstart -k gui/$(id -u)/com.ironclaw.daemon`

### PostgreSQL Connection Failure

**Symptoms:** `deadpool_postgres` pool exhaustion errors; `DATABASE_URL connection refused`.

**Diagnosis:**

```bash
# Check PostgreSQL is running
pg_isready -h "$POSTGRES_HOST" -p "${POSTGRES_PORT:-5432}"

# Check connection pool usage
psql "$DATABASE_URL" -c "SELECT count(*), state FROM pg_stat_activity WHERE datname = current_database() GROUP BY state;"

# Look for long-running queries blocking connections
psql "$DATABASE_URL" -c "SELECT pid, now() - pg_stat_activity.query_start AS duration, query, state FROM pg_stat_activity WHERE (now() - pg_stat_activity.query_start) > interval '5 minutes';"
```

**Recovery:**

1. Kill blocking queries if safe:
   ```bash
   psql "$DATABASE_URL" -c "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE (now() - query_start) > interval '10 minutes' AND state != 'idle';"
   ```
2. If the connection pool is exhausted, restart IronClaw to reset pool state.
3. If PostgreSQL is down, switch to libSQL backend by setting `DATABASE_BACKEND=libsql` and `LIBSQL_URL=file:~/.ironclaw/ironclaw.db`.

### Database Corruption

**Symptoms:** `DecryptionFailed`, `unparseable timestamp`, `row not found` after successful insert.

**libSQL / SQLite:**

```bash
# Check integrity
sqlite3 ~/.ironclaw/ironclaw.db "PRAGMA integrity_check;"

# If integrity check returns anything other than "ok", the database is corrupt.
# Stop ironclaw first, then attempt recovery:
sqlite3 ~/.ironclaw/ironclaw.db ".recover" | sqlite3 ~/.ironclaw/ironclaw_recovered.db

# Swap the recovered database
cp ~/.ironclaw/ironclaw.db ~/.ironclaw/ironclaw.db.bak.$(date +%Y%m%d%H%M%S)
cp ~/.ironclaw/ironclaw_recovered.db ~/.ironclaw/ironclaw.db
```

**PostgreSQL:**

```bash
# Check for corruption
psql "$DATABASE_URL" -c "SELECT * FROM pg_catalog.pg_class WHERE relkind = 'r';" > /dev/null

# Restore from backup if pg_dump backups are available
pg_restore -d "$DATABASE_URL" /path/to/backup.dump
```

### Migration Failure

**Symptoms:** Log line `libSQL migration V{N} ({name}) failed` or `Migration` error variant at startup; service exits with non-zero code.

**Diagnosis:**

```bash
# Check which migrations have run
sqlite3 ~/.ironclaw/ironclaw.db "SELECT version, name, applied_at FROM _migrations ORDER BY version;"

# For PostgreSQL
psql "$DATABASE_URL" -c "SELECT version, name, applied_at FROM _migrations ORDER BY version;"
```

**Recovery:**

Migrations are transactional — a failed migration will roll back automatically. The service will not start with a partially-applied migration.

1. Review the failing migration SQL in `src/db/migrations/` (libSQL) or `src/db/postgres_migrations/` (PostgreSQL).
2. If the error is a transient infrastructure issue (disk full, lock timeout), fix the root cause and restart.
3. If the migration SQL has a bug:
   - Do not manually run partial SQL in production.
   - Roll back to the previous binary version if this is a new deployment.
   - Fix the migration source and redeploy.
4. Manually marking a migration as applied (emergency only — requires code review approval):
   ```bash
   sqlite3 ~/.ironclaw/ironclaw.db \
     "INSERT INTO _migrations (version, name, applied_at) VALUES (N, 'migration_name', datetime('now'));"
   ```

---

## LLM Provider Outage

### Identifying Circuit Breaker State

The circuit breaker (`src/llm/circuit_breaker.rs`) trips open after 5 consecutive transient failures and stays open for 30 seconds before allowing a probe. When open, every request returns immediately with:

```
LlmError::RequestFailed { reason: "Circuit breaker open (N consecutive failures, recovery in Xs)" }
```

**Diagnosis from logs:**

```bash
# Find circuit breaker state transitions
journalctl --user -u ironclaw | grep -E "Circuit breaker|HalfOpen|Closed|Open"
```

**States:**

- `Closed -> Open`: Backend has failed 5+ times consecutively. Provider is degraded.
- `Open -> HalfOpen`: Recovery timeout (30s) elapsed; probing backend.
- `HalfOpen -> Closed`: Backend recovered (2 successful probes).
- `HalfOpen -> Open`: Backend probe failed; back to open.

### Failover Chain

The `FailoverProvider` wraps the primary model. If the primary is in cooldown (3 consecutive retryable failures → 300s cooldown), the failover model (`NEARAI_FALLBACK_MODEL`) is used automatically. If all providers are in cooldown, the least-recently-cooled is tried.

```bash
# Check which model is active in logs
journalctl --user -u ironclaw | grep -E "provider=|model_name=|failover"
```

### Manual Provider Override

To bypass the current provider entirely and force a specific backend:

```bash
# Override to Anthropic
export LLM_BACKEND=anthropic
export ANTHROPIC_API_KEY=sk-ant-...
systemctl --user restart ironclaw

# Override to OpenAI
export LLM_BACKEND=openai
export OPENAI_API_KEY=sk-...
systemctl --user restart ironclaw

# Override to local Ollama (no circuit breaker dependency)
export LLM_BACKEND=ollama
export OLLAMA_BASE_URL=http://localhost:11434
systemctl --user restart ironclaw
```

To force a specific model on the NEAR AI backend (bypassing smart routing):

```bash
export NEARAI_MODEL=fireworks::accounts/fireworks/models/llama-v3p1-70b-instruct
unset NEARAI_CHEAP_MODEL      # Disable smart routing
unset NEARAI_FALLBACK_MODEL   # Disable failover
systemctl --user restart ironclaw
```

### Rate Limiting

**Symptoms:** Log lines `RateLimited { retry_after: Some(Xs) }`. The retry provider will honor `Retry-After` headers from the provider. If rate limiting is sustained:

1. Check request volume; the agent may be in a loop.
2. Reduce `NEARAI_MAX_RETRIES` to 1 to limit retry storms.
3. Enable the response cache: `NEARAI_RESPONSE_CACHE_ENABLED=true` to avoid duplicate calls.
4. If stuck in a loop, kill the offending job: find the job UUID in logs and mark it failed in the database:
   ```bash
   sqlite3 ~/.ironclaw/ironclaw.db \
     "UPDATE jobs SET status = 'Failed', error = 'manually terminated' WHERE id = 'JOB_UUID';"
   ```

---

## Worker Container Escape / Security Breach

### Indicators of Compromise

- Unexpected processes in the Docker container namespace
- Network traffic from a container to non-allowlisted hosts
- Files written outside `/workspace` or `/output` mounts
- Container running as root (expected UID is 1000)
- `readonly_rootfs` bypass

### Immediate Containment

```bash
# List all running IronClaw sandbox containers
docker ps --filter "name=sandbox-" --format "table {{.ID}}\t{{.Names}}\t{{.Status}}\t{{.CreatedAt}}"

# Kill all sandbox containers immediately
docker ps --filter "name=sandbox-" -q | xargs -r docker kill

# Remove them
docker ps -a --filter "name=sandbox-" -q | xargs -r docker rm -f

# Stop new containers from being created by stopping ironclaw
systemctl --user stop ironclaw
```

### Forensics

```bash
# Collect container logs before removal
for cid in $(docker ps -a --filter "name=sandbox-" -q); do
    docker logs "$cid" > "/tmp/container-${cid}.log" 2>&1
done

# Check Docker audit log for unusual capability usage
journalctl -u docker | grep -E "privileged|cap_add|seccomp" | tail -100

# Inspect the image used
docker inspect sandbox-IMAGE:TAG | jq '.[0].Config.User, .[0].HostConfig.CapDrop, .[0].HostConfig.SecurityOpt'

# Check for unexpected network connections from container
# (run while container is still alive)
docker exec CONTAINER_ID ss -tnp
```

### Verification of Security Constraints

Expected container config (from `src/sandbox/container.rs`):

- `CapDrop: ["ALL"]`, `CapAdd: ["CHOWN"]` only
- `SecurityOpt: ["no-new-privileges:true"]`
- `ReadonlyRootfs: true` (unless `FullAccess` policy)
- `User: "1000:1000"` (non-root)
- `NetworkMode: "bridge"` (proxied via IronClaw network proxy on allowlist)
- No host PID namespace, no host network

If any of these constraints are missing in a running container, treat it as a compromise.

### Post-Incident

1. Revoke any API keys or tokens that were mounted into or accessible from the container.
2. Audit `~/.ironclaw/workspace/` for any unexpected files written.
3. Review the tool call that triggered the container — find it in job history:
   ```bash
   sqlite3 ~/.ironclaw/ironclaw.db \
     "SELECT id, description, created_at FROM jobs WHERE status = 'InProgress' OR status = 'Failed' ORDER BY created_at DESC LIMIT 20;"
   ```
4. File a SEV-1 incident report and review the WASM tool that launched the container.

---

## Credential / Secrets Compromise

### Master Key Leak

The `SECRETS_MASTER_KEY` (or OS keychain entry) is the root of all secret encryption. If it is exposed, all stored secrets must be considered compromised.

**Immediate actions:**

1. Rotate all stored secrets immediately (do not wait for key rotation):
   ```bash
   # List all secrets stored in ironclaw
   ironclaw secrets list
   ```
2. Revoke each secret at its provider (API provider dashboards, AWS IAM, etc.).
3. Perform a full master key rotation per the [KEY_ROTATION.md](./KEY_ROTATION.md) guide.
4. If `SECRETS_MASTER_KEY` was in a `.env` file committed to git, scrub git history:
   ```bash
   git log --all --full-history -- .env
   # Use git-filter-repo or BFG Repo-Cleaner to remove it
   ```
5. Notify all users/integrations that had their secrets stored.

### API Key Exposure

**Symptoms:** Unexpected charges on LLM provider accounts, unknown API calls in provider logs.

1. Revoke the key immediately at the provider's dashboard.
2. Delete the secret from IronClaw:
   ```bash
   ironclaw secrets delete SECRET_NAME
   ```
3. Generate a new key and store it:
   ```bash
   ironclaw secrets set SECRET_NAME --value "new-key-value"
   ```
4. Check IronClaw job history for any exfiltration paths:
   ```bash
   sqlite3 ~/.ironclaw/ironclaw.db \
     "SELECT j.id, j.description, j.created_at FROM jobs j ORDER BY j.created_at DESC LIMIT 50;"
   ```
5. Audit the WASM tool or skill that had access to the exposed secret via `allowed_secrets`.

### NEARAI Session Token Compromise

The session token is persisted to `~/.ironclaw/session.json` (mode 0600) and optionally to the database `settings` table.

```bash
# Revoke the current session token
curl -X POST "https://private.near.ai/v1/auth/logout" \
  -H "Authorization: Bearer $NEARAI_SESSION_TOKEN"

# Remove local session file
rm -f ~/.ironclaw/session.json

# Clear from database settings
sqlite3 ~/.ironclaw/ironclaw.db \
  "DELETE FROM settings WHERE key = 'nearai.session_token';"

# Re-authenticate
ironclaw onboard
```

---

## Service Crash and Recovery

### SIGTERM / Graceful Shutdown Not Completing

IronClaw handles SIGTERM by:

1. Signaling the shutdown broadcast channel.
2. Draining in-flight scheduler jobs with a **30-second timeout** (`src/main.rs`).
3. Shutting down MCP child processes.
4. Flushing LLM trace recordings.
5. Stopping the webhook server.
6. Stopping the tunnel.

If shutdown hangs beyond 30 seconds:

```bash
# Check what is still running
journalctl --user -u ironclaw | tail -50

# Find the PID
systemctl --user show ironclaw --property MainPID

# Send SIGKILL as last resort (after 30s drain window)
kill -9 $(systemctl --user show ironclaw --property MainPID | cut -d= -f2)
```

For systemd, configure a kill timeout in the unit file if needed:

```ini
[Service]
TimeoutStopSec=45
KillSignal=SIGTERM
KillMode=mixed
```

### Zombie Processes

Docker container children may become zombies if the IronClaw process exits without waiting for them.

```bash
# Find zombie Docker containers
docker ps -a --filter "status=exited" --filter "name=sandbox-" \
  --format "table {{.ID}}\t{{.Names}}\t{{.ExitedAt}}"

# Remove all exited sandbox containers
docker ps -a --filter "status=exited" --filter "name=sandbox-" -q | xargs -r docker rm

# Find zombie processes owned by the ironclaw user
ps aux | grep -E "Z|zombie" | grep -v grep
```

### Stuck Jobs

Jobs transition through: `Pending -> InProgress -> Completed / Failed`. A job stuck in `InProgress` means the scheduler lost track of it (process crash, timeout not recorded).

```bash
# Find stuck jobs (InProgress for more than 1 hour)
sqlite3 ~/.ironclaw/ironclaw.db \
  "SELECT id, description, status, created_at FROM jobs WHERE status = 'InProgress' AND created_at < datetime('now', '-1 hour');"

# Mark stuck jobs as Failed
sqlite3 ~/.ironclaw/ironclaw.db \
  "UPDATE jobs SET status = 'Failed', error = 'recovered from stuck state' WHERE status = 'InProgress' AND created_at < datetime('now', '-1 hour');"
```

The state machine also supports `Stuck -> InProgress` (recovery) and `Stuck -> Failed`. If the scheduler detects a job has been InProgress too long, it transitions it to Stuck automatically on the next scheduler tick.

### Service Won't Start

```bash
# Check for configuration errors
RUST_LOG=ironclaw=debug ironclaw run 2>&1 | head -50

# Common causes:
# 1. DATABASE_URL not set and no libsql database found
# 2. LLM_BACKEND set to a provider without required API key
# 3. SECRETS_MASTER_KEY too short (minimum 32 bytes)
# 4. Port already in use (HTTP channel)
lsof -i :"${IRONCLAW_HTTP_PORT:-3000}"
```

---

## SSE / WebSocket Gateway Degraded

### Architecture

The web gateway SSE endpoint (`/api/chat/events`, `/api/logs/events`) uses a broadcast channel with a capacity of **256 events** (`src/channels/web/sse.rs`). Slow clients that fall behind will silently miss events; SSE clients are expected to reconnect and re-fetch history from the ring buffer.

The WebSocket endpoint (`/api/chat/ws`) is used for full-duplex chat. The gateway runs inside the unified `WebhookServer` via axum.

### Symptoms and Diagnosis

**SSE clients seeing gaps / missed events:**

```bash
# Check for broadcast lag in logs
journalctl --user -u ironclaw | grep -E "lagged|BroadcastStream|slow client"
```

This is expected behavior. The 256-event buffer is intentional. Clients should reconnect with `Last-Event-ID` to resume. If the application cannot tolerate gaps, reduce the server's event emission rate or increase the buffer by modifying `BROADCAST_CAPACITY` in `src/channels/web/sse.rs`.

**SSE connections dropping:**

```bash
# Check if the axum HTTP server is still running
curl -sf http://localhost:"${IRONCLAW_HTTP_PORT:-3000}"/health

# Check for TLS certificate issues if behind a proxy
openssl s_client -connect your-domain.com:443 -servername your-domain.com < /dev/null 2>&1 | grep -E "Verify|error"
```

**WebSocket upgrade failures:**

```bash
# Verify the upgrade header is accepted
curl -i -N \
  -H "Connection: Upgrade" \
  -H "Upgrade: websocket" \
  -H "Sec-WebSocket-Key: $(openssl rand -base64 16)" \
  -H "Sec-WebSocket-Version: 13" \
  -H "Authorization: Bearer $IRONCLAW_WEB_TOKEN" \
  http://localhost:"${IRONCLAW_HTTP_PORT:-3000}"/api/chat/ws
```

**Gateway not binding:**

```bash
# Check port in use
lsof -i :"${IRONCLAW_HTTP_PORT:-3000}"

# Check bind address config
printenv | grep -E "HTTP_PORT|BIND_ADDR|WEB_"
```

### Recovery

1. Restart the service to reset the broadcast channel and re-establish SSE connections:
   ```bash
   systemctl --user restart ironclaw
   ```
2. If behind a reverse proxy (nginx, Caddy), ensure the proxy has `proxy_read_timeout` set to at least 3600s for SSE endpoints.
3. If the tunnel (Cloudflare/ngrok/Tailscale) is the cause, restart the tunnel:
   ```bash
   ironclaw service tunnel restart
   ```

---

## Escalation Contacts

> Replace the placeholders below with real contact information before deploying.

| Role | Contact | When to Page |
|------|---------|--------------|
| Primary On-Call | `[PLACEHOLDER: oncall@example.com]` | SEV-1 and SEV-2 |
| Security Lead | `[PLACEHOLDER: security@example.com]` | Any credential compromise or container escape |
| Database Admin | `[PLACEHOLDER: dba@example.com]` | Database corruption, migration failure |
| LLM Provider Support | NEAR AI: `support@near.ai` / provider dashboard | Circuit breaker stuck open for >1 hour |
| Infrastructure | `[PLACEHOLDER: infra@example.com]` | Docker daemon failure, OS-level issues |

**Incident log location:** `[PLACEHOLDER: https://incidents.example.com]`

**Status page:** `[PLACEHOLDER: https://status.example.com]`
