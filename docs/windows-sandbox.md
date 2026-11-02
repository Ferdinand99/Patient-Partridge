# The Windows sandbox of the PDF engine

Every document is read by an engine process of its own. On Windows the app starts that process (`crates/engine-client/src/spawn/windows.rs` and `windows/container.rs`) with the layers below, and the engine checks them when it starts (`crates/engine/src/sandbox/windows.rs`). A release build refuses to open a document if the AppContainer token or any other layer it controls is missing. The one answer it does not insist on is the answer of the network to its test attempts (see "When the network block is not proved").

| Layer | What it does |
|---|---|
| Restricted token | No privileges (the app removes even `SeChangeNotifyPrivilege`, which the usual way of dropping privileges keeps); the user and every group are deny-only; one restricting SID, the null SID, which no object grants access to. |
| AppContainer, no capabilities | The token is an AppContainer token of a package that has no capabilities. Windows refuses the network use of such a process: a TCP connection, the making of a datagram socket, a name lookup, loopback included. |
| No right to open the socket device | Without `SeChangeNotifyPrivilege` the engine cannot open `\Device\Afd\Endpoint`, the device every socket is made from, so no socket of any kind can be made. This is what closes the raw ICMP socket, which the AppContainer alone did not (see below). |
| Untrusted integrity level | Below everything the user owns. |
| Job object | One process, no children, 3 GiB, ended when the app lets go of it, no clipboard or user interface access. |
| Mitigation policies | No dynamic code, no extension points, strict handle checks, and others. |
| Desktop of its own | The engine cannot see the windows of the user or the clipboard. |
| Handle list | The engine inherits exactly its pipes, a null device and the document. |

## Why an AppContainer

A restricted token does not stop the network on Windows: a process with such a token can still make a socket and connect. Blocking the network without administrator rights, an installer step or a signature leaves one mechanism, the AppContainer, which refuses the network access of a token that does not hold a network capability (`internetClient` and others). The engine holds none. No firewall rule is added: it would need administrator rights and would stay behind on the computer. See "What blocks the network" for what is known about how the refusal is made.

The options that were looked at:

- **AppContainer with no capabilities, on top of the restricted token** (what is built). Both access checks apply: an object has to be open to the restricting SID *and* to the package. The restricted token keeps its job: after the engine has locked itself in it cannot open any file, not even its own program, which the package is allowed to read.
- **Less privileged AppContainer.** Takes "all application packages" out of the token, so the engine could reach even less. Not built: the plain AppContainer already blocks the network, and the restricted token already keeps the engine away from files, so it would reduce the reach to objects that only "all application packages" can open, and each start-up problem described below would have to be solved again. It was tried once in CI, by adding the capability `WIN://NOALLAPPPKG` to the token: the engine started and passed the sandbox proof and the start tests, and a raw socket could still be made. So it did not close the raw socket either.
- **A firewall rule** is out, as said above.

## What it does to the other layers

- **Handles from the app.** The document, the system fonts and the pipes are opened by the app and inherited. A handle keeps the access it was opened with, so the AppContainer does not change how they are read.
- **The first thread.** A process with a null-SID restricting token cannot load its own libraries, so the first thread starts out impersonating a copy of the same token that the user's own SIDs restrict. That copy is made an AppContainer token of the same package too (the system lowers an impersonation of another package to "identification" and the loader then fails). It keeps `SeChangeNotifyPrivilege`, so that loading PDFium and the fonts works as before. The engine ends the impersonation with `RevertToSelf` when everything is loaded; the thread then has the token of the process, which has no privilege at all.
- **PDFium.** The library is loaded from its file by the first thread, before the lock-in, as before. For that the package has to read the engine program, the folder of the program, the PDFium folder, `pdfium.dll`, the `VERSION` file and the font files of the `fonts` folder. The app adds a read-and-execute entry for the package to those files (and, without inheritance, to the folders) before it starts an engine. The engine does not ask the mount manager for the real path of the PDFium folder (that device is closed to an AppContainer); it uses the full path it was given.
- **The desktop.** The desktop the engines run on gets an entry for the package; without it the engine cannot start.
- **Pipes and the protocol** are inherited anonymous pipes, so nothing changes.

## The profile is never created

`CreateAppContainerProfile` writes under `HKCU\Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppContainer\Mappings` and creates a folder under `%LOCALAPPDATA%\Packages`, and both would have to be deleted again. The app does not call it. It computes the package SID from a fixed name (`DeriveAppContainerSidFromAppContainerName`) and makes the token with `NtCreateLowBoxToken`, the call Windows itself makes when it starts an AppContainer process. The network refusal looks at the token, not at a registered profile. So the app creates no profile and writes nothing to the registry. (The MSI writes one marker value under `HKLM\Software\Patient Partridge Installer` and removes it again on uninstall.)

The entries on the engine's files are the only trace on disk. They name the package by number, they are written by the user who owns the files (no administrator is needed), they are kept, and they are added again after an update replaces a file. In `Program Files`, where "all application packages" can read already, they are not needed and the app does not change anything it is not allowed to change.

## Installers outside Program Files

A per-machine install in a folder other than `Program Files` (the install page of the MSI lets the user choose one, and deployment tools set `INSTALLDIR`) belongs to the administrators. The user who runs the program cannot change who may read those files, so the app cannot add the entry itself, and the engine could not read `pdfium.dll`: every document failed with the message that the PDF library is missing. So the installers, which run with administrator rights, add the entry: the MSI through custom actions (`src-tauri/wix/package-access.wxs`) and the NSIS installer through a hook (`src-tauri/windows/installer-hooks.nsh`).

The entry is for the package of the engine only (`S-1-15-2-1452645665-1071814252-341713739-2096083442-1079832312-2911523698-3580489556`, which is what the app computes from the fixed name; a test of the client checks that the installer files name that number), read and execute, and only on:

- the install folder, without inheritance, and
- `partridge-engine.exe` and `pdfium.dll`, by name.

Earlier versions of the installers granted "all application packages" read access with inheritance on whatever folder was chosen. A "Change" to `Documents` or to the root of a drive would then have opened everything below it to every AppContainer on the computer (it needs the user's own rights too, so it does not cross users), it would have been inherited by every file there, and uninstall never removed it. Now no other application package gains anything, nothing is inherited, a file of the user in the same folder gets no entry, and the uninstall removes the entry on the folder (the entries on the two files go with the files). A major upgrade removes the old entries before the new files are installed, and the new install adds them again.

The job "Windows, as a standard user" checks this as an administrator, for both installers, before its other steps: it installs into a folder of its own, requires the entry `(RX)` and nothing inherited on the folder, the engine program and the PDFium library, creates a file, a folder and a file in it, and requires that none of them has an entry for the package or for all application packages. It then uninstalls and requires that no entry for the package is left on the folder (which stays because of the file of the user). What this shows is the access list of the files, not an access attempt from a sandbox: the engine's token, after the lock-in, cannot open any file anyway, so an access attempt from it would pass for a reason that has nothing to do with the list. After that the job installs into the folders that the standard user works with, opens a document in each as that user, and does the same with a copy that has no entry, which must fail (docs/windows-standard-user.md).

## Starting reliably

The system's loader loads the libraries a program imports with helper threads of the process. Those threads have only the locked-down token, not the start identity of the first thread, so they cannot open a library file. A library that is not on the system's list of known libraries (the Visual C++ runtime: `vcruntime140.dll` and the `api-ms-win-crt-*` family) has to be opened as a file; with the dynamic runtime, about nine starts in ten of the engine ended with "library not found" (exit code `0xC0000135`). The workspace therefore links the runtime statically on Windows (`.cargo/config.toml`), so the libraries that are left are all on the list. Taking `SeChangeNotifyPrivilege` out of that token did not change this: 0 of 100 sequential starts and 0 of 80 concurrent starts had to be tried again, on both server images, in every firewall state, and as a standard user.

The client also starts the engine again, up to three times in all, when a start ends with that exit code before the greeting, in case a system keeps one more library outside its list. Every attempt has all its restrictions. That second try would hide a start that fails now and then, so `windows_start.rs` counts the starts that had to be tried again (`EngineClient::start_retries`) and requires zero of 100 starts in a row and 80 starts at the same time. The count is in the log of every run.

## What the tests prove

`crates/engine/tests/sandbox.rs` starts the self-test program the way the app starts the engine, hands it a document, and lets it try, after the lock-in, among others:

- a TCP connection to a listener of the test, to an address of the computer that is not the loopback one, to a computer on the Internet (the first IPv4 address of the name used for the lookup, port 443) and to `[::1]`; a datagram from a socket that was bound and from one that was never bound, to the loopback address, to a computer that does not exist (the documentation address 192.0.2.1) and to `[::1]`; a name lookup of an Internet host name; a path on the computer's own non-loopback address and on a computer that does not exist; a named pipe whose access list is open to everybody.
- a raw ICMP socket over IPv4 and a raw ICMPv6 socket, an echo request from a raw socket to the loopback address, to 192.0.2.1 and to 1.1.1.1, a raw IP socket, and sockets of the Unix, Hyper-V and Bluetooth families; the device `\Device\Afd\Endpoint`, which every socket is made from.
- Each is refused. The TCP and datagram attempts fail with `WSAEACCES` (error 10013; access denied, error 5, on IPv6). A socket of every kind fails in `socket` itself, so **no datagram and no echo request is ever sent**: none of the lines shows a refused `sendto`. The log says which call refused. A path on a computer that does not exist is refused at once (0 ms; trying a connection takes seconds), so no connection was tried.
- Each attempt is also made by a copy of the program that is not locked in (the control), and the test requires that it works there and that the listeners saw it. A control that fails fails the test, so a denial is never an attempt that could not work. The listeners must have seen nothing from the sandboxed program. For the raw sockets and the socket families the control must work on the CI runners; on another computer the test says where the control cannot.

The test also requires that the engine names `appcontainer`, `no-privileges` and `no-network` in its greeting, and `windows_start.rs` starts the real engine many times, and many at once.

The engine tests that existed before (open, draw, text, search, fonts, save and read back, the memory limit) run in the same sandbox, and so do the tests of the client.

In CI the checks run on Windows Server 2025 (`windows-latest`) and Windows Server 2022 (`windows-2022`), on the administrator account of the runner, and as a local standard user (the job "Windows, as a standard user", which installs the app for that user and opens documents with it). See [windows-standard-user.md](windows-standard-user.md). **Windows 10 and Windows 11 are not in CI, and have not been tried.** The sandbox proof, `windows_start` and an installed app should be run once by hand on each, as a standard user, before a release.

## What blocks the network

Known, because CI shows it from inside the sandboxed engine and from the control:

- In every state tried (below) a TCP connection, a datagram socket and a name lookup fail, and the engine cannot open `\Device\Afd\Endpoint` (`STATUS_ACCESS_DENIED`). A socket of every family that was tried fails in `socket` with access denied.
- With the privilege `SeChangeNotifyPrivilege` still in the token (an earlier version of this code), the AppContainer still refused TCP and datagram sockets, but a raw ICMP socket could be made, an echo request could be sent from it and, to the loopback address, the reply read. That held on both server images, as the administrator of the runner and as a standard user, and with the less privileged kind of AppContainer. Removing the privilege closed it, together with all other families.

Inferred, from reading Microsoft's network driver (`tcpip.sys`, on Windows 11 and on the Windows 10 line) with Microsoft's public symbols, and not tested here:

- Creating a raw ICMP socket is allowed to every token, whatever its groups, by the design of Windows (the system calls it an unprivileged ping socket): the check that asks for administrator rights is ignored for ICMP and ICMPv6, and the AppContainer check does not look at that kind of socket. That is why the AppContainer could not close it and the missing right to open the device could.
- For the other sockets, the first refusal is an access check of the driver on the endpoint, against a fixed list with no entry that a token with no network capability matches, and not the filtering platform (the part of Windows that also runs its firewall). That fits that no firewall state changed anything, that a datagram socket cannot even be made, and that a TCP connection fails at once.

Not known:

- Whether those refusals depend on the filtering service running. The runners cannot stop it, and no third-party firewall product was tried.
- Whether an echo request that a raw socket sends from an AppContainer token that still has the privilege leaves the computer: see "Packet capture".
- What Windows 10 and Windows 11 do.
- How the name lookup is refused. It fails with `WSANO_RECOVERY` and not with "access denied", so the test shows that a lookup does not work, not which layer stops it.

The previous wording of the README and of this page ("the block rests on the Windows Filtering Platform") was more than is known, and so was the claim that only the server images allow a raw socket.

## Packet capture

PACKET_CAPTURE_RESULT

## When the network block is not proved

The engine tries the network itself when it starts: a TCP connection and a UDP datagram to the loopback address, and a raw ICMP and ICMPv6 socket. Each must fail with "access denied". On a machine where one does not (nothing has produced that so far) the engine does not refuse every document, as an earlier design would have: the fail-closed condition is the AppContainer token (checked by the engine with `TokenIsAppContainer`), the token having no privilege, and the other layers it controls. If the network attempts do not get the answer of a blocked network, the engine still opens the document, names `network-not-proved` instead of `no-network` in its greeting, with what the system answered, and the window shows one calm line (once, it can be dismissed). Nothing is claimed that was not shown. On an ordinary machine the greeting says `no-network` and the window says nothing.

What was tried, on Windows Server 2025 (`windows-latest`) and, where marked, also Windows Server 2022 (`windows-2022`), with the job "Windows, firewall state". The script that makes each state checks that the state was reached and fails the job if it was not:

| State of the runner | What the engine did |
|---|---|
| Default (profiles on, service running) (both) | TCP, UDP and raw sockets refused; `no-network` |
| Firewall profiles switched off (Domain, Private, Public) (both) | The same |
| Default for outgoing traffic set to allow, and a rule that allows all outgoing traffic | The same |
| A rule that allows the engine's package (outgoing and incoming) | The same: a rule for the package does not give it a network capability |
| A loopback exemption for the package (`CheckNetIsolation`) | The same |
| An engine told, in a debug build, to take the answer of its check as one that does not prove the block | The engine opened the document, named `network-not-proved`, and the client and the pool (the part the app asks) reported it; this is the path a state we could not create would take. The window itself (`app.ts`) is tried by the browser tests with the field mocked, not together with the Rust side. |

The firewall service (`MpsSvc`) and the filtering engine (`BFE`) cannot be stopped on a hosted runner (`Access is denied`, to an administrator and to the system account alike), so those two states have no leg in CI; the script has them and fails if the service is still running afterwards. A third-party firewall product registered with Security Center and a VPN with a kill switch have not been tried either.

## What this does not do

- It does not make the app itself safer. The window, the web view and the code that starts the engines run with the user's rights.
- The win32k system calls stay on: PDFium's DLL does not load without them.
- The name lookup fails in the sandbox with `WSANO_RECOVERY` and not with "access denied". It shows that a lookup does not work there, not which layer stops it.
- The engine's own check of the network tries TCP and UDP on IPv4 and raw sockets on IPv4 and IPv6; the tests try the families listed above and an IPv6 connection. No test starts a second engine from inside an engine.
- Windows 10 and Windows 11, a stopped firewall service or filtering engine, and third-party firewall products have not been tried.
