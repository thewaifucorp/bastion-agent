# FAQ

## Is Bastion a hosted service?

No. Bastion is a self-hosted product runtime built on `bastion-core`. You choose the host, channels, provider configuration, and operating controls.

## Where does state live?

Sessions use the SQLite path configured by `session.db_path`. The Compose deployment also creates named volumes for the core and local sidecars. Back up those volumes as application data.

## Why does a channel ignore a message?

The sender may not be mapped in `[[identity]]`, the channel may be disabled, or its credential may be absent. Check structured logs without exposing tokens. See [Channels](channels.md).

## Can I use the mobile app remotely?

Yes, but only after you make the webhook/mobile surface reachable through a deployment you control and protect. Do not expose the default port without an explicit network and access-control plan. See [Mobile companion](mobile-app.md).

## Does Bastion support MCP?

Yes. It composes MCP clients, and a build with the `mcp-server` feature can expose `bastion mcp-stdio` for local stdio transport.

## How does Bastion improve?

The skill-writing path can identify completed-work patterns as candidates for reusable skills. Candidates are queued for approval; they are not silently auto-applied.
