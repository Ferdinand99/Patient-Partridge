# Windows: running as a standard user

The PDF engine runs in a sandbox that the app builds when it starts the engine: an AppContainer without capabilities (which is what blocks the network) made from a restricted token, the untrusted integrity level, a job object, process mitigation policies and a desktop of its own (see `crates/engine-client/src/spawn/windows.rs` and [windows-sandbox.md](windows-sandbox.md)). A release build requires the sandbox, so if it could not be built, the engine would refuse every document.

The Windows runners of GitHub run every step as an administrator. Most people who use the app are not administrators, and the token of such a user differs from the one the sandbox was tested with (no Administrators group, a medium integrity level that is not elevated, other defaults on the desktop). The CI job **Windows, as a standard user** (`windows-standard-user` in `.github/workflows/ci.yml`) closes that gap.

## What the job does

All of it runs on `windows-latest`, with read-only permissions and no secrets, after the Build job, and uses the NSIS installer that the Build job made in the same run.

1. `prepare.ps1` makes a local user whose name has a letter outside ASCII (`testbruk` + é + `r`, so the profile is `C:\Users\testbrukér`). The user is only in `Users`. The password is random, made in the job, and masked in the log. The user is removed at the end.
2. `run-as-user.ps1` starts `standard-user-checks.ps1` as that user with `Start-Process -Credential` (the secondary logon service). The process gets a logon of its own and the window station and desktop of the session of the step.
3. `standard-user-checks.ps1`, as that user:
   - prints `whoami /groups` and `whoami /priv`, and checks that the user is not in Administrators, has no Administrators SID in the token (not even deny-only), has a token that is not elevated, runs at medium integrity, and cannot write to the Windows folder or to `HKLM`;
   - installs the NSIS installer with `/S` (a per-user install, in `%LOCALAPPDATA%`) and checks that the program, the engine and `pdfium.dll` are installed;
   - opens a PDF with the installed program (a release build) with `PARTRIDGE_MEASURE_REPORT` set: once at `C:\Users\testbrukér\Mine dokumenter\rapport æøå.pdf` (a space, letters outside ASCII in the folder and file name, and the report written in that profile folder), once at a plain path. A pass is a report with at least one page and the time of the first page: the app writes it when the first page is drawn and closes itself;
   - opens a file that is not a PDF and checks that no first page is reported (a control for the check itself);
   - runs the test programs of `partridge-engine` and `partridge-engine-client` that CI builds for the engine (`sandbox`, `sandbox_refusal`, `engine`, `reader`, `pool`, `hostile_engine`, `hostile_text`), which start the real engine and the self-test program through the code the app uses, with `SandboxPolicy::Required`.

## Which session it runs in

The log says it. On the runners the step runs in session 2, on `WinSta0\Default`, which is visible (an interactive window station), and the checks started with `Start-Process -Credential` run in the same session on the same window station and desktop. The engine of each document runs on a desktop of its own in that window station (`PartridgeEngine<pid>`), which the sandbox tests confirm.

## What this does not prove

- It is a local account on a virtual machine, not a user of a domain, a user whose profile is on a network share, or a computer with group policy, Defender settings, controlled folder access or an application whitelist.
- The user's session is the session of the runner's own logon, not a session the user logged in to. The user has its own token, profile and registry hive, but it shares the screen session with the runner account. A second, separate interactive session (fast user switching, remote desktop) is not tried.
- The installer is the per-user NSIS one. The MSI (per machine) is built and not installed here.
- The release builds are not signed in CI, so SmartScreen is not part of this.
- The report proves that the first page was drawn, not that the page looks right; the browser tests and the engine tests cover that.

## The AppContainer and the files of the checks

The engine runs in an AppContainer, which can open only what is open to its own package or to "all application packages". Two kinds of files are involved here:

- **The installed app** (step 3 of the job). It is installed for the standard user in `%LOCALAPPDATA%`, where the user owns the files and "all application packages" cannot read. The app adds the entry for the engine's package itself, with the rights of the user. This is the case that matters for a real user, and the job checks that a release build of the app, with the engine required to be in its sandbox, draws a first page.
- **The test programs**, built by the administrator into `C:\pp\build`. The standard user cannot change who may read files that the administrator owns, so `prepare.ps1` gives "all application packages" read access to `C:\pp` (what `Program Files` has), and the test programs find the engine there.
