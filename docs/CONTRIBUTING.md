# Contributing to Nitroid

Thanks for considering a contribution! This document covers the basics.

## Code of conduct

Be respectful. Be specific. Be patient. Disagreements happen — address them on the merits, not the person.

## Getting started

1. **Fork** the repository on GitHub.
2. **Clone** your fork locally:
   ```bash
   git clone https://github.com/<your-username>/nitroid.git
   cd nitroid
   git remote add upstream https://github.com/salom600/nitroid.git
   ```
3. **Create a branch** for your work:
   ```bash
   git checkout -b feat/my-feature
   ```
4. **Build and test** as described in [BUILDING.md](BUILDING.md).
5. **Open a pull request** targeting `main`.

## What to work on

Issues tagged [`good-first-issue`](../../issues?q=is:open+label:good-first-issue) are the best starting point. Issues tagged [`help-wanted`](../../issues?q=is:open+label:help-wanted) are higher-impact but may require deeper context.

If you have an idea that isn't tracked in an issue, please open one first so we can discuss it before you spend time on code.

## Code style

- Run `cargo fmt --all` before committing — CI checks this.
- Run `cargo clippy --workspace --all-targets -- -D warnings` — CI checks this too.
- Every public function, struct, and module must have a doc comment. The CI doesn't enforce this yet (we'll add `#![deny(missing_docs)]` once the API stabilises), but reviewers will.
- Tests go in the same file as the code, in a `#[cfg(test)] mod tests` block at the bottom. Integration tests go in `tests/`.

## Commit messages

Use the [Conventional Commits](https://www.conventionalcommits.org/) format:

```
<type>: <subject>

[optional body]

[optional footer]
```

Types we use:
- `feat` — a new feature
- `fix` — a bug fix
- `docs` — documentation only
- `refactor` — code change that neither adds a feature nor fixes a bug
- `perf` — code change that improves performance
- `test` — adding or correcting tests
- `ci` — CI configuration changes
- `chore` — anything else (dependency bumps, etc.)

Example:

```
feat(input): add support for gamepad axes in the keymap

Extends the HostEvent enum with GamepadAxis variants and adds the
corresponding translation logic to InputTranslator. Closes #42.
```

## Pull request checklist

Before opening a PR:

- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] Public API has doc comments
- [ ] New behaviour has tests
- [ ] Commit messages follow Conventional Commits
- [ ] Branch is up to date with `main` (rebase if needed)

## Reviewing

A maintainer will review your PR within 7 days. Reviews focus on:

1. **Correctness** — does the change do what it says?
2. **Tests** — is the behaviour covered?
3. **API** — does this change lock us into a design we might regret?
4. **Performance** — does this introduce a regression?
5. **Safety** — if the change touches `unsafe`, is the safety argument sound?

If your PR is stalled for more than 2 weeks, ping it with a comment.

## License

By contributing, you agree that your contributions are dual-licensed under the MIT and Apache 2.0 licenses, as described in the project LICENSE files.

## Security

If you discover a security vulnerability, please **do not** open a public issue. Email the maintainers privately instead. See [SECURITY.md](SECURITY.md) for details.
