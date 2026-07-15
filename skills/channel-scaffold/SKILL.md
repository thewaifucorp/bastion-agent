---
name: bastion/channel-scaffold
version: "1.0.0"
description: >
  Scaffolds a new Bastion channel skill: generates the SKILL.md stub and documents the
  Channel trait requirements, bastion.toml registration, and OwnerMap auth rules.
triggers:
  - /add-channel
  - /new-channel
  - "adicionar canal"
---

# bastion/channel-scaffold

Use this skill to create a new Bastion channel skill. Channels are skills, not core — they
implement the Channel trait and register via bastion.toml. Specific channels (WhatsApp, Discord,
Email) are community and future work; this skill provides the scaffold and documentation only.

## Objective

Generate a `skills/add-<channel>/` directory with a SKILL.md stub and document the three
mandatory rules every channel implementation must follow.

---

## Channel Trait Requirements

Every Bastion channel MUST satisfy all three rules:

### Rule 1: Implement the Channel trait (`src/channel/mod.rs`)

```rust
#[async_trait::async_trait]
pub trait Channel: Send + Sync {
    /// Run the channel's I/O loop forever. Each inbound message is sent to the AgentLoop;
    /// the reply is returned over the channel's transport.
    async fn run(self: Box<Self>, agent: AgentHandle) -> anyhow::Result<()>;

    /// Optional default persona hint for messages arriving on this channel (CHAN-04).
    fn default_persona(&self) -> Option<&str>;
}
```

Both methods are required. `run` owns the I/O loop; `default_persona` returns the persona
hint sent to the router for messages on this channel (return `None` if no preference).

### Rule 2: Register via `bastion.toml [[channel]]`

Channels are not auto-discovered. Each channel must have a `[[channel]]` entry in
`bastion.toml`. The daemon reads these at startup and instantiates the matching type.

```toml
[[channel]]
type   = "whatsapp"          # matches the Rust type slug registered in main.rs
config = { phone = "+55..." } # channel-specific config passed to the constructor
```

### Rule 3: Never call providers directly — route all messages through AgentHandle

Channels are transport adapters, not reasoning engines. All LLM calls, memory reads,
and skill dispatches must flow through `AgentHandle`. A channel that calls a provider
directly violates the architecture boundary and breaks persona routing.

```rust
// Correct — forward to AgentHandle
let reply = agent.send(Message::new(owner_id, text)).await?;

// Wrong — never do this from a channel
let reply = openai_client.complete(prompt).await?;
```

### Rule 4: HTTP handlers MUST use OwnerMap.resolve(token) → 401 (CR-03)

Any HTTP handler in a channel that accepts requests from untrusted clients must resolve the
caller's identity from a trusted token map — never from the request body.

```rust
// CR-03 pattern — mandatory for any HTTP channel handler
let owner_id = owner_map
    .resolve(&token)
    .ok_or(StatusCode::UNAUTHORIZED)?; // 401 if unknown token

// Wrong — request body controls identity (privilege escalation risk)
let owner_id = body.owner_id; // NEVER do this
```

This is the same pattern used by `WebhookChannel` (see `src/channel/webhook.rs`).
Community channel developers who skip CR-03 create an authentication bypass.

---

## Generated Stub Structure

When scaffolding `/add-<channel>`, generate:

```
skills/
  add-<channel>/
    SKILL.md           ← conversational skill (this stub)
```

The SKILL.md stub guides the user through wiring the new channel in Rust and registering it.
The Rust implementation lives in `src/channel/<channel>.rs` (outside this skill's scope).

---

## SKILL.md Stub Template

```markdown
---
name: bastion/add-<channel>
version: "1.0.0"
description: >
  Guides setup of the <Channel> channel for Bastion: installation, authentication,
  and bastion.toml registration.
triggers:
  - /add-<channel>
  - "configurar <channel>"
---

# bastion/add-<channel>

Sets up the <Channel> integration so Bastion can receive and send messages via <Channel>.

## Prerequisites

- [ ] <Channel> account and API credentials
- [ ] Bastion daemon v1.0+ running

## Setup Steps

1. **Obtain credentials**: [channel-specific auth steps]
2. **Add to bastion.toml**:
   ```toml
   [[channel]]
   type   = "<channel>"
   config = { api_key = "<YOUR_API_KEY>" }
   ```
3. **Restart daemon**: `systemctl restart bastion` (or `cargo run` for dev)
4. **Test**: send a message via <Channel>; Bastion should reply.

## Implementation Notes (for contributors)

- Implement `Channel` trait in `src/channel/<channel>.rs`
- Use `OwnerMap.resolve(token)` for any HTTP callback — never trust request body identity (CR-03)
- Route all messages through `AgentHandle` — do not call providers directly
- Register the type slug in `main.rs` channel factory

## Security

- Store API keys in environment variables, never in bastion.toml plaintext
- Any webhook endpoint must validate the channel's signature header before processing
- Apply CR-03 pattern: resolve owner from trusted map, return 401 for unknown credentials
```

---

## Example: Community WhatsApp Channel

A community developer building WhatsApp support would:

1. Implement `WhatsAppChannel` in `src/channel/whatsapp.rs` (implements Channel trait)
2. Add `[[channel]] type = "whatsapp"` to bastion.toml
3. Create `skills/add-whatsapp/SKILL.md` using the stub above
4. Submit the skill to agentskills.io for community review

The WhatsApp channel does NOT ship in Bastion core (D-11). It is community work that follows
this scaffold. Core ships only `WebhookChannel` (HTTP/SSE) and `TelegramChannel`.

---

## Security Reminder

New HTTP handlers introduced by a community channel inherit the daemon's trust surface.
Before merging any community channel:

1. Verify `OwnerMap.resolve(token)` is called in every handler that modifies state (CR-03)
2. Verify no provider is called directly (only via AgentHandle)
3. Verify API keys are read from env vars, not hardcoded
4. Verify webhook signatures are validated before processing payloads

Channels that omit these checks create privilege escalation vectors (T-06-04-03).

---

## Edge Cases

- **Channel type slug collision**: if `bastion.toml [[channel]] type` matches an existing built-in, the daemon will use the built-in. Use a distinct slug (e.g., `community-whatsapp`).
- **Multiple instances of the same channel**: Bastion supports multiple `[[channel]]` blocks with the same type but different configs (e.g., two Telegram bots). Each gets its own AgentHandle clone.
- **Channel startup failure**: if `channel.run()` returns an error, the daemon logs it and continues running other channels. One broken channel does not take down the daemon.
