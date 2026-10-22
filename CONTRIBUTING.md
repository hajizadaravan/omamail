# Contributing to Omamail

Thanks for your interest in contributing to Omamail!

Omamail is a lightweight Gmail client for the Omarchy shell,
with a QML interface and a Rust backend.

## Before You Start

Bug fixes and documentation improvements are welcome as direct
pull requests.

For new features, please open an issue first so we can discuss
the proposal before implementation.

## Reporting Bugs

Please include:

- A clear description of the problem.
- Steps to reproduce it.
- What you expected to happen and what actually happened.
- Relevant details about your setup.
- Screenshots or logs, if helpful.

## Making Changes

1. Fork the repository and create a branch for your changes.
2. Keep your changes focused on a single issue or improvement.
3. Follow the existing code style and project structure.
4. Update the documentation if your changes affect how Omamail works.
5. Test the parts affected by your changes.

Avoid unrelated refactoring or formatting changes in the same pull request.

## Versioning

Every change that lands on `main` reaches users as an update, so every
change bumps the version, following [Semantic Versioning](https://semver.org).
`1.0.0` is the first stable release; from there:

- **Patch** (`1.0.0` → `1.0.1`): bug fixes and documentation.
- **Minor** (`1.0.0` → `1.1.0`): new features that keep working setups working.
- **Major** (`1.0.0` → `2.0.0`): changes that need users to do something,
  such as rerunning `install.sh` by hand or signing in again.

The version lives in `manifest.json` and `backend/Cargo.toml`, and
`backend/Cargo.lock` follows `Cargo.toml`. Bump all three in the same
commit: `install.sh` builds with `--locked`, so a stale lockfile breaks
installs and updates. To update the lockfile, run `cargo build` once
without `--locked`: it rewrites only Omamail's own entry. Do not run a
plain `cargo update`, which would also move every dependency.

Before committing, run `cargo test --locked` in `backend/`. It fails when
`manifest.json` and `Cargo.toml` disagree, and, like `install.sh`, when
`Cargo.lock` is stale; a plain `cargo test` would quietly fix the
lockfile on your machine only.

## Testing

Describe how you tested your changes in your pull request.

For UI changes, include screenshots. Check that the interface continues
to use the Omarchy shell's components and theme conventions.

If you could not test something, clearly mention it in the pull request.

## Submitting a Pull Request

Use a clear title and include:

- What changed and why.
- A link to the related issue, if there is one.
- How you tested the changes.
- Screenshots for UI changes.
- Any known limitations or untested behavior.

## AI-Assisted Contributions

AI agents and coding assistants are welcome. Review and test any
AI-generated changes before submitting them.

## Working Together

Be respectful and constructive in issues, pull requests, and reviews.
Keep feedback focused on the code and the problem being solved.
