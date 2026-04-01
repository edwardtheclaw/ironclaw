"""Ownership model E2E tests.

Verifies:
- Bootstrap creates the owner user on startup (single-tenant).
- The pairing API works end-to-end with the DB-backed store.
- Owner identity is stable across requests (settings scoped correctly).
- Regression: existing owner-scope behaviour is preserved.
"""

import asyncio
import uuid

import httpx

from helpers import AUTH_TOKEN


def _headers():
    return {"Authorization": f"Bearer {AUTH_TOKEN}"}


# ---------------------------------------------------------------------------
# Bootstrap
# ---------------------------------------------------------------------------


async def test_server_starts_and_health_ok(ironclaw_server):
    """Server starts cleanly after bootstrap_ownership runs."""
    async with httpx.AsyncClient() as client:
        r = await client.get(f"{ironclaw_server}/api/health", timeout=10)
    assert r.status_code == 200


async def test_settings_written_and_readable(ironclaw_server):
    """Settings written by the owner are readable in the next request — scope stable."""
    key = f"e2e_ownership_{uuid.uuid4().hex[:8]}"

    async with httpx.AsyncClient() as client:
        w = await client.post(
            f"{ironclaw_server}/api/settings/{key}",
            json={"value": "ownership_ok"},
            headers=_headers(),
            timeout=10,
        )
    # Accept 200, 201, or 204
    assert w.status_code in (200, 201, 204), f"Write failed: {w.status_code} {w.text[:200]}"

    async with httpx.AsyncClient() as client:
        r = await client.get(
            f"{ironclaw_server}/api/settings/{key}",
            headers=_headers(),
            timeout=10,
        )
    assert r.status_code == 200
    assert "ownership_ok" in str(r.json()), f"Expected ownership_ok in: {r.json()}"


async def test_unauthenticated_cannot_read_settings(ironclaw_server):
    """Unauthenticated requests cannot read owner-scoped settings."""
    async with httpx.AsyncClient() as client:
        r = await client.get(
            f"{ironclaw_server}/api/settings/e2e_ownership_test",
            timeout=10,
        )
    assert r.status_code in (401, 403), f"Expected auth rejection, got {r.status_code}"


# ---------------------------------------------------------------------------
# Pairing API — DB-backed store
# ---------------------------------------------------------------------------


async def test_pairing_list_empty_for_new_channel(ironclaw_server):
    """GET /api/pairing/{channel} returns empty list for a channel with no requests."""
    async with httpx.AsyncClient() as client:
        r = await client.get(
            f"{ironclaw_server}/api/pairing/ownership-test-channel",
            headers=_headers(),
            timeout=10,
        )
    assert r.status_code in (200, 404)
    if r.status_code == 200:
        data = r.json()
        requests = data.get("requests", data if isinstance(data, list) else [])
        assert requests == [], f"Expected empty list, got: {requests}"


async def test_approve_invalid_code_returns_error(ironclaw_server):
    """POST /api/pairing/{channel}/approve with unknown code returns 400/404."""
    async with httpx.AsyncClient() as client:
        r = await client.post(
            f"{ironclaw_server}/api/pairing/ownership-test-channel/approve",
            json={"code": "NOTEXIST"},
            headers=_headers(),
            timeout=10,
        )
    assert r.status_code >= 400, f"Expected error, got {r.status_code}: {r.text[:200]}"


async def test_pairing_requires_auth(ironclaw_server):
    """All pairing endpoints reject unauthenticated requests."""
    async with httpx.AsyncClient() as client:
        r = await client.get(
            f"{ironclaw_server}/api/pairing/ownership-test-channel",
            timeout=10,
        )
    assert r.status_code in (401, 403)

    async with httpx.AsyncClient() as client:
        r = await client.post(
            f"{ironclaw_server}/api/pairing/ownership-test-channel/approve",
            json={"code": "ABCD1234"},
            timeout=10,
        )
    assert r.status_code in (401, 403)
