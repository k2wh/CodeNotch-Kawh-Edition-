; Installer hooks: set the agents up to report to CodeNotch, and put their
; settings back as they were on the way out.
;
; The work is done by the app itself (`--hooks on` / `--hooks off`), so there
; is one implementation of the merge — the installer never edits the user's
; Claude or Codex settings directly. Both run as the installing user, which
; is whose `~/.claude` and `~/.codex` we mean, and both exit immediately with
; no window.

!macro NSIS_HOOK_POSTINSTALL
  DetailPrint "Setting up agent hooks..."
  nsExec::ExecToLog '"$INSTDIR\codenotch.exe" --hooks on'
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  DetailPrint "Removing agent hooks..."
  nsExec::ExecToLog '"$INSTDIR\codenotch.exe" --hooks off'
  Pop $0
!macroend
