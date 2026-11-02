# Windows: running as a standard user

The PDF engine runs in a sandbox that the app builds when it starts the engine: an AppContainer without capabilities (which is what blocks the network) made from a restricted token, the untrusted integrity level, a job object, process mitigation policies and a desktop of its own (see `crates/engine-client/src/spawn/windows.rs` and [windows-sandbox.md](windows-sandbox.md)). A release build requires the sandbox, so if it could not be built, the engine would refuse every document.

The Windows runners of GitHub run every step as an administrator. Most people who use the app are not administrators, and the token of such a user differs from the one the sandbox was tested with (no Administrators group, a medium integrity level that is not elevated, other defaults on the desktop). The CI job **Windows, as a standard user** (`windows-standard-user` in `.github/workflows/ci.yml`) closes that gap.

## What the job does

All of it runs on `windows-latest` and on `windows-2022` (a second Windows version), with read-only permissions and no secrets, after the Build job, and uses the installers that the Build job made in the same run.

1. `prepare.ps1` makes a local user whose name has a letter outside ASCII (`testbruk` + é + `r`, so the profile is `C:\Users\testbrukér`). The user is only in `Users`. The password is random, made in the job, and masked in the log. The user is removed at the end.
2. `install-as-admin.ps1`, as the runner's administrator, installs the app the way an administrator or a deployment tool does, in folders outside Program Files and outside any profile: the MSI with `INSTALLDIR=C:\Apps\pp-msi\`, the NSIS installer with `/D=C:\Apps\pp-nsis`, and a copy of the MSI folder without the entry for the engine's package (`C:\Apps\pp-bare`, a control). The files belong to the administrators.
3. `run-as-user.ps1` starts `standard-user-checks.ps1` as that user with `Start-Process -Credential` (the secondary logon service). The process gets a logon of its own and the window station and desktop of the session of the step.
4. `standard-user-checks.ps1`, which `run-as-user.ps1` starts, runs as that user:
   - prints `whoami /groups` and `whoami /priv`, and checks that the user is not in Administrators, has no Administrators SID in the token (not even deny-only), has a token that is not elevated, runs at medium integrity, and cannot write to the Windows folder or to `HKLM`;
   - installs the NSIS installer with `/S` (a per-user install, in `%LOCALAPPDATA%`) and checks that the program, the engine and `pdfium.dll` are installed;
   - opens a PDF with the installed program (a release build) with `PARTRIDGE_MEASURE_REPORT` set: once at `C:\Users\testbrukér\Mine dokumenter\rapport æøå.pdf` (a space, letters outside ASCII in the folder and file name, and the report written in that profile folder), once at a plain path. A pass is a report with at least one page and the time of the first page: the app writes it when the first page is drawn and closes itself;
   - opens a file that is not a PDF and checks that no first page is reported (a control for the check itself);
   - opens a PDF with each of the three programs installed by the administrator (step 2). The user cannot change the access list of those folders (the check says so). The MSI and the NSIS copy must draw a first page; the copy without the entry must not, which shows that the first two pass because of the entry the installers add, and that the failure this guards against is real;
   - runs the test programs of `partridge-engine` and `partridge-engine-client` that CI builds for the engine (`sandbox`, `sandbox_refusal`, `engine`, `reader`, `pool`, `hostile_engine`, `hostile_text`), which start the real engine and the self-test program through the code the app uses, with `SandboxPolicy::Required`.

## Which session it runs in

The log says it. On the runners the step runs in session 2, on `WinSta0\Default`, which is visible (an interactive window station), and the checks started with `Start-Process -Credential` run in the same session on the same window station and desktop. The engine of each document runs on a desktop of its own in that window station (`PartridgeEngine<pid>`), which the sandbox tests confirm.

## What this does not prove

- It is a local account on a virtual machine, not a user of a domain, a user whose profile is on a network share, or a computer with group policy, Defender settings, controlled folder access or an application whitelist.
- The user's session is the session of the runner's own logon, not a session the user logged in to. The user has its own token, profile and registry hive, but it shares the screen session with the runner account. A second, separate interactive session (fast user switching, remote desktop) is not tried.
- The NSIS installer of this app is a per-user one; it has no per-machine mode. Run by an administrator with a folder given, it leaves the same kind of files as the MSI, which is what the job tries. The MSI is installed by the administrator, with a folder given, but not updated from an older version, and not installed in the default folder in Program Files (where "all application packages" can read already).
- The Windows versions are two server images (`windows-latest` and `windows-2022`). Windows 10 and Windows 11 are not in CI and have not been tried; see `docs/windows-sandbox.md`.
- The release builds are not signed in CI, so SmartScreen is not part of this.
- The report proves that the first page was drawn, not that the page looks right; the browser tests and the engine tests cover that.

## The AppContainer and the files of the checks

The engine runs in an AppContainer, which can open only what is open to its own package or to "all application packages". Two kinds of files are involved here:

- **The installed app** (in the checks of step 4, as that user). It is installed for the standard user in `%LOCALAPPDATA%`, where the user owns the files and "all application packages" cannot read. The app adds the entry for the engine's package itself, with the rights of the user. This is the case that matters for a real user, and the job checks that a release build of the app, with the engine required to be in its sandbox, draws a first page.
- **Programs installed by an administrator outside the profile** (step 2). The standard user who runs them cannot add the entry. The installers add it for the engine's package only (read and execute, without inheritance, on the install folder and on `partridge-engine.exe` and `pdfium.dll`, and on nothing else) and remove the entry on the folder when the program is uninstalled: the MSI through custom actions (`src-tauri/wix/package-access.wxs`), the NSIS installer through a hook (`src-tauri/windows/installer-hooks.nsh`). Step 2 also checks that a file of the user beside the install gets no entry for any application package, and that the uninstall leaves none on the folder. Without the entry no document could be opened there: the engine could not read `pdfium.dll`, and every file failed with the message that the PDF library is missing.
- **The test programs**, built by the administrator into `C:\pp\build`. The standard user cannot change who may read files that the administrator owns, so `prepare.ps1` gives "all application packages" read access to `C:\pp` (what `Program Files` has), and the test programs find the engine there.
