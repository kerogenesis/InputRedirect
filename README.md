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

## What it does to your computer

Read this once before the first run. The program asks the same question on
screen and installs nothing until you answer it.

InputRedirect cannot create a virtual keyboard or mouse by itself. It carries
Logitech's signed driver package and installs it on first start: three kernel
drivers, added to the Windows driver store and registered as system services.
They were written by Logitech, not by this project.

Two consequences are worth weighing before you say yes:

- **Memory Integrity.** Windows may refuse to turn Core Isolation / Memory
  Integrity on while these drivers are installed.
- **Anti-cheat.** Protected games look for this driver by name and may refuse to
  run alongside it - see [Compatibility](#compatibility).

The driver stays installed after the window closes. `D` in the menu removes it
again, after which the computer has to restart to finish the job.

While InputRedirect is running it also keeps **Logitech G HUB closed**, because
G HUB claims the same two virtual devices and a product id can only be claimed
once. Profiles, macros and lighting live in G HUB's own files and are applied
again the next time it starts.

[SECURITY.md](SECURITY.md) writes all of this out as a threat model, including
where you should not install it.

## Quick Start

Run `InputRedirect.exe` as administrator and choose an option:

| Key | Action |
| --- | --- |
| `1` | Redirect mouse / touchpad clicks |
| `2` | Redirect keyboard |
| `3` | Stop everything |
| `4` | Re-create the virtual devices |
| `D` | Remove the driver |
| `Q` | Quit |

`1` and `2` are switches - press again to turn them off. Closing the window turns the
redirect off.

After `D` the machine has to restart before the driver is fully gone; the tool
offers it and reminds you on the next start if you skipped it.

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
