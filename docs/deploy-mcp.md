# Deploy MCP endpoint (sift.bobka.net)

Current MCP transport is stdio only (`sift mcp`). To expose it publicly:

1. Build release binary on target host.
2. Run `sift mcp` behind an HTTP/SSE bridge (e.g. mcp-proxy or a small wrapper).
3. Put it behind nginx/Caddy with TLS.
4. Publish via Cloudflare Tunnel as `sift.bobka.net`.
5. Add auth (token) before public access.

Blockers: remote host access, transport bridge choice, auth policy.