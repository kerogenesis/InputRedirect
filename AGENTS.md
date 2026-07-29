# Contributing to InputRedirect

InputRedirect is a Windows-only, user-mode program that redirects keyboard and
mouse input through a real, signed HID driver. It installs the driver, plugs in
virtual devices, intercepts input with low-level hooks, and re-sends it through
the driver.

This means most changes touch one of three things: the Win32/NT boundary, the
hooks, or the per-event hot path. Read this file before opening a pull request.

## General requirements

- Keep pull requests small and focused. One concern per pull request; if a
  change needs a rename or a refactor to make sense, that is its own commit.
- Explain the issue and why the change fixes it. A diff that only says what it
  does cannot be reviewed - the reasoning is the part that has to be checked.
- Before adding new functionality, make sure it does not already exist
  elsewhere in the codebase. This project has small, purpose-built helpers
  (path encoding, process lookup, device enumeration, handle wrappers) that are
  easy to reinvent by accident.
- English only, everywhere: commit messages, commit bodies, pull request titles
  and descriptions, code, comments and documentation.

## Pull request titles

Titles follow conventional commit standards:

| Prefix      | Use for                                              |
| ----------- | ---------------------------------------------------- |
| `feat:`     | new feature or functionality                         |
| `fix:`      | bug fix                                              |
| `docs:`     | documentation or README changes                      |
| `chore:`    | maintenance tasks, dependency updates, etc.          |
| `refactor:` | code refactoring without changing behaviour          |
| `test:`     | adding or updating tests                             |

When a pull request spans several kinds of change, use the prefix that
describes its purpose, not the largest part of the diff. Individual commits
within the branch use the same prefixes.

## Before you push

CI runs these, in this order, and fails the build on the first one that
complains. Run them locally on Windows first:

```
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test
```

Notes that catch people out:

- `--locked` means `Cargo.lock` must already match `Cargo.toml`. Commit the
  updated lock file together with any dependency change.
- Clippy runs with `pedantic` and `-D warnings`. Among other things, an item
  named in a doc comment has to be in backticks.
- `rustfmt` runs with default settings, so an argument list wider than 60
  characters gets split across lines even though the line fits in 100.
- Every commit that lands on the branch should build on its own. Do not split a
  signature change and its call sites into separate commits.

## Code conventions

- `unsafe_op_in_unsafe_fn` is denied. Every `unsafe` block carries a `SAFETY:`
  comment stating why the call is sound - what owns the handle, who guarantees
  the pointer, which thread it runs on.
- Comments explain why, briefly. They are not a narration of the code.
- Tests are named as sentences describing the property being pinned down, for
  example `a_plug_refused_while_the_bus_catches_up_is_asked_again`.
- Prefer RAII wrappers over paired open/close calls, and absolute paths over
  anything Windows would resolve through `PATH`.
- The hooks run on the system's input path. Work inside a hook callback is
  latency for every keystroke on the machine, so nothing goes in there that can
  block, allocate without need, or call out to Windows unnecessarily.
- Never let a panic cross an `extern "system"` boundary: unwinding out of a
  callback aborts the process, and an abort skips the destructors that release
  the virtual devices.

## Things to leave alone

These look like oversights and are not. Changing them needs a reason in the
pull request description:

- The low-level hook architecture, and the choice not to redirect pointer
  movement or the wheel.
- The absence of `panic = "abort"` in the release profile.
- The offset assertions in `src/driver/ioctl.rs`, which pin down the layout the
  driver expects.
- The Authenticode verification step in CI.
