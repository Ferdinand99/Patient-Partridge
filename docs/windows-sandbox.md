# The Windows sandbox of the PDF engine

Every document is read by an engine process of its own. On Windows the app starts that process (`crates/engine-client/src/spawn/windows.rs` and `windows/container.rs`) with the layers below, and the engine checks them when it starts (`crates/engine/src/sandbox/windows.rs`). A release build refuses to open a document if the AppContainer token or any other layer it controls is missing. The one answer it does not insist on is the answer of the network to its test attempts (see "When the network block is not proved").

| Layer | What it does |
|---|---|
| Restricted token | No privileges; the user and every group are deny-only; one restricting SID, the null SID, which no object grants access to. |
| AppContainer, no capabilities | The token is an AppContainer token of a package that has no capabilities. Windows refuses the network use of such a process: a TCP connection, the making of a datagram socket, a name lookup, loopback included. A raw socket is not covered on every account (see below). |
| Untrusted integrity level | Below everything the user owns. |
| Job object | One process, no children, 3 GiB, ended when the app lets go of it, no clipboard or user interface access. |
| Mitigation policies | No dynamic code, no extension points, strict handle checks, and others. |
| Desktop of its own | The engine cannot see the windows of the user or the clipboard. |
| Handle list | The engine inherits exactly its pipes, a null device and the document. |

## Why an AppContainer

A restricted token does not stop the network on Windows: a process with such a token can still make a socket and connect. Blocking the network without administrator rights, an installer step or a signature leaves one mechanism, the AppContainer. The Windows Filtering Platform, the part of Windows that runs the firewall, refuses the network access of an AppContainer token that does not hold a network capability (`internetClient` and others). The engine holds none. No firewall rule is added: it would need administrator rights and would stay behind on the computer.

The options that were looked at:

- **AppContainer with no capabilities, on top of the restricted token** (what is built). Both access checks apply: an object has to be open to the restricting SID *and* to the package. The restricted token keeps its job: after the engine has locked itself in it cannot open any file, not even its own program, which the package is allowed to read.
- **Less privileged AppContainer.** Takes "all application packages" out of the token, so the engine could reach even less. Not built: the plain AppContainer already blocks the network, and the restricted token already keeps the engine away from files, so it would reduce the reach to objects that only "all application packages" can open, and each start-up problem described below would have to be solved again. A debug build can be started in this variant (`PARTRIDGE_TEST_LPAC`) to see what it does; the result is in the next section.
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

## Installers outside Program Files

A per-machine install in a folder other than `Program Files` (the install page of the MSI lets the user choose one, and deployment tools set `INSTALLDIR`) belongs to the administrators. The user who runs the program cannot change who may read those files, so the app cannot add the entry itself, and the engine could not read `pdfium.dll`: every document failed with the message that the PDF library is missing. So the installers, which run with administrator rights, add the entry on the install folder (read and execute for "all application packages", inherited by the files): the MSI through a custom action (`src-tauri/wix/package-access.wxs`) and the NSIS installer through a hook (`src-tauri/windows/installer-hooks.nsh`). The job "Windows, as a standard user" installs both into folders outside Program Files as an administrator, opens a document in each as a standard user, and does the same with a copy that has no entry, which must fail (docs/windows-standard-user.md).

## Starting reliably

The system's loader loads the libraries a program imports with helper threads of the process. Those threads have only the locked-down token, not the start identity of the first thread, so they cannot open a library file. A library that is not on the system's list of known libraries (the Visual C++ runtime: `vcruntime140.dll` and the `api-ms-win-crt-*` family) has to be opened as a file; with the dynamic runtime, about nine starts in ten of the engine ended with "library not found" (exit code `0xC0000135`). The workspace therefore links the runtime statically on Windows (`.cargo/config.toml`), so the libraries that are left are all on the list.

The client also starts the engine again, up to three times in all, when a start ends with that exit code before the greeting, in case a system keeps one more library outside its list. Every attempt has all its restrictions. That second try would hide a start that fails now and then, so `windows_start.rs` counts the starts that had to be tried again (`EngineClient::start_retries`) and requires zero of 100 starts in a row and 80 starts at the same time. The count is in the log of every run.

## What the tests prove

`crates/engine/tests/sandbox.rs` starts the self-test program the way the app starts the engine, hands it a document, and lets it try, after the lock-in, among others:

- a TCP connection to a listener of the test, to an address of the computer that is not the loopback one, to a computer on the Internet (the first IPv4 address of the name used for the lookup, port 443) and to `[::1]`; a datagram from a socket that was bound and from one that was never bound, to the loopback address, to a computer that does not exist (the documentation address 192.0.2.1) and to `[::1]`; a name lookup of an Internet host name; a path on the computer's own non-loopback address and on a computer that does not exist; a named pipe whose access list is open to everybody.
- Each is refused, and the TCP and datagram attempts with `WSAEACCES` (error 10013; access denied, error 5, on IPv6). The refusal of a datagram comes from `socket` itself: **a datagram socket cannot be made in the sandbox**, so no datagram is ever sent and none of the lines shows a refused `sendto`. The log says which call refused. A path on a computer that does not exist is refused at once (0 ms; trying a connection takes seconds), so no connection was tried.
- Each attempt is also made by a copy of the program that is not locked in (the control), and the test requires that it works there and that the listeners saw it. A control that fails fails the test, so a denial is never an attempt that could not work. The listeners must have seen nothing from the sandboxed program.

The test also requires that the engine names `appcontainer` in its greeting, and `windows_start.rs` starts the real engine many times, and many at once.

The engine tests that existed before (open, draw, text, search, fonts, save and read back, the memory limit) run in the same sandbox, and so do the tests of the client.

In CI the checks run on Windows Server 2025 (`windows-latest`) and Windows Server 2022 (`windows-2022`), on the administrator account of the runner, and as a local standard user (the job "Windows, as a standard user", which installs the app for that user and opens documents with it). See [windows-standard-user.md](windows-standard-user.md). **Windows 10 and Windows 11 are not in CI: one manual run on each is still to be made before the release** (it is on the list `before-first-release`: the sandbox proof, `windows_start` and an installed app, as a standard user).

### Raw sockets and ICMP

A raw ICMP socket (`socket(AF_INET, SOCK_RAW, IPPROTO_ICMP)`) is not closed by the AppContainer on the systems CI uses. On both server images, on the administrator account of the runner and also as a local standard user, a raw socket can be made in the sandbox and an echo request can be sent from it, to the loopback address (where the reply comes back to the sandboxed program) and to another address (the control, outside the sandbox, can do the same). One echo request to a computer on the Internet (1.1.1.1) got no reply on the runner, in the sandbox or in the control, so the log does not show that a packet left the computer and was answered; it shows that the system accepted the send. The ICMP packets that fit through a raw socket can carry data, so this is a way out that the AppContainer does not close here. Whether Windows 10 and 11 differ (the documentation says that raw sockets need administrator rights there, and the server images apparently do not ask for them) is for the manual runs to show.

The engine therefore tries it itself, as part of its check: it makes a raw socket and sends one echo request to the loopback address. When that works, the greeting names `network-not-proved` and the window tells the user (next section). The tests require the greeting and the raw socket to agree: `network-not-proved` exactly when a packet can be sent from a raw socket. The ICMP service of the system (`IcmpSendEcho`) could not be tried from inside the sandbox: its library does not start there (`IcmpCreateFile` fails), which says nothing about the network.

## When the network block is not proved

The block is made by the filtering platform of Windows, fed by the firewall. On a machine where the engine's attempts do not get the answer of a blocked network (a third-party firewall product that took over from the Windows firewall, a rule that lets the package out, a damaged filtering service, a raw socket that works), the engine would have refused every document in the earlier design. It does not any more: the fail-closed condition is the AppContainer token (checked by the engine with `TokenIsAppContainer`) and the other layers it controls. If the network attempts do not get the answer of a blocked network, the engine still opens the document, names `network-not-proved` instead of `no-network` in its greeting, with what the system answered, and the window shows one calm line (once, it can be dismissed). Nothing is claimed that was not shown.

What was tried, on Windows Server 2025 (`windows-latest`) and, where marked, also Windows Server 2022 (`windows-2022`), with the job "Windows, firewall state":

| State of the runner | What the engine did |
|---|---|
| Default (profiles on, service running) (both) | TCP and UDP refused with 10013; the raw socket check made the engine say `network-not-proved` because the runner account is an administrator |
| Firewall profiles switched off (Domain, Private, Public) (both) | The same |
| Default for outgoing traffic set to allow, and a rule that allows all outgoing traffic | The same |
| A rule that allows the engine's package (outgoing and incoming) | The same: a rule for the package does not give it a network capability |
| A loopback exemption for the package (`CheckNetIsolation`) | The same |
| Firewall service (`MpsSvc`) or filtering engine (`BFE`) stopped | **Not tried**: the runner refuses to stop either, to an administrator and to the system account alike (`Access is denied`); the log of the job shows each attempt |
| An engine told, in a debug build, to take the answer of its check as one that does not prove the block | The engine opened the document, named `network-not-proved`, the window path works; this is the path a state we could not create would take |
| The less privileged AppContainer, in a debug build | See the log of the job; this variant is not part of the product |

In every state the TCP connection to the loopback address was refused with `WSAEACCES`, so the test's own answer for a blocked network held in each. The states that a hosted runner cannot create (a stopped firewall service or filtering engine, a third-party firewall product registered with Security Center, a VPN with a kill switch) have not been tried. They are on the list `before-first-release`.

## What this does not do

- It does not make the app itself safer. The window, the web view and the code that starts the engines run with the user's rights.
- The win32k system calls stay on: PDFium's DLL does not load without them.
- The name lookup fails in the sandbox with `WSANO_RECOVERY` and not with "access denied". It shows that a lookup does not work there, not which layer stops it.
- The block depends on the filtering platform of Windows working, and on the account: see the two sections above.
- Other socket families (`AF_HYPERV`, `AF_BTH`) are tried by no test.
