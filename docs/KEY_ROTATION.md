# Master Key Rotation Guide

## Overview

The IronClaw master key is the root of the secrets encryption hierarchy. Rotating it requires decrypting every stored secret with the old key and re-encrypting it with the new key, then updating the key in the OS keychain or environment variable.

**This procedure applies equally to both database backends (PostgreSQL and libSQL/Turso).** Both backends use the same `SecretsCrypto` layer (`src/secrets/crypto.rs`) and store the same ciphertext format.

---

## What the Master Key Protects

### Key Derivation Hierarchy

```
master_key (32+ bytes)
    │
    └── HKDF-SHA256(master_key, per-secret salt, info="near-agent-secrets-v1")
            │
            └── derived_key (32 bytes, AES-256)
                    │
                    └── AES-256-GCM encrypt/decrypt per secret
```

Each secret row in the `secrets` table contains:
- `encrypted_value`: `nonce (12 bytes) || ciphertext || GCM tag (16 bytes)`
- `key_salt`: 32-byte random salt unique to that secret

The master key itself never touches a secret directly. HKDF derives a unique AES-256 key per secret using the per-secret salt, so even two identical plaintext secrets produce different ciphertexts.

**A compromised or rotated master key makes all stored secrets unreadable without re-encryption.** There is no secondary recovery path.

---

## Where the Master Key is Stored

The key is loaded at startup via a sequential probe in `src/config/secrets.rs`:

1. `SECRETS_MASTER_KEY` environment variable (takes precedence)
2. OS keychain:
   - **macOS**: Keychain Services, service `ironclaw`, account `master_key`
   - **Linux**: Secret Service (GNOME Keyring or KWallet), same labels

The key is stored as a hex string in both keychain backends. Minimum length is 32 bytes (64 hex characters).

---

## Safety Precautions

Before rotating, do the following without exception:

1. **Perform the rotation in a test environment first.** Verify that all secrets decrypt correctly with the new key before touching production.
2. **Take a database backup.** If rotation fails partway through, the backup is the only recovery path.
   ```bash
   # libSQL / SQLite
   cp ~/.ironclaw/ironclaw.db ~/.ironclaw/ironclaw.db.pre-rotation.$(date +%Y%m%d%H%M%S)

   # PostgreSQL
   pg_dump "$DATABASE_URL" -Fc -f ironclaw-pre-rotation-$(date +%Y%m%d%H%M%S).dump
   ```
3. **Stop the IronClaw service.** Do not rotate while the service is running — in-flight operations may partially succeed with the old key, leaving the database in an inconsistent state.
   ```bash
   systemctl --user stop ironclaw
   # or
   launchctl unload ~/Library/LaunchAgents/com.ironclaw.daemon.plist
   ```
4. **Keep the old key available until the rotation is verified complete.**

---

## Step-by-Step Rotation Procedure

### Step 1: Generate a New Master Key

```bash
# Generate 32 cryptographically random bytes and hex-encode them
NEW_MASTER_KEY=$(openssl rand -hex 32)
echo "New key (64 hex chars, save this securely): $NEW_MASTER_KEY"
echo "${#NEW_MASTER_KEY} chars (expect 64)"
```

Store the new key immediately in a password manager or secure vault before proceeding.

### Step 2: Export All Existing Secrets (Decrypt Phase)

With the old key loaded, decrypt every secret and write it to a temporary plaintext export file. This file is sensitive — store it on a `tmpfs` or encrypted volume, and delete it as soon as re-encryption is complete.

```bash
# Ensure the old key is active
export SECRETS_MASTER_KEY="$OLD_MASTER_KEY"

# Export all secrets as a JSON array: [{user_id, name, value, provider, expires_at}, ...]
# IronClaw CLI export command (writes to stdout)
ironclaw secrets export --output /dev/shm/secrets-export.json

# Verify the export is non-empty
jq length /dev/shm/secrets-export.json
```

If the CLI export command is not available in your build, export manually via the database and a small Rust script, or use the `ironclaw tool memory_search` API to enumerate them. Alternatively, use the database directly for libSQL:

```bash
# List secret names per user to verify scope
sqlite3 ~/.ironclaw/ironclaw.db \
  "SELECT user_id, name, provider FROM secrets ORDER BY user_id, name;"
```

### Step 3: Re-Encrypt All Secrets with the New Key

```bash
# Set the new key
export SECRETS_MASTER_KEY="$NEW_MASTER_KEY"

# Re-import all secrets; this overwrites each row with new ciphertext
# (ON CONFLICT ... DO UPDATE in both PostgreSQL and libSQL backends)
ironclaw secrets import --input /dev/shm/secrets-export.json
```

If the CLI import command is not available, the re-encryption can be driven programmatically by reading the export JSON and calling `SecretsStore::create()` with the new `SECRETS_MASTER_KEY` active. The upsert logic in both backends will overwrite the existing row's `encrypted_value` and `key_salt` columns.

### Step 4: Update the Key in the OS Keychain / Environment

**macOS keychain:**

```bash
# Remove the old entry
security delete-generic-password -s ironclaw -a master_key

# Store the new entry (security prompts for confirmation)
security add-generic-password -s ironclaw -a master_key -w "$NEW_MASTER_KEY"

# Verify
security find-generic-password -s ironclaw -a master_key -w
```

**Linux secret service (GNOME Keyring):**

```bash
# Delete the old entry
secret-tool clear service ironclaw account master_key

# Store the new entry
echo -n "$NEW_MASTER_KEY" | secret-tool store --label="ironclaw master key" service ironclaw account master_key

# Verify
secret-tool lookup service ironclaw account master_key
```

**Environment variable (headless / containerized deployments):**

```bash
# Update .env or the systemd unit override
# For systemd user unit:
systemctl --user edit ironclaw
# Add or update:
# [Service]
# Environment=SECRETS_MASTER_KEY=<new_key>

# For launchd, edit the plist EnvironmentVariables key:
# ~/Library/LaunchAgents/com.ironclaw.daemon.plist
```

For container or CI deployments, update the secret in the secrets manager (AWS Secrets Manager, Vault, etc.) and redeploy.

### Step 5: Verify All Secrets Decrypt Correctly with the New Key

```bash
# Start ironclaw with the new key (do not fully restart the service yet)
SECRETS_MASTER_KEY="$NEW_MASTER_KEY" RUST_LOG=ironclaw::secrets=debug ironclaw run --dry-run 2>&1 | grep -E "secrets|decrypt|error" | head -40

# Or verify via the CLI
SECRETS_MASTER_KEY="$NEW_MASTER_KEY" ironclaw secrets list

# Spot-check a known secret
SECRETS_MASTER_KEY="$NEW_MASTER_KEY" ironclaw secrets get SECRET_NAME
```

For a thorough check, write a verification script that attempts `get_decrypted` on every secret name returned by `list`:

```bash
# Verify all secrets decrypt without error
SECRETS_MASTER_KEY="$NEW_MASTER_KEY" ironclaw secrets verify-all
```

If any secret fails to decrypt with the new key, the export/import step did not complete successfully. Roll back using Step 6 before investigating.

### Step 6 (Normal): Invalidate / Remove the Old Key

Only proceed here after Step 5 confirms successful decryption of all secrets.

```bash
# Securely zero the variable from the shell session
unset OLD_MASTER_KEY

# Delete the export file from memory
shred -u /dev/shm/secrets-export.json 2>/dev/null || rm -f /dev/shm/secrets-export.json

# Remove the old key from any .env files or config
# Search for it
grep -rn "$OLD_KEY_PREFIX" ~/.ironclaw/ 2>/dev/null

# Delete any plaintext backups of the old key
```

### Step 7: Restart the Service

```bash
# Reload the new key into the running environment
systemctl --user start ironclaw
# or
launchctl load -w ~/Library/LaunchAgents/com.ironclaw.daemon.plist

# Confirm healthy startup
journalctl --user -u ironclaw -n 30 | grep -E "error|warn|started|secrets"
```

---

## Emergency Rollback Procedure

If the rotation fails after Step 3 (new key in keychain, some secrets re-encrypted but others failed), use the database backup to recover.

```bash
# Stop the service
systemctl --user stop ironclaw

# Restore the pre-rotation database backup
# libSQL
cp ~/.ironclaw/ironclaw.db.pre-rotation.TIMESTAMP ~/.ironclaw/ironclaw.db

# PostgreSQL
pg_restore -d "$DATABASE_URL" --clean ironclaw-pre-rotation-TIMESTAMP.dump

# Restore the old master key to the keychain
security delete-generic-password -s ironclaw -a master_key  # macOS
security add-generic-password -s ironclaw -a master_key -w "$OLD_MASTER_KEY"

# Or set the env var
export SECRETS_MASTER_KEY="$OLD_MASTER_KEY"

# Restart with old key
systemctl --user start ironclaw

# Verify secrets are accessible
ironclaw secrets list
ironclaw secrets get SAMPLE_SECRET_NAME
```

If the database backup itself is corrupt, the secrets are unrecoverable from IronClaw storage. Re-enter them from the source systems (provider dashboards, password manager, etc.).

---

## Scheduled Rotation Recommendation

| Trigger | Action |
|---------|--------|
| Annual (minimum) | Full rotation per this guide |
| Suspected key exposure | Immediate rotation (SEV-1) |
| Employee offboarding with key access | Rotation within 24 hours |
| OS keychain compromise or hardware reset | Immediate rotation |
| Deployment to new infrastructure | Generate a fresh key; do not copy old key |

Automate the rotation reminder with a cron job or calendar alert. There is no built-in rotation scheduling in IronClaw — this is a manual operational procedure.

---

## Notes on Both Database Backends

The crypto layer (`src/secrets/crypto.rs`) is database-agnostic. Both `PostgresSecretsStore` and `LibSqlSecretsStore` call the same `SecretsCrypto::encrypt()` and `SecretsCrypto::decrypt()` methods. The `secrets` table schema is identical across both backends:

```
secrets(id, user_id, name, encrypted_value BLOB, key_salt BLOB, provider, expires_at, ...)
```

The rotation procedure does not vary by backend. If you run both backends (unlikely in practice but supported), rotate and verify each database independently using the same new master key.
