# ll-desktop-cli

Rust client for the Lunatic-Lorry desktop control plane.

## Ownership boundary

`ll-desktop-cli` is a **client only**. It does not launch Lunatic, supervise WASM actors, own Cloudflare Tunnel processes, mutate desktop desired state, or perform updates directly. All machine lifecycle changes flow through `ll-desktop-daemon`.

The daemon currently exposes a versioned loopback API under `/v1`. This client authenticates with the daemon token file and supports deterministic JSON output for automation.

## Runtime semantics

Lunatic-Lorry actors are **one-shot and non-reusable**. Each `invoke` request is delegated to the daemon, which must launch a fresh Lunatic/WASM process and reap it after success, failure, cancellation, or timeout. The CLI must never implement pooling or process reuse.

## Commands

```text
ll-desktop-cli status
ll-desktop-cli invoke --tenant <tenant> --deployment <revision> --payload '{"key":"value"}'
```

Global options are defined in `.cli-flags.toml` and parsed through the pinned `flags2env` binding. Unknown options and invalid typed values fail closed.

## Local authentication

The default token path is:

```text
~/.lunatic-lorry/daemon/token
```

Override only with `LL_DESKTOP_TOKEN_FILE`. The token is never accepted as a command-line flag so it does not leak through argv or shell history.

## Current scope

This bootstrap closes the previously empty CLI-repository blocker and establishes the daemon-client boundary. Follow-up work remains tracked by `lunatic-lorry/ll-desktop-infra#1`: cancellation, deploy/revision lifecycle, logs, doctor, reconcile, tunnel control, updates, keep-awake, and native desktop-app parity.
