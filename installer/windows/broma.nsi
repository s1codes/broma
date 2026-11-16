; Broma Windows Installer Script (NSIS 3 / MUI2)
;
; Build via PowerShell helper:
;   .\installer\windows\build-installer.ps1
;
; Or directly with makensis from installer\windows\ (x64 default):
;   makensis -DVERSION=0.1.0 broma.nsi

Unicode True
ManifestDPIAware true
SetCompressor /SOLID lzma

!ifndef VERSION
  !define VERSION "0.1.0"
!endif

!ifndef RUST_TARGET
  !define RUST_TARGET "x86_64-pc-windows-msvc"
!endif

!ifndef BIN_PATH
  !define BIN_PATH "..\..\target\${RUST_TARGET}\release\broma.exe"
!endif

!ifndef ARCH_SUFFIX
  !define ARCH_SUFFIX "x64"
!endif

!ifndef OUT_FILE
  !define OUT_FILE "Broma-Setup-${VERSION}-${ARCH_SUFFIX}.exe"
!endif

!ifndef APP_ICON_PATH
  !define APP_ICON_PATH "..\..\assets\broma.ico"
!endif

!define APP_NAME "Broma"
!define APP_PUBLISHER "Broma"
!define APP_EXE "broma.exe"
!define APP_ICON "broma.ico"
!define INSTALL_REG_KEY "Software\Broma"
!define UNINSTALL_REG_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Broma"

;--------------------------------
; General Metadata & Output
;--------------------------------
Name "${APP_NAME} ${VERSION}"
OutFile "${OUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\Broma"
InstallDirRegKey HKCU "${INSTALL_REG_KEY}" "InstallDir"
RequestExecutionLevel user

;--------------------------------
; Includes
;--------------------------------
!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "LogicLib.nsh"

;--------------------------------
; Modern UI Configuration
;--------------------------------
!define MUI_ABORTWARNING
!define MUI_ICON "${APP_ICON_PATH}"
!define MUI_UNICON "${APP_ICON_PATH}"

!define MUI_FINISHPAGE_RUN "$INSTDIR\${APP_EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "Launch ${APP_NAME}"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

;--------------------------------
; Helpers
;--------------------------------
; Gracefully ask a running Broma instance (e.g. minimized in the tray) to exit,
; wait briefly, and force-terminate if it is still holding broma.exe open.
!macro StopRunningBroma
  DetailPrint "Checking for running ${APP_NAME} instances..."
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /IM ${APP_EXE}'
  Pop $0
  ${If} $0 == 0
    Sleep 750
    nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /F /T /IM ${APP_EXE}'
    Pop $0
    Sleep 250
  ${EndIf}
!macroend

;--------------------------------
; Install Section
;--------------------------------
Section "Install" SecMain
  SetShellVarContext current

  !insertmacro StopRunningBroma

  SetOutPath "$INSTDIR"
  File /oname=${APP_EXE} "${BIN_PATH}"
  File /oname=${APP_ICON} "${APP_ICON_PATH}"

  WriteUninstaller "$INSTDIR\Uninstall.exe"

  ; Shortcuts (working directory is $INSTDIR from SetOutPath above)
  CreateDirectory "$SMPROGRAMS\${APP_NAME}"
  CreateShortcut "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\${APP_ICON}" 0
  CreateShortcut "$DESKTOP\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\${APP_ICON}" 0

  ; Persist install directory for future upgrades
  WriteRegStr HKCU "${INSTALL_REG_KEY}" "InstallDir" "$INSTDIR"

  ; Windows Installed Apps / Add-Remove Programs entry (per-user)
  WriteRegStr HKCU "${UNINSTALL_REG_KEY}" "DisplayName" "${APP_NAME}"
  WriteRegStr HKCU "${UNINSTALL_REG_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_REG_KEY}" "Publisher" "${APP_PUBLISHER}"
  WriteRegStr HKCU "${UNINSTALL_REG_KEY}" "DisplayIcon" "$INSTDIR\${APP_ICON}"
  WriteRegStr HKCU "${UNINSTALL_REG_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_REG_KEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr HKCU "${UNINSTALL_REG_KEY}" "QuietUninstallString" '"$INSTDIR\Uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINSTALL_REG_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_REG_KEY}" "NoRepair" 1

  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKCU "${UNINSTALL_REG_KEY}" "EstimatedSize" "$0"
SectionEnd

;--------------------------------
; Uninstall Section
;--------------------------------
Section "Uninstall"
  SetShellVarContext current

  !insertmacro StopRunningBroma

  ; Remove installed files and install directory (only if empty)
  Delete "$INSTDIR\${APP_EXE}"
  Delete "$INSTDIR\${APP_ICON}"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"

  ; Remove shortcuts
  Delete "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk"
  RMDir "$SMPROGRAMS\${APP_NAME}"
  Delete "$DESKTOP\${APP_NAME}.lnk"

  ; Remove registry keys
  DeleteRegKey HKCU "${UNINSTALL_REG_KEY}"
  DeleteRegKey HKCU "${INSTALL_REG_KEY}"

  ; Per AGENTS.md §7 & §13: directories::ProjectDirs::from("com", "broma", "Broma")
  ; stores user data in $LOCALAPPDATA\broma\Broma\data. Leave it intact by default
  ; so upgrades/reinstalls never wipe soundboard libraries accidentally.
  IfSilent skip_data_prompt
    MessageBox MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON2 \
      "Do you also want to delete your Broma soundboard library, imported clips, and settings?$\r$\n$\r$\nLocation: $LOCALAPPDATA\broma\Broma$\r$\n$\r$\nSelect No to keep your sounds for future reinstalls." \
      IDNO skip_data_prompt
    RMDir /r "$LOCALAPPDATA\broma\Broma"
    RMDir "$LOCALAPPDATA\broma"
  skip_data_prompt:
SectionEnd
