# bastion-control-plane

Python client for Bastion's External Control Plane API (`/v1/*`). Mirrors
`sdk/typescript/` field-for-field against the same frozen wire contract —
see `docs/en/contracts/control-plane-v1.openapi.yaml` (repo root) and
`docs/en/control-plane-security.md` for the threat model.

Zero runtime dependencies: built on `urllib.request`/`hmac`/`hashlib`/`uuid`
(all stdlib), the same minimalism the TypeScript SDK's "one class, built on
the global `fetch`" approach follows.

## Install (from a local checkout)

```bash
cd sdk/python
pip install -e ".[dev]"
```

## Usage

```python
import os
from bastion_control_plane import BastionClient

client = BastionClient(
    base_url="http://127.0.0.1:8080",
    token=os.environ["BASTION_TOKEN"],  # bcp_<opaque>
)

task = client.create_task({"objective": "Fix the auth bug"})
client.pause_task(task["id"], task["revision"])

for t in client.tasks(status="running"):
    print(t["id"], t["objective"])
```

Response shapes are plain `dict`s typed as `TypedDict`s for editor/type-checker
support (`bastion_control_plane.types`) — like the TypeScript SDK's `interface`s,
there is no runtime validation; the client trusts the server's JSON to match.

## Verifying inbound webhook deliveries

```python
from bastion_control_plane import verify_webhook_signature

# `raw_body` MUST be the exact bytes received — see verify_webhook_signature's
# doc comment on why a re-serialized copy fails verification.
ok = verify_webhook_signature(subscription_secret, raw_body, request.headers.get("X-Bastion-Signature"))
```

## Testing

```bash
pip install -e ".[dev]"
pytest
```

Spins up a real local HTTP server (`http.server`) as a mock Bastion API, so
requests exercise actual `urllib.request` calls rather than a mocked
transport — the same approach `sdk/typescript`'s own test suite documents
(a real `node:http` server), ported to Python's stdlib equivalent.
