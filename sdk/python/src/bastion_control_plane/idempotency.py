"""Mirrors ``sdk/typescript/src/idempotency.ts``."""

from __future__ import annotations

import uuid


def generate_idempotency_key() -> str:
    """Generate an opaque idempotency key for ``BastionClient.create_task``.

    This is a CONVENIENCE, not a requirement -- any caller-chosen unique
    string works (the server derives its own storage key from
    ``sha256(owner || idempotency_key)``; see
    ``src/control_plane/routes.rs``'s ``deterministic_task_id``). Prefer your
    own stable id (e.g. an upstream issue id) over this generator whenever
    one already exists -- a random key defeats the point of idempotent retry
    across process restarts, since a NEW key on retry after a crash creates a
    second task instead of returning the first.
    """
    return str(uuid.uuid4())
