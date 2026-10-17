# WebCodex headless TOML deployment

WebCodex ships only `webcodex-server` and `webcodex-runner`. Runtime configuration is TOML; no standalone CLI or Desktop is required.

Build:

```bash
cargo build --locked --release --bins -p webcodex -p webcodex-runner
```

Templates: `config/server.example.toml`, `config/runner.example.toml`, `config/runner-bindings.example.toml`, and `config/project.example.toml`.

Validate and start each binary:

```bash
webcodex-server --config /etc/webcodex/server.toml --check-config
webcodex-runner --config /etc/webcodex/runner.toml --check-config
webcodex-server --config /etc/webcodex/server.toml
webcodex-runner --config /etc/webcodex/runner.toml
```

The Server reads `[server]`, `[runners]`, optional `[tunnel]` and `[options]` sections from `server.toml`. The Runner reads `runner.toml` and per-project TOML files from its `project_registry_dir`. For a new Runner identity, generate a `wc_agent_` token with 64 hex characters, keep the plaintext only in an owner-private `token_file`, and register its SHA-256 hash/prefix and exact owner/client ID in Server's private `runner-bindings.toml`. Only agent transport scopes are granted. A revoked token cannot be recreated implicitly.

Server's bootstrap token and optional OpenAI Tunnel API key are also owner-private files referenced by TOML, never inline credentials. The Tunnel runs **inside Server**; remote Runners connect directly to Server over a reachable secure URL. Computer Use executes on the Runner host. Existing authenticated management APIs remain available.

For detailed steps and safe provisioning commands, see [the Chinese headless TOML guide](HEADLESS_TOML.zh-CN.md).
