# Security

InputRedirect asks Windows to accept your typing and your clicks from a driver
rather than from your keyboard and mouse. There is no way to do that from user
mode, so the program carries a signed kernel driver and installs it. That single
fact is what this document is about: it is the largest thing InputRedirect does
to a computer, and it is larger than the program itself.

Read this before the first run. The program asks the same question on screen and
will not install anything until it is answered.

## What running this program does to the machine

Every one of these is done deliberately and can be read in the source.

- **It requires administrator rights.** Without them it refuses to start. Not a
  convenience: adding a driver to the driver store and creating a device node
  cannot be done otherwise.
- **It installs three kernel-mode drivers.** `logi_joy_bus_enum.sys`,
  `logi_joy_xlcore.sys` and `logi_joy_vir_hid.sys`, build `2021.1.1365.0`,
  written by Logitech and shipped with Logitech G HUB. They are embedded in the
  executable, written out to a temporary folder and handed to `pnputil`, which
  copies them into the Windows driver store.
- **It registers three kernel services** under those names. They are
  demand-start, not boot-start, but the device node they attach to persists, so
  plug and play loads them whenever Windows enumerates it - which is every
  start, not only while this program runs.
- **It creates a device node**, `root\LGHUBVirtualBus`, and binds the bus driver
  to it. The virtual keyboard and mouse are children of that node.
- **It installs low-level input hooks** (`WH_KEYBOARD_LL`, `WH_MOUSE_LL`) for as
  long as it runs. Every key and button on the machine passes through this
  process while it is up.
- **It closes Logitech G HUB** and stops `LGHUBUpdaterService` for the duration
  of the session, and restarts them not at all - G HUB claims the same two
  virtual devices, and a product id can only be claimed once. Profiles, macros
  and lighting are G HUB's own files and are untouched.
- **It writes one registry value**, `HKLM\SOFTWARE\InputRedirect\RestartPending`,
  to remember that a removal is waiting on a reboot.

Everything except the driver goes away when the program exits. The driver does
not: it stays until it is removed, whether or not this program is ever run
again.

## What that costs you

This is the part worth arguing about, and it does not depend on the quality of
the Rust in this repository.

**A program that brings its own signed kernel driver is a pattern the security
industry has a name for: bring your own vulnerable driver.** It is not an
accusation against these particular files. It is a description of the shape, and
the shape has consequences whatever the driver turns out to be.

- **Memory Integrity may stop working.** These driver builds are known to block
  Core Isolation / Memory Integrity (HVCI); Logitech publishes a support note
  about it. If you have Memory Integrity on, Windows may turn it off or refuse
  to turn it back on while the driver is installed. That is a hardware-backed
  kernel protection, and it is not a small thing to give up.
- **Anti-cheat software looks for exactly this driver.** Protected games detect
  it by name and may refuse to run. This is the documented reason InputRedirect
  does not work in some games.
- **A blocklist entry would end it.** If Microsoft adds these builds to the
  Vulnerable Driver Blocklist, Windows stops loading them and the program stops
  working entirely. Nothing in this repository can prevent that.
- **The attack surface of the machine grows.** Three more drivers in the kernel
  are three more drivers in the kernel. Any weakness in them is reachable on
  your machine once they are installed, by anything running on it, not only by
  this program.
- **Reports of bugchecks exist.** These drivers appear in third-party reports of
  `HIDCLASS.SYS` bugchecks. Not reproduced here; recorded because it would be
  dishonest to leave it out.

## Where not to run this

Do not install it on a machine you do not own outright, or on one where the
consequences above are somebody else's to accept:

- managed or corporate machines, including anything joined to a domain or
  enrolled in Intune;
- machines that have to stay compliant with a hardening baseline, or where
  Memory Integrity, Credential Guard or an EDR agent is a requirement rather
  than a default;
- shared machines, and machines used by anyone who has not read this page;
- anything where a blocked driver, a failed anti-cheat check or an unexplained
  bugcheck would matter.

InputRedirect is a tool for a machine its owner is willing to experiment on.
It is not enterprise software and is not built to be.

## What the program does not do

Stated because "it installs a kernel driver" invites worse guesses:

- **No network access.** The program makes no outbound connections, has no
  update check and sends no telemetry. Its whole dependency list is `bitflags`,
  `tempfile`, `thiserror`, `windows` and `embed-resource`, none of which speaks
  to the network.
- **No keystroke logging.** Input passes through the hooks and is forwarded to
  the virtual device. Nothing is written to disk and nothing is kept beyond the
  two counters on screen.
- **No autostart, no service of its own, no scheduled task.** Closing the window
  ends it. The only thing that survives is the driver.
- **No modification of the driver bytes.** The files are embedded verbatim,
  because a single changed byte would break the signature in the catalogue and
  Windows would refuse to load them.

## Supply chain

The driver files in `drivers/` are Logitech's, unmodified. CI checks, on every
push and on every release:

- the folder holds exactly the seven expected files;
- each catalogue carries a valid Authenticode signature from the Windows
  Hardware Compatibility Publisher, which is how a kernel driver on x64 is
  signed - the vendor submits the package and Microsoft signs it, so the signer
  never names the vendor;
- each `.inf` names Logitech as the vendor, which is where the ownership of the
  package is actually stated;
- `cargo audit` reads `Cargo.lock` against the RustSec advisory database.

CI also prints the SHA-256 of each driver file. Those hashes are printed rather
than compared against a recorded list, so a change to them is visible in a run
log but is not yet enforced by a check.

## Removing everything

Press `D` in the menu. The program unplugs the virtual devices, stops the
services, deletes both packages from the driver store and offers a restart.
**The restart is not optional:** until it happens Windows keeps the old driver
images loaded, so the removal is only half done and the program cannot work.
After the restart the machine is in the state it was in before the first run.

If the removal fails, the program names the processes holding the driver files
open. That is usually G HUB or the kernel itself, and a restart followed by an
immediate second attempt is the answer.

## Reporting a vulnerability

Open a [security advisory](https://github.com/kerogenesis/InputRedirect/security/advisories/new)
rather than a public issue. Useful things to include: the Windows build, whether
Memory Integrity was on, and which of the steps above the machine got to.

Two classes of report will be closed as working as intended, because they are
this program's design and are documented above: that it installs a third-party
kernel driver, and that it hooks all keyboard and mouse input while it runs. A
report that one of those does something beyond what this page describes is very
much wanted.
