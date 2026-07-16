# Architecture

Bastion is a Rust product runtime layered on `bastion-core`. The product repository composes the agent loop with real-world channels, configuration, MCP services, extensions, and a mobile companion.

```text
channel / CLI / mobile
          │
          ▼
  src/channel + src/api
          │  identity and trust classification
          ▼
  AgentHandle / AgentLoop (bastion-core)
          │
          ├── providers and sessions
          ├── capability registry and approval boundary
          ├── memory, personas, cognition, mesh
          ▼
 src/mcp + extensions + local sidecars
```

## Product boundaries

| Area | Responsibility | Main location |
| --- | --- | --- |
| Entry point | CLI commands, config loading, process composition | `src/main.rs` |
| Configuration | TOML/env loading and identity validation | `src/config.rs` |
| Channels | Telegram, webhook, WhatsApp, Discord, Slack, email, voice | `src/channel/` |
| API | Inference route and webhook/mobile pairing surfaces | `src/api/`, `src/channel/webhook.rs` |
| MCP | Client composition and optional MCP stdio server | `src/mcp/` |
| Extensions | Declarative, WASM, and subprocess extension host | `src/extension/` |
| Companion | Flutter mobile application | `mobile/` |
| Local skills | Memory, self-improvement, voice, and other MCP services | `skills/` |

## A message flow

1. A channel adapter receives a message or the CLI receives `agent --message`.
2. The adapter resolves the sender against the configured identity table.
3. The adapter marks public or externally supplied content with its trust level.
4. `AgentHandle` sends the request into the shared agent loop.
5. The runtime uses its provider, session, memory, and registered capabilities to produce a response.
6. The adapter returns the response to the source channel. Unknown owners are not given an agent session.

## Deployment shape

`docker-compose.yml` runs the Rust core plus sidecars for MemuPalace, skill writing, self-improvement, and voice. The internal `bastion-net` prevents the Python sidecars from reaching the internet; only the core also joins `egress-net`. State lives in named volumes, while `bastion.toml`, personas, and most skills are mounted from the repository.

## Extension and MCP boundary

MCP clients are always part of the product composition. The MCP server surface is feature-gated by `mcp-server`; when compiled, `bastion mcp-stdio` provides a local stdio transport. Extensions are a separate product boundary: declarative, WASM, and subprocess mechanisms are hosted under `src/extension/`. Installing an extension is not equivalent to granting it unlimited authority—the runtime capability model remains the decision point.

## Repository layout

```text
src/        Rust binary and product composition
skills/     Local MCP services and reusable skills
mobile/     Flutter companion application
tests/      Rust integration, adversarial, and end-to-end coverage
docker-compose.yml  Local multi-service deployment
bastion.toml        Non-secret defaults
```
