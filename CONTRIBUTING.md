# Contributing to InputRedirect

InputRedirect is a Windows-only, user-mode program that redirects keyboard and
mouse input through a real, signed HID driver. It installs the driver, plugs in
virtual devices, intercepts input with low-level hooks, and re-sends it through
the driver.

This means most changes touch one of three things: the Win32/NT boundary, the
hooks, or the per-event hot path. Read this file before opening a pull request.

## Reporting an issue

A report is only useful if it can be told apart from a machine-specific
problem, so include:

- The Windows build, and whether Memory integrity (HVCI) is on.
- What the program printed. Every failure goes through one error type and comes
  out as a sentence on the console; that sentence is the most useful line in the
  report.
- Whether Logitech G HUB or another Logitech driver was installed beforehand.
- What you did, what you expected, and what happened instead.

Anti-cheat software blocking the driver is expected rather than a bug; see the
Compatibility section of the README.

## Where things live

| Path             | What is in it                                                     |
| ---------------- | ----------------------------------------------------------------- |
| `src/main.rs`    | Entry point and exit codes                                        |
| `src/error.rs`   | The crate's error type; every failure path goes through it        |
| `src/app/`       | Startup, shutdown, single-instance guard, consent, menu actions     |
| `src/driver/`    | Driver install and removal, virtual devices, IOCTLs, the watchdog  |
| `src/redirect/`  | The hooks, the decision made per event, echo suppression, combos   |
| `src/hid/`       | HID report formats, modifier flags, scan code tables               |
| `src/ui/`        | Console output: layout, colours, the menu, keypress reading        |
| `drivers/`       | The bundled signed driver package                                  |
| `res/`           | Application manifest and icon resources                            |
| `build.rs`       | Embeds the manifest and resources                                  |

## How one keystroke travels

The shortest path into the codebase is to follow a single key press through it.

1. **The hook receives it.** `src/redirect/hook.rs` installs a low-level
   keyboard and mouse hook on a thread of its own and runs a message loop there,
   because that is how Windows delivers hook callbacks. The hooks are installed
   again periodically: Windows drops a hook whose callback took too long, and it
   does not say so.
2. **The engine decides.** `src/redirect/mod.rs` turns the event into one of a
   few outcomes: pass it through, swallow it, or send it through the driver. The
   decision is a plain function over the event, which is why it is the part with
   the most tests.
3. **Our own echo is let through.** The virtual keyboard is a real HID device,
   so a report we send arrives back at our own hook a moment later.
   `src/redirect/echo.rs` recognises it and passes it on untouched - without
   that, the first keystroke would go round for ever.
4. **Modifiers stay on the physical path.** They are not events of their own to
   the driver: they ride along in the same report as the key they modify, which
   `src/redirect/combo.rs` keeps track of. Redirecting them separately would
   break every shortcut.
5. **The report is built.** `src/hid/` turns the event into the bytes a device
   sends: `scancode.rs` maps the key to its HID usage, `keyboard.rs` fills the
   eight-byte report with its modifier byte and six key slots, `mouse.rs` the
   five-byte one. Nothing here knows about Windows, so it is fully covered by
   tests and the easiest place to start reading.
6. **The driver sends it.** `src/driver/ioctl.rs` holds the request layout,
   `src/driver/device.rs` the open handle. The original event is swallowed, so
   the application only ever sees the one that came from the virtual device.

Pointer movement and the wheel are deliberately not redirected: they carry no
information an application checks the device for, and passing them through the
driver only adds latency and jitter to the pointer.

## Running it

This is not a program that can be casually run to see what a change does:

- Windows only, on `x86_64`. There is no cross-platform path and no stub.
- The manifest requires administrator rights, so it will not start elevated by
  accident - it either runs elevated or exits.
- It installs a driver package into the system driver store, creates a root
  device, and can ask for a reboot to finish. Uninstalling is a separate action
  in the menu, not something that happens on exit.

So verify changes with `cargo test` and CI. Treat an actual run as a deliberate
act on a machine you are willing to have a driver installed on.

## Before you push

CI runs these, in this order, and fails the build on the first one that
complains. Run them locally on Windows first:

```
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
```

Notes that catch people out:

- **A new Win32 call needs its feature.** The `windows` crate is compiled with
  an explicit feature list in `Cargo.toml`. Calling an API from a module that
  is not in that list fails to compile with the item simply not existing - add
  the feature in the same commit.
- The `windows` crate is a moving target across versions. An associated
  function that a tutorial or an older branch uses may no longer exist; check
  what the pinned version actually provides rather than the name you expect.
- `--locked` means `Cargo.lock` must already match `Cargo.toml`. Commit the
  updated lock file together with any dependency change.
- Clippy runs with `pedantic` and `-D warnings`. Among other things, an item
  named in a doc comment has to be in backticks, and every `unsafe` block needs
  a `SAFETY:` comment.
- `rustfmt` runs with default settings, so an argument list wider than 60
  characters gets split across lines even though the line fits in 100.
- Every commit that lands on the branch should build on its own. Do not split a
  signature change and its call sites into separate commits.

## Build configuration

These are pinned on purpose. A change here changes what ships, so it belongs in
its own pull request with a reason:

- `rust-toolchain.toml` pins the toolchain, and `Cargo.toml` states the minimum
  supported version.
- `.cargo/config.toml` sets the target to `x86_64-pc-windows-msvc` and links
  the CRT statically, so the release binary runs without a redistributable. CI
  checks the built executable for dynamic CRT imports.
- The release profile uses fat LTO, one codegen unit, and strips symbols.
- CI checks that `drivers/` is the expected package, that its catalogues carry
  a valid signature from the Windows Hardware Compatibility Publisher - which
  is the signature a kernel driver needs, and the only one it can have, since
  the vendor submits the package and Microsoft signs it - and that the INF files
  name Logitech as the vendor.

## Pull requests

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

## Code conventions

- `unsafe_op_in_unsafe_fn` is denied. Every `unsafe` block carries a `SAFETY:`
  comment stating why the call is sound - what owns the handle, who guarantees
  the pointer, which thread it runs on.
- Comments explain why, briefly. They are not a narration of the code.
- Names are words, not letters. Tests are named as sentences describing the
  property being pinned down, for example
  `a_plug_refused_while_the_bus_catches_up_is_asked_again`.
- Prefer RAII wrappers over paired open/close calls, and absolute paths over
  anything Windows would resolve through `PATH`.
- The hooks run on the system's input path. Work inside a hook callback is
  latency for every keystroke on the machine, so nothing goes in there that can
  block, allocate without need, or call out to Windows unnecessarily.
- Never let a panic cross an `extern "system"` boundary: unwinding out of a
  callback aborts the process, and an abort skips the destructors that release
  the virtual devices.

## The driver interface is not ours to simplify

`src/driver/ioctl.rs` describes a binary interface that belongs to the driver:
request layouts, field offsets, packing, IOCTL codes, report descriptors, and
the vendor and product ids the devices are published under. The driver reads
these bytes at fixed positions.

The assertions in that file are the specification. A layout that looks
redundant, a padded field, or an odd struct size is the driver's requirement,
not an oversight. If a change there is genuinely needed, the assertions must be
updated deliberately and the reason stated - never relaxed to make a build pass.

## Things to leave alone

These look like oversights and are not. Changing them needs a reason in the
pull request description:

- The low-level hook architecture, and the choice not to redirect pointer
  movement or the wheel.
- The absence of `panic = "abort"` in the release profile.
- The offset assertions in `src/driver/ioctl.rs`, which pin down the layout the
  driver expects.
- The driver verification step in CI.
- Closing G HUB while the program runs. It claims the same two virtual devices,
  and none of its own settings are affected.
- The two questions in `src/app/consent.rs`, and the fact that `Driver::connect`
  cannot install anything without one of them being answered. Installing a
  kernel driver outlives the program, so it is asked for rather than assumed;
  `SECURITY.md` and the README say the same things the screens do, and the three
  are meant to stay in step. A start that changes nothing asks nothing, which is
  what keeps the questions worth reading.

## Releasing

There are two workflows. `ci.yml` checks and builds every push and pull request
and keeps the binary as a run artifact; it publishes nothing. `release.yml`
runs only on a tag and is the only thing that creates a release.

To publish version `X.Y.Z`:

1. Bump `version` in `Cargo.toml`, and the `input-redirect` entry in
   `Cargo.lock` in the same commit. The lock file is not optional here: CI
   builds with `--locked` and will fail on a bump that only touched one file.
2. Merge that through a pull request like any other change.
3. Tag the merge commit and push the tag:

```
git tag vX.Y.Z
git push origin vX.Y.Z
```

The tag has to match the version in `Cargo.toml`; the workflow refuses to
publish otherwise, because an archive whose name and whose executable disagree
about the version is worse than no release. From there it verifies the driver
files, runs the tests, builds, checks the binary really is statically linked,
packs `InputRedirect.exe` and `LICENSE` into
`InputRedirect-X.Y.Z-x86_64-windows.zip` with a `.sha256` beside it, and
publishes a release whose notes are generated from the commits and pull
requests since the previous tag.

A tag with a suffix - `vX.Y.Z-rc1` - is published as a prerelease.

Nothing here deletes or rewrites a published release. A release that went out
wrong is superseded by the next tag, not edited in place.
