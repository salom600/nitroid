# Security Policy

## Reporting a vulnerability

If you discover a security vulnerability in Nitroid, please **do not** open a public GitHub issue. Instead:

1. Email the maintainer at `salom600` via GitHub's [private vulnerability reporting](../../security/advisories/new).
2. Include a clear description of the issue and a minimal reproduction if possible.
3. Allow up to 7 days for an initial response.

## Scope

Vulnerabilities in scope:
- Anything that lets a Nitroid instance escape its sandbox
- Anything that lets a malicious Android image compromise the host
- Anything that lets a downloaded system image execute arbitrary code on first run
- Privilege escalation through the KVM/WHPX backend
- Crashes triggered by malformed input files (system images, keymap JSON, etc.)

Out of scope:
- Bugs in the guest Android system itself (report those to Android-x86 or Bliss OS)
- Bugs in third-party libraries (report those upstream)
- Issues that require physical access to the machine
- "Information disclosure" of data the user explicitly opted in to share

## Hardening checklist for contributors

When contributing code that touches:

- **The virtualization backend** — every `unsafe` block must have a `// SAFETY:` comment explaining the invariant being relied on.
- **The image loader** — never trust the image's declared size; re-stat the file before reading.
- **The JSON parser** — never use `serde_json::from_str` on untrusted input without a size cap.
- **The translation cache** — never `mmap` a file with write+execute permissions.
- **The network code** (future) — every TCP listener must bind to `127.0.0.1` only unless explicitly configured otherwise.

## Secret management

**Never commit secrets to this repository.** This includes:

- GitHub Personal Access Tokens
- API keys
- SSH private keys
- Cloud credentials

If you accidentally push a secret:

1. **Revoke it immediately** at the provider's settings page. This is the most important step — once a secret is in a public commit, assume it is compromised.
2. Force-push to remove it from history:
   ```bash
   git rebase -i HEAD~5    # find the offending commit
   git push --force-with-lease
   ```
3. Open an issue describing what happened so maintainers can audit access logs.

GitHub's secret scanning will automatically revoke any leaked PAT it detects in commits or issues.

## Dependency policy

We use [`cargo-deny`](https://github.com/EmbarkStudios/cargo-deny) to scan dependencies for known vulnerabilities and licence issues. The configuration is in `deny.toml`. CI runs `cargo deny check` on every PR.

To check locally:

```bash
cargo install cargo-deny
cargo deny check
```

If `cargo deny` flags one of your dependencies:

1. **Update** the dependency to a non-vulnerable version if one exists.
2. If no fix is available, **pin** the dependency to a specific commit (using a `[patch.crates-io]` entry in the workspace `Cargo.toml`) and open an issue tracking the upstream fix.
3. If the dependency is essential and no fix is possible, document the risk in the PR and the maintainers will decide.
