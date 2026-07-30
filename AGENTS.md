# Instructions for coding agents

Read `CONTRIBUTING.md` first. It describes what this program is, where the code
lives, how one keystroke travels through it, the checks CI runs, the commit and
pull request conventions, and the parts that are pinned on purpose. All of it
applies to you.

This file adds only what an agent working here gets wrong.

## You cannot run this program

It is Windows-only, it requires administrator rights, and it installs a driver.
These three commands are your whole feedback loop:

```
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
```

If you could not run them, say so and say which parts are therefore unverified.
Do not describe a change as working because it looks right.

## Verify the Win32 surface, do not recall it

- The `windows` crate is pinned and its features are listed explicitly in
  `Cargo.toml`. Check what the pinned version provides instead of the signature
  you remember, and add the feature in the same commit as the call.
- A wrong argument or a missing feature shows up as an item that does not exist,
  not as a helpful error.
- Behaviour at that boundary is not guessable. If you cannot tell what Windows
  returns in a case, handle it explicitly rather than assuming.

## Stay inside what was asked

- One concern per pull request. Do not widen a diff beyond the request; if you
  notice something adjacent, mention it instead of changing it.
- Do not edit `README.md` unless the change was asked for.
- Do not add or upgrade a dependency without the matching `Cargo.lock` update in
  the same commit. `cargo` writes that file; it cannot be edited by hand,
  because the checksums are not something you can produce.
- Do not merge a pull request.

## Do not invent what is already decided

- The offsets, sizes and IOCTL codes in `src/driver/ioctl.rs` were read out of
  one build of the driver. Never relax an assertion to make a build pass.
- The timeouts, retries and pauses in `src/driver/` were chosen against real
  hardware behaviour. Leave them unless the change is about them.
- Small helpers already exist for path encoding, process lookup, device
  enumeration and handle ownership. Search before adding another.

## Comments, names and tests

- Every `unsafe` block needs a `SAFETY:` comment saying why it is sound; clippy
  fails the build without one.
- Comments say why, briefly, and only where the reason is not in the code. Do
  not narrate, do not annotate a diff, and do not delete a comment that records
  how something was found out.
- Names are words, not letters. Test names are sentences about the property
  being pinned down.
- A behaviour change comes with the test that would have caught it.

## English only

Code, comments, documentation, commit messages, pull request titles and
descriptions.
