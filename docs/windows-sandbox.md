# The Windows sandbox of the PDF engine

Every document is read by an engine process of its own. On Windows the app starts that process (`crates/engine-client/src/spawn/windows.rs` and `windows/container.rs`) with the layers below, and the engine checks them when it starts (`crates/engine/src/sandbox/windows.rs`). A release build refuses to open a document if any of them is missing.

| Layer | What it does |
|---|---|
| Restricted token | No privileges; the user and every group are deny-only; one restricting SID, the null SID, which no object grants access to. |
| AppContainer, no capabilities | The token is an AppContainer token of a package that has no capabilities. Windows blocks all network use of such a process: connect, send and name lookup, loopback included. |
| Untrusted integrity level | Below everything the user owns. |
| Job object | One process, no children, 3 GiB, ended when the app lets go of it, no clipboard or user interface access. |
| Mitigation policies | No dynamic code, no extension points, strict handle checks, and others. |
| Desktop of its own | The engine cannot see the windows of the user or the clipboard. |
| Handle list | The engine inherits exactly its pipes, a null device and the document. |

## Why an AppContainer

A restricted token does not stop the network on Windows: a process with such a token can still make a socket and connect. Blocking the network without administrator rights, an installer step or a signature leaves one mechanism, the AppContainer. The Windows Filtering Platform, the part of Windows that runs the firewall, refuses every network access of an AppContainer token that does not hold a network capability (`internetClient` and others). The engine holds none. No firewall rule is added: it would need administrator rights and would stay behind on the computer.

The options that were looked at:

- **AppContainer with no capabilities, on top of the restricted token** (what is built). Both access checks apply: an object has to be open to the restricting SID *and* to the package. The restricted token keeps its job: after the engine has locked itself in it cannot open any file, not even its own program, which the package is allowed to read.
- **Less privileged AppContainer.** Takes "all application packages" out of the token, so the engine could reach even less. Not built and not tried: the plain AppContainer already blocks the network, and the restricted token already keeps the engine away from files, so it would add reach-reduction for objects that only "all application packages" can open, at the price of a second round of work on every start-up problem above.
- **A firewall rule** is out, as said above.

## What it does to the other layers

- **Handles from the app.** The document, the system fonts and the pipes are opened by the app and inherited. A handle keeps the access it was opened with, so the AppContainer does not change how they are read.
- **The first thread.** A process with a null-SID restricting token cannot load its own libraries, so the first thread starts out impersonating a copy of the same token that the user's own SIDs restrict. That copy is made an AppContainer token of the same package too (the system lowers an impersonation of another package to "identification" and the loader then fails). The engine ends the impersonation with `RevertToSelf` when everything is loaded.
- **PDFium.** The library is loaded from its file by the first thread, before the lock-in, as before. For that the package has to read the engine program, the folder of the program, the PDFium folder, `pdfium.dll`, the `VERSION` file and the font files of the `fonts` folder. The app adds a read-and-execute entry for the package to those files (and, without inheritance, to the folders) before it starts an engine. The engine does not ask the mount manager for the real path of the PDFium folder (that device is closed to an AppContainer); it uses the full path it was given.
- **The desktop.** The desktop the engines run on gets an entry for the package; without it the engine cannot start.
- **Pipes and the protocol** are inherited anonymous pipes, so nothing changes.

## The profile is never created

`CreateAppContainerProfile` writes under `HKCU\Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppContainer\Mappings` and creates a folder under `%LOCALAPPDATA%\Packages`, and both would have to be deleted again. The app does not call it. It computes the package SID from a fixed name (`DeriveAppContainerSidFromAppContainerName`) and makes the token with `NtCreateLowBoxToken`, the call Windows itself makes when it starts an AppContainer process. The network filters look at the token, not at a registered profile. So nothing is written to the registry and there is nothing to clean up.

The entries on the engine's files are the only trace on disk. They name the package by number, they are written by the user who owns the files (no administrator is needed), they are kept, and they are added again after an update replaces a file. In `Program Files`, where "all application packages" can read already, they are not needed and the app does not change anything it is not allowed to change.

## Starting reliably

The system's loader loads the libraries a program imports with helper threads of the process. Those threads have only the locked-down token, not the start identity of the first thread, so they cannot open a library file. A library that is not on the system's list of known libraries (the Visual C++ runtime: `vcruntime140.dll` and the `api-ms-win-crt-*` family) has to be opened as a file; with the dynamic runtime, about nine starts in ten of the engine ended with "library not found" (exit code `0xC0000135`). The workspace therefore links the runtime statically on Windows (`.cargo/config.toml`), so the libraries that are left are all on the list. In CI 380 starts in a row, two at a time, all worked; with the dynamic runtime the first dozen already failed. The client also starts the engine again, up to three times in all, when a start ends with that exit code before the greeting, in case a system keeps one more library outside its list. Every attempt has all its restrictions.

## What the tests prove

`crates/engine/tests/sandbox.rs` starts the self-test program the way the app starts the engine, hands it a document, and lets it try, after the lock-in:

- a TCP connection to a listener of the test, a UDP datagram to a socket of the test, a TCP connection to an address of the computer that is not the loopback one, and a name lookup of an Internet host name. Each is refused (the first three with `WSAEACCES`, error 10013).
- Each of these is also done by a copy of the program that is not locked in (the control), and the test requires that it works there and that the listeners saw it. A control that fails fails the test, so a denial is never an attempt that could not work.
- The listeners must have seen nothing from the sandboxed program.

The test also requires that the engine names `appcontainer` and `no-network` in its greeting, and `windows_start.rs` starts the real engine many times, and many at once.

The engine tests that existed before (open, draw, text, search, fonts, save and read back, the memory limit) run in the same sandbox, and so do the tests of the client.

In CI the checks run twice: on the administrator account of the runner, and as a local standard user (the job "Windows, as a standard user", which installs the app for that user and opens documents with it). See [windows-standard-user.md](windows-standard-user.md).

## What this does not do

- It does not make the app itself safer. The window, the web view and the code that starts the engines run with the user's rights.
- The win32k system calls stay on: PDFium's DLL does not load without them.
- The name lookup fails in the sandbox with `WSANO_RECOVERY` and not with "access denied". It shows that a lookup does not work there, not which layer stops it.
- The block depends on the filtering platform of Windows working. Without it the engine's check does not get "access denied" and the engine does not run.
