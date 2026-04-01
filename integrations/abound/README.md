# Abound Integration

Customer-specific integration for Abound's remittance platform. This directory is designed to be separable into its own repo.

## Contents

```
integrations/abound/
├── credentials.json                     # Credential mappings for Abound's API hosts
├── skills/abound-remittance/SKILL.md    # Agent skill for remittance workflows
└── tests/
    ├── test_abound_api_direct.py        # Direct tests against Abound's dev API
    └── test_abound_e2e.py              # E2E tests through IronClaw's Responses API
```

## Deployment

Set these env vars in Railway (or your deployment):

```bash
# Points IronClaw at the Abound credential mappings
INTEGRATION_CREDENTIALS=/app/integrations/abound/credentials.json

# Points the skill system at the Abound skill
SKILLS_DIR=/app/integrations/abound/skills
```

The Dockerfile already sets these.

## Per-User Credential Setup

After creating a user via the Admin API, inject their Abound credentials:

```bash
# Inject bearer token (per-user, from Abound)
PUT /api/admin/users/{user_id}/secrets/abound_external_token
{"value": "<user's abound token>", "provider": "abound"}

# Inject shared API key
PUT /api/admin/users/{user_id}/secrets/abound_api_key
{"value": "<shared X-API-KEY>", "provider": "abound"}
```

The `http` tool will auto-inject `Authorization: Bearer` and `X-API-KEY` headers for all requests to Abound's hosts.

## Running Tests

```bash
# Direct API tests (no IronClaw needed)
python integrations/abound/tests/test_abound_api_direct.py

# E2E tests (requires running IronClaw deployment)
python integrations/abound/tests/test_abound_e2e.py
```

## Abound Dev Endpoints

| Endpoint | Method | URL |
|----------|--------|-----|
| Account Info | GET | `devneobank.timesclub.co/times/bank/remittance/agent/account/info` |
| Exchange Rate | GET | `devneobank.timesclub.co/times/bank/remittance/agent/exchange-rate` |
| Send Wire | POST | `devneobank.timesclub.co/times/bank/remittance/agent/send-wire` |
| Notification | POST | `dev.timesclub.co/times/users/agent/create-notification` |
