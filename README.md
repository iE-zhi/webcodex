# WebCodex

Headless ChatGPT MCP backend configured through TOML files. **Exactly two backend executables**:

- **`webcodex-server`** — MCP, authentication, task lifecycle, and embedded OpenAI Tunnel.
- **`webcodex-runner`** — projects, files, Git, shell commands, browser, and Computer Use.

No standalone `webcodex` CLI, Desktop, Web Console, or display-only MCP Apps are distributed. Minimal Computer screenshot and Agent/Job continuation Host carriers remain.

## Build

```bash
cargo build --locked --release --bins -p webcodex -p webcodex-runner
```

## Configuration

Configuration uses `server.toml`, `runner.toml`, `runner-bindings.toml`, and per-project TOML files. Secrets must live in separate owner-private files referenced from TOML.

```bash
webcodex-server --config /etc/webcodex/server.toml --check-config
webcodex-runner --config /etc/webcodex/runner.toml --check-config
webcodex-server --config /etc/webcodex/server.toml
webcodex-runner --config /etc/webcodex/runner.toml
```

Runner registration uses a SHA-256 credential hash and exact Runner identity in the Server's private TOML; no CLI pairing is needed. See the [TOML deployment guide](docs/HEADLESS_TOML.md), the [detailed Chinese guide](docs/HEADLESS_TOML.zh-CN.md), and `config/*.example.toml`.

Python 3.11+ is needed for release tooling. Legacy env-based deployments remain supported for migration. Never place plaintext secrets in normal TOML or Git. See [SECURITY.md](SECURITY.md).

License: Apache-2.0.
