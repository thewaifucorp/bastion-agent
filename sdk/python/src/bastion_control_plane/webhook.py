"""Mirrors ``sdk/typescript/src/webhook.ts``.

Verifies an inbound webhook delivery's ``X-Bastion-Signature`` header,
matching ``src/control_plane/webhook_delivery.rs``'s ``sign_payload`` exactly
(HMAC-SHA256 over the raw body bytes, ``sha256=<hex>``).
"""

from __future__ import annotations

import hashlib
import hmac
import re
from typing import Optional, Union

_SIGNATURE_RE = re.compile(r"^[0-9a-fA-F]{64}$")
_PREFIX = "sha256="


def verify_webhook_signature(
    secret: str,
    raw_body: Union[bytes, str],
    signature_header: Optional[str],
) -> bool:
    """
    Args:
        secret: The signing secret returned ONCE at subscription creation
            (``BastionClient.create_webhook_subscription``'s response) --
            store it yourself; the server never returns it again.
        raw_body: The EXACT bytes received on the wire. Passing a
            re-serialized/re-parsed copy of the JSON will fail verification
            even for a genuine delivery -- whitespace/key-order can differ
            from what was signed. Use your framework's raw-body access (e.g.
            Flask's ``request.get_data()``), not ``json.dumps(request.json)``.
        signature_header: The full ``X-Bastion-Signature`` header value,
            e.g. ``"sha256=3f2a..."``.
    """
    if not signature_header or not signature_header.startswith(_PREFIX):
        return False
    expected_hex = signature_header[len(_PREFIX) :]
    if not _SIGNATURE_RE.match(expected_hex):
        return False

    body_bytes = raw_body.encode("utf-8") if isinstance(raw_body, str) else raw_body
    actual_hex = hmac.new(secret.encode("utf-8"), body_bytes, hashlib.sha256).hexdigest()

    # hmac.compare_digest is the stdlib's constant-time comparison -- never a
    # plain `==` on a signature.
    return hmac.compare_digest(actual_hex.lower(), expected_hex.lower())
