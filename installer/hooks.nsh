; Aimdot installer hooks (Tauri's NSIS bundler, like Echo's src-tauri/windows/hooks.nsh). $UpdateMode / $PassiveMode
; are Tauri's own flags (/UPDATE = the auto-update, /P = passive: progress only). The autostart ("Start with my PC")
; Run key is NOT written here - an installer writing a Run key looks like malware persistence to antivirus
; heuristics (Echo's lesson). The overlay rewrites it with its own path at every start, so after an update or a move
; from the portable zip it points at this copy as soon as Aimdot runs (the updater restarts it with /R).

; Both programs must be closed before their files are replaced: ask the overlay to quit (it saves first), then stop
; whatever still runs from THIS install folder (other copies, e.g. a portable one, are left alone).
!macro AimdotStop
  ${If} ${FileExists} "$INSTDIR\aimdot.exe"
    nsExec::Exec '"$INSTDIR\aimdot.exe" --quit'
    Pop $0
  ${EndIf}
  nsExec::Exec `powershell -NoProfile -ExecutionPolicy Bypass -Command "$$d='$INSTDIR\*'; for($$i=0; $$i -lt 10; $$i++){ $$p = Get-Process aimdot,aimdot-settings -ErrorAction SilentlyContinue | Where-Object { $$_.Path -like $$d }; if(-not $$p){ break }; Start-Sleep -Milliseconds 300; $$p | Stop-Process -Force -ErrorAction SilentlyContinue }"`
  Pop $0
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro AimdotStop
!macroend

; Installs / refreshes the widget for this user (no admin: the MSIX is signed). Its framework packages (.NET Native
; framework + runtime, VC libs) come along in $INSTDIR\widget-deps\ and go in with -DependencyPath: Windows without
; the Store / App Installer (LTSC) can't fetch a missing framework itself - 1.0.3's widget needed .NET Native 2.3 and
; failed there with 0x80073CF3 (11 Nov 2026). If that fails, once more without them (as before). $0 = 0: installed.
!macro AimdotWidgetAdd
  nsExec::Exec `powershell -NoProfile -ExecutionPolicy Bypass -Command "$$m = '$INSTDIR\AimdotWidget.msix'; $$d = @(Get-ChildItem '$INSTDIR\widget-deps' -File -ErrorAction SilentlyContinue | ForEach-Object { $$_.FullName }); try { if ($$d.Count) { Add-AppxPackage -Path $$m -DependencyPath $$d -ForceApplicationShutdown -ErrorAction Stop } else { Add-AppxPackage -Path $$m -ForceApplicationShutdown -ErrorAction Stop }; exit 0 } catch { try { Add-AppxPackage -Path $$m -ForceApplicationShutdown -ErrorAction Stop; exit 0 } catch { exit 1 } }"`
  Pop $0
!macroend

; The Game Bar widget (for exclusive-fullscreen DirectX games) - only in installers that carry the signed MSIX:
; - normal install: ask; yes = install it (AimdotWidgetAdd), App Installer as the fallback
; - auto-update: refresh it only if it is installed already (no question)
!macro NSIS_HOOK_POSTINSTALL
  ${If} ${FileExists} "$INSTDIR\AimdotWidget.msix"
  ${AndIf} $UpdateMode = 1
    nsExec::Exec `powershell -NoProfile -ExecutionPolicy Bypass -Command "if (Get-AppxPackage -Name Resister0.AimdotWidget) { exit 0 } else { exit 1 }"`
    Pop $0
    ${If} $0 = 0
      !insertmacro AimdotWidgetAdd
    ${EndIf}
  ${EndIf}
  ${If} ${FileExists} "$INSTDIR\AimdotWidget.msix"
  ${AndIf} $UpdateMode <> 1
  ${AndIf} $PassiveMode <> 1
  ${AndIfNot} ${Silent}
    MessageBox MB_YESNO|MB_ICONQUESTION "Also install the Aimdot Game Bar widget?$\r$\n$\r$\nYou only need it for games in exclusive fullscreen (DirectX). Borderless and windowed games work without it. You can add it later from the Aimdot folder (AimdotWidget.msix)." IDNO aimdot_no_widget
      ; installed directly (no admin needed, the MSIX is signed): opening the .msix needs Windows' App Installer,
      ; which some Windows editions don't have (LTSC: an "open with" list with nothing to pick, 11 Nov 2026).
      ; App Installer stays the fallback (it can also fetch a missing framework package).
      DetailPrint "Installing the Game Bar widget..."
      !insertmacro AimdotWidgetAdd
      ${If} $0 <> 0
        nsExec::Exec `powershell -NoProfile -ExecutionPolicy Bypass -Command "if (Get-AppxPackage -Name Microsoft.DesktopAppInstaller) { exit 0 } else { exit 1 }"`
        Pop $0
        ${If} $0 = 0
          ExecShell "open" "$INSTDIR\AimdotWidget.msix"
        ${Else}
          MessageBox MB_OK|MB_ICONEXCLAMATION "The Game Bar widget couldn't be installed on this PC. Aimdot itself works; the widget is only needed for exclusive-fullscreen games."
        ${EndIf}
      ${EndIf}
    aimdot_no_widget:
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro AimdotStop
!macroend

; Tauri's "Delete the app data" checkbox removes %APPDATA%\<identifier>; Aimdot's settings + images live in
; %APPDATA%\Aimdot, so remove that too when it is ticked (never during an update). The Game Bar widget stays: it is
; its own app (Settings > Apps > Aimdot Game Bar widget).
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    RmDir /r "$APPDATA\Aimdot"
  ${EndIf}
!macroend
