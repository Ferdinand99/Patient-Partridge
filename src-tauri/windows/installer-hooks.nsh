; The PDF engine runs in an AppContainer, which can read only what is open to its own package or to
; "all application packages" (S-1-15-2-1). The installer adds that entry on the install folder:
; read and execute, inherited by what is in it. Without it, a program installed by an administrator
; into a folder outside Program Files (a deployment tool, or the folder chosen on the install page)
; could not be read by the engine of a standard user, who cannot change the access list of files
; the administrators own. For a per-user install the user owns the files and the program adds the
; entry itself when it starts; this makes the folder right from the start. A failure is not an
; error of the installation.
!macro NSIS_HOOK_POSTINSTALL
  nsExec::Exec '"$SYSDIR\icacls.exe" "$INSTDIR\." /grant *S-1-15-2-1:(OI)(CI)(RX)'
  Pop $0
!macroend
