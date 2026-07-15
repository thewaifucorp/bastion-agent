#!/usr/bin/env bash
set -euo pipefail

echo "=== External Agent E2E Validation (ORCH-03) ==="
echo
echo "Automated companion:"
echo "  cargo test mcp_client_e2e --test -- --nocapture"
echo
echo "Manual validation: terminal-agent provider"
echo "  1. Configure a terminal-agent provider in bastion.toml or env."
echo "  2. Start Bastion with OTel stdout or OTLP enabled."
echo "  3. Send a turn that routes to terminal-agent."
echo "  4. Verify the external agent output is returned as data."
echo
echo "Manual validation: MCP client to external MCP server"
echo "  1. Start an MCP-compliant external server."
echo "  2. Add it under [mcp.servers] in bastion.toml."
echo "  3. Start Bastion and verify the MCP client registers tools."
echo "  4. Invoke one registered tool through a normal turn."
echo
echo "Manual validation: OTel correlation"
echo "  1. Enable BASTION_OTEL_STDOUT=true or OTLP export."
echo "  2. Invoke terminal-agent or an MCP client tool."
echo "  3. Verify Bastion and external agent spans share trace context where supported."
echo
echo "ORCH-03 validation procedure documented."
