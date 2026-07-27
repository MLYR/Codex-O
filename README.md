# Codex-O

Codex-O is a local-first Tauri 2 desktop application. This repository currently contains
the T0 application shell, nine formal routes, and machine-verifiable quality gates.

## Development

```bash
npm ci
npm run tauri dev
```

## Quality Gates

```bash
npm run lint
npm run typecheck
npm test -- --run
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml
npm run check
```
