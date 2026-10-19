; Optional step at the end of installation: import a cookies.txt.
; Skipped in silent installs. The file is copied for the current user only.

!macro NSIS_HOOK_POSTINSTALL
  IfSilent harmony_cookies_done
  MessageBox MB_YESNO|MB_ICONQUESTION "Optional: import a YouTube cookies.txt now?$\r$\n$\r$\nIt only helps if YouTube blocks playback, but having it is better.$\r$\n$\r$\nExport your OWN file from a browser where you are signed in (a spare account is safest) using a 'Get cookies.txt LOCALLY' style extension. Do not share it with anyone.$\r$\n$\r$\nYou can also do this later: right-click the Harmony icon > Import cookies.txt.$\r$\n$\r$\nChoose a file now?" IDNO harmony_cookies_done
  nsDialogs::SelectFileDialog open "" "Cookies file (*.txt)|*.txt|All files (*.*)|*.*"
  Pop $0
  StrCmp $0 "" harmony_cookies_done
  CreateDirectory "$LOCALAPPDATA\Harmony"
  CopyFiles /SILENT "$0" "$LOCALAPPDATA\Harmony\cookies.txt"
  harmony_cookies_done:
!macroend

; The cookies file is a login session, so it is removed together with the app.
!macro NSIS_HOOK_POSTUNINSTALL
  Delete "$LOCALAPPDATA\Harmony\cookies.txt"
!macroend
