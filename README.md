 ![InputRedirect](/.github/assets/logo.png)
 <p align="center">
  <img src="https://img.shields.io/badge/language-Rust-orange">
  <img src="https://img.shields.io/badge/platform-Windows-blue">
  <img src="https://img.shields.io/badge/license-MIT-blue.svg">
  <img src="https://github.com/kerogenesis/InputRedirect/actions/workflows/ci.yml/badge.svg?branch=main">
</p>

A small Windows utility that sends your keyboard, touchpad and mouse
through a signed Logitech HID driver, so applications see the input as coming
from a real Logitech device.

One executable, nothing to install: the signed driver files are embedded in it
and deployed on first start.

## Requirements

- Windows 10 or 11
- Administrator rights

## Quick Start

Run `InputRedirect.exe` as administrator and choose an option:

| Key | Action |
| --- | --- |
| `1` | Redirect mouse / touchpad clicks |
| `2` | Redirect keyboard |
| `3` | Stop everything |
| `4` | Re-create the virtual devices |
| `R` | Remove the driver |
| `Q` | Quit |

`1` and `2` are switches - press again to turn them off. Closing the window turns the
redirect off.

After `R` the machine has to restart before the driver is fully gone; the tool
offers it and reminds you on the next start if you skipped it.

## Command line

You do not have to open the menu. Started from a terminal, InputRedirect can
switch a redirect on straight away - handy for shortcuts, scheduled tasks and
scripts:

```
Usage:
    InputRedirect [options]

Options:
    -m, --mouse            redirect the mouse buttons
    -k, --keyboard         redirect the keyboard
    -r, --remove-driver    remove the driver
    -h, --help             show this help
```

With `--mouse`, `--keyboard`, or both (short `-m` / `-k`), the tool starts as
usual, switches on what you asked for and prints one line saying what is
running - no menu. Closing the window or pressing Ctrl+C switches everything
back, exactly as the menu does. `--remove-driver` (short `-r`) runs the same
removal as the `R` key.

## Building

```sh
cargo build --release
cargo test
```

The binary lands in `target/x86_64-pc-windows-msvc/release/InputRedirect.exe`.
Ready-made builds of the latest commit are published under
[Releases](../../releases/tag/latest).

## Compatibility
Note that some anti-cheat systems block the Logitech driver used by InputRedirect. As a result, the utility may not function in games protected by those systems.

At least it still works with a certain 20+ year old Korean MMO ;)
