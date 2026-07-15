---
name: bastion/mesh-connect
version: "1.0.0"
description: >
  Manages P2P mesh pairing between Bastion instances: generates one-time tokens,
  handles /connect-peer flow, lists peer status, triggers sync, and manages allowlists.
triggers:
  - /connect-peer
  - /mesh-status
  - /mesh-sync
  - /mesh-allowlist
  - "conectar instância"
---

# bastion/mesh-connect

Connects two Bastion instances over a secure P2P mesh — so Mario's instance and Ana's instance
can share tagged beliefs (e.g., `mercado`, `calendario`) without either owner's private data
leaving their node.

## Triggers

| Trigger | Action |
|---------|--------|
| `/connect-peer` | Generate a one-time pairing token and guide the peer connection flow |
| `/mesh-status` | List registered peers and their last successful sync timestamp |
| `/mesh-sync` | Trigger an immediate mesh sync to all registered peers |
| `/mesh-allowlist <peer> <tags>` | Update the allowlist for a peer — control which belief tags they receive |

---

## Flow: /connect-peer (7 steps)

```
1. User types /connect-peer in their Bastion chat.
2. Bastion generates a one-time pairing token: BAST-PEER-XXXX (5-minute TTL).
3. Token is displayed in chat. User shares it out-of-band (Signal, email, voice).
4. The peer enters the token on their own Bastion instance (e.g., /connect-peer <token>).
5. Peer daemon POSTs /mesh/pair { token, peer_url, age_pubkey } to this instance.
6. This instance validates TTL, registers the peer in bastion.toml [[mesh.peer]],
   and stores the peer's age_pubkey for E2E encryption.
7. Connection established — first sync happens on the next scheduler tick (15 min default).
```

---

## Flow: /mesh-status

```
1. User types /mesh-status.
2. Bastion reads all [[mesh.peer]] entries from bastion.toml.
3. For each peer: show owner_id, peer_url, last_sync_at, allowed_tags.
4. Example output:
     Peers (2 connected):
     - ana@bastion: https://ana.bastion.local | last sync: 14 min ago | tags: mercado, calendario
     - work@bastion: https://work.bastion.local | last sync: 1 h ago | tags: projetos
```

---

## Flow: /mesh-sync

```
1. User types /mesh-sync.
2. Bastion triggers spawn_mesh_sync_job immediately (does not wait for next scheduler tick).
3. For each peer: filter_for_mesh collects CloudOk beliefs with allowed tags → encrypt with
   peer's age_pubkey → POST /mesh/ingest to peer_url.
4. Reports sync result: "Synced 12 beliefs to ana@bastion. 0 errors."
```

---

## Flow: /mesh-allowlist <peer> <tags>

```
1. User types /mesh-allowlist ana mercado,calendario.
2. Bastion updates [[mesh.peer]] allowed_tags for the matched peer.
3. New allowlist takes effect on the next sync.
4. Confirms: "Updated allowlist for ana@bastion: mercado, calendario."
```

---

## bastion.toml Configuration

After pairing, bastion.toml is updated automatically. Manual configuration follows this schema:

```toml
[[mesh.peer]]
owner_id    = "ana@bastion"
peer_url    = "https://ana.bastion.local"
age_pubkey  = "age1xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
allowed_tags = ["mercado", "calendario"]

[mesh.allowlist.ana]
tags = ["mercado", "calendario"]
```

- `owner_id`: unique identifier for the peer (matches the peer's daemon identity)
- `peer_url`: base URL of the peer daemon (must be reachable from this instance)
- `age_pubkey`: bech32 age public key received during /mesh/pair handshake
- `allowed_tags`: only beliefs tagged with these tags will be shared with this peer

---

## API Reference

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/mesh/pair` | Pairing handshake — peer sends `{ token, peer_url, age_pubkey }` |
| `POST` | `/mesh/ingest` | Receive an encrypted MeshEnvelope from a peer |
| `GET` | `/mesh/status` | Returns peer list and last sync timestamps (owner-authenticated) |

The `/mesh/pair` endpoint validates:
1. Token exists in the pending token table and TTL has not expired (5 min)
2. `peer_url` is a valid HTTPS URL
3. `age_pubkey` is a valid bech32 age public key
4. On success: registers peer in MeshPeerMap; deletes the one-time token

---

## Network Requirements

| Scenario | Setup |
|----------|-------|
| Same LAN | Direct IP/hostname — no extra config needed |
| Cross-network (home + work) | Tailscale recommended; assign stable hostnames via MagicDNS |
| Remote managed | Bastion Cloud (closed relay) |

Bastion Cloud acts as a blind relay: it forwards encrypted `MeshEnvelope.ciphertext` without
holding any private key. Neither party needs to expose a public IP.

---

## Privacy Guarantees

- **Allowlist-only sharing**: only beliefs tagged with peer's `allowed_tags` cross the boundary.
  Untagged beliefs stay local by default.
- **local-only beliefs never leave the node** (WR-04): `filter_for_mesh` calls `check_egress`
  on every belief before inclusion. Beliefs with `privacy_tier: local-only` are dropped silently.
- **E2E encryption**: all mesh traffic is encrypted with the peer's age public key before transit.
  Neither the network nor the relay can read belief content.
- **No implicit sync**: sync only happens on the scheduler tick or explicit `/mesh-sync`. No
  background exfiltration.

---

## Canonical Example: Mario + Ana

Mario and Ana want to share grocery lists and calendar events, but Mario's health beliefs
(tagged `saude`) must never leave his node.

```toml
# Mario's bastion.toml
[[mesh.peer]]
owner_id     = "ana@bastion"
peer_url     = "https://ana.bastion.local"
age_pubkey   = "age1ana..."
allowed_tags = ["mercado", "calendario"]
# "saude" is NOT in allowed_tags → health beliefs never leave Mario's node
```

When sync runs:
- `filter_for_mesh` collects Mario's beliefs → keeps only `mercado` + `calendario` tagged beliefs
- `check_egress` (WR-04) drops any belief with `privacy_tier: local-only` regardless of tag
- Remaining beliefs are encrypted with Ana's age key → posted to `/mesh/ingest` on Ana's instance
- Ana's instance decrypts and merges into her local Cabinet

Mario's `saude` beliefs are never included — the allowlist acts as the boundary, and WR-04 is
the safety net for any belief that slips through with a local-only tier.

---

## Edge Cases

- **Token expired (> 5 min)**: reject with 401; user must run `/connect-peer` again to get a new token.
- **peer_url unreachable**: sync skips this peer and logs a warning. Use `/mesh-status` to diagnose.
- **age_pubkey mismatch on ingest**: the receiving daemon rejects the envelope (cannot decrypt). Re-pair to refresh keys.
- **Peer removed from bastion.toml**: beliefs are no longer sent; existing synced beliefs on the peer's node are not recalled (no delete protocol in v1.0).
- **Multiple instances, same owner_id**: last registration wins; the earlier peer entry is overwritten.
