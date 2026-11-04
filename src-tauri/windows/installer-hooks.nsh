; The PDF engine runs in an AppContainer, which can read only what is open to its own package. The
; installer adds an entry for the package of the engine (S-1-15-2-..., computed from the name
; "Partridge.Engine.Sandbox"; a test of the engine client checks that it is the one the app uses),
; and only that: read and execute on the install folder, without inheritance, and on the two files
; the engine opens by path, partridge-engine.exe and pdfium.dll. Nothing else in the folder gets an
; entry and no other application package gains anything, also when the folder was typed in (/D=) and
; has other files in it.
;
; Without it, a program installed by an administrator into a folder outside Program Files (a
; deployment tool, or the folder given on the command line) could not be read by the engine of a
; standard user, who cannot change the access list of files the administrators own. For a per-user
; install the user owns the files and the program adds the entries itself when it starts; this
; makes the folder right from the start. The entry on the folder is removed again on uninstall (the
; entries on the files go with the files). A failure is not an error of the installation.
!macro NSIS_HOOK_POSTINSTALL
  nsExec::Exec '"$SYSDIR\icacls.exe" "$INSTDIR\." /grant *S-1-15-2-1452645665-1071814252-341713739-2096083442-1079832312-2911523698-3580489556:(RX)'
  Pop $0
  nsExec::Exec '"$SYSDIR\icacls.exe" "$INSTDIR\partridge-engine.exe" /grant *S-1-15-2-1452645665-1071814252-341713739-2096083442-1079832312-2911523698-3580489556:(RX)'
  Pop $0
  nsExec::Exec '"$SYSDIR\icacls.exe" "$INSTDIR\partridge-signer.exe" /grant *S-1-15-2-1452645665-1071814252-341713739-2096083442-1079832312-2911523698-3580489556:(RX)'
  Pop $0
  nsExec::Exec '"$SYSDIR\icacls.exe" "$INSTDIR\pdfium.dll" /grant *S-1-15-2-1452645665-1071814252-341713739-2096083442-1079832312-2911523698-3580489556:(RX)'
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::Exec '"$SYSDIR\icacls.exe" "$INSTDIR\." /remove *S-1-15-2-1452645665-1071814252-341713739-2096083442-1079832312-2911523698-3580489556'
  Pop $0
!macroend
