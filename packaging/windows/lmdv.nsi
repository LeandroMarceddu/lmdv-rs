; lmdv Windows installer (NSIS 3 / Modern UI 2).
;
; Build from the repository root, for example:
;   makensis /DVERSION=0.6.0 ^
;     /DSOURCE="%CD%\target\release\lmdv.exe" ^
;     /DICON="%CD%\assets\lmdv.ico" ^
;     /DOUTDIR="%CD%\dist" packaging\windows\lmdv.nsi
;
; The CI release workflow passes absolute paths for all four defines.

Unicode true
!include "MUI2.nsh"

!ifndef VERSION
  !define VERSION "0.6.0"
!endif
!ifndef SOURCE
  !define SOURCE "..\..\target\release\lmdv.exe"
!endif
!ifndef ICON
  !define ICON "..\..\assets\lmdv.ico"
!endif
!ifndef OUTDIR
  !define OUTDIR "..\..\dist"
!endif

Name "lmdv"
Caption "lmdv ${VERSION} Setup"
OutFile "${OUTDIR}\lmdv-${VERSION}-setup.exe"
InstallDir "$LOCALAPPDATA\Programs\lmdv"
InstallDirRegKey HKCU "Software\lmdv" "InstallDir"
RequestExecutionLevel user
BrandingText "lmdv"
SetCompressor /SOLID lzma
XPStyle on
ManifestDPIAware true

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "lmdv"
VIAddVersionKey "FileDescription" "lmdv installer"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "LegalCopyright" "MIT"

!define MUI_ABORTWARNING
!define MUI_ICON "${ICON}"
!define MUI_UNICON "${ICON}"
!define MUI_FINISHPAGE_RUN "$INSTDIR\lmdv.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Launch lmdv"
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Function .onInit
  SetShellVarContext current
FunctionEnd

Function un.onInit
  SetShellVarContext current
FunctionEnd

Section "lmdv (required)" SEC_MAIN
  SectionIn RO
  SetOutPath "$INSTDIR"
  File "${SOURCE}"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  CreateDirectory "$SMPROGRAMS\lmdv"
  CreateShortCut "$SMPROGRAMS\lmdv\lmdv.lnk" "$INSTDIR\lmdv.exe"
  CreateShortCut "$SMPROGRAMS\lmdv\Uninstall lmdv.lnk" "$INSTDIR\uninstall.exe"

  WriteRegStr HKCU "Software\lmdv" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\lmdv" "DisplayName" "lmdv"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\lmdv" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\lmdv" "Publisher" "lmdv"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\lmdv" "DisplayIcon" "$INSTDIR\lmdv.exe,0"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\lmdv" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\lmdv" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\lmdv" "InstallLocation" "$INSTDIR"
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\lmdv" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\lmdv" "NoRepair" 1
SectionEnd

Section /o "Desktop shortcut" SEC_DESKTOP
  CreateShortCut "$DESKTOP\lmdv.lnk" "$INSTDIR\lmdv.exe"
SectionEnd

Section "Associate .md files with lmdv" SEC_ASSOC
  ; Per-user default association plus Open with / Default Apps registration.
  ; Windows 10/11 may keep an existing per-user UserChoice (set in Settings >
  ; Default apps) above this; the app is still registered and can be picked
  ; there. We never delete the user's UserChoice key.
  ReadRegStr $0 HKCU "Software\Classes\.md" ""
  StrCmp $0 "lmdv.md" md_keep_previous
    WriteRegStr HKCU "Software\lmdv" "PreviousMdProgId" "$0"
  md_keep_previous:

  WriteRegStr HKCU "Software\Classes\lmdv.md" "" "Markdown document"
  WriteRegStr HKCU "Software\Classes\lmdv.md" "FriendlyTypeName" "Markdown document"
  WriteRegStr HKCU "Software\Classes\lmdv.md\DefaultIcon" "" "$INSTDIR\lmdv.exe,0"
  WriteRegStr HKCU "Software\Classes\lmdv.md\shell\open\command" "" '"$INSTDIR\lmdv.exe" "%1"'
  WriteRegStr HKCU "Software\Classes\lmdv.md\shell\edit\command" "" '"$INSTDIR\lmdv.exe" "%1"'
  WriteRegStr HKCU "Software\Classes\.md" "" "lmdv.md"
  WriteRegStr HKCU "Software\Classes\.md" "Content Type" "text/markdown"
  WriteRegStr HKCU "Software\Classes\.md\OpenWithProgids" "lmdv.md" ""
  WriteRegStr HKCU "Software\Classes\Applications\lmdv.exe" "FriendlyAppName" "lmdv"
  WriteRegStr HKCU "Software\Classes\Applications\lmdv.exe\shell\open\command" "" '"$INSTDIR\lmdv.exe" "%1"'
  WriteRegStr HKCU "Software\Classes\Applications\lmdv.exe\SupportedTypes" ".md" ""
  WriteRegStr HKCU "Software\RegisteredApplications" "lmdv" "Software\lmdv\Capabilities"
  WriteRegStr HKCU "Software\lmdv\Capabilities" "ApplicationName" "lmdv"
  WriteRegStr HKCU "Software\lmdv\Capabilities" "ApplicationDescription" "Native Markdown viewer and editor"
  WriteRegStr HKCU "Software\lmdv\Capabilities" "ApplicationIcon" "$INSTDIR\lmdv.exe,0"
  WriteRegStr HKCU "Software\lmdv\Capabilities\FileAssociations" ".md" "lmdv.md"
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, i 0, i 0)'
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_MAIN} "lmdv, the native Markdown viewer and editor."
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_DESKTOP} "Add a shortcut on the desktop."
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_ASSOC} "Make lmdv the default app for .md files and register it in Open with / Default apps."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section "Uninstall"
  Delete "$INSTDIR\lmdv.exe"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  Delete "$SMPROGRAMS\lmdv\lmdv.lnk"
  Delete "$SMPROGRAMS\lmdv\Uninstall lmdv.lnk"
  RMDir "$SMPROGRAMS\lmdv"
  Delete "$DESKTOP\lmdv.lnk"

  ; Restore the previous .md default only when it is still ours; if the user
  ; picked another app after installing, leave that choice alone.
  ReadRegStr $0 HKCU "Software\Classes\.md" ""
  StrCmp $0 "lmdv.md" 0 md_done
    ReadRegStr $1 HKCU "Software\lmdv" "PreviousMdProgId"
    StrCmp $1 "" 0 md_restore
      DeleteRegValue HKCU "Software\Classes\.md" ""
      Goto md_done
    md_restore:
      WriteRegStr HKCU "Software\Classes\.md" "" "$1"
  md_done:

  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\lmdv"
  DeleteRegKey HKCU "Software\lmdv\Capabilities"
  DeleteRegKey HKCU "Software\lmdv"
  DeleteRegKey HKCU "Software\Classes\lmdv.md"
  DeleteRegKey HKCU "Software\Classes\Applications\lmdv.exe"
  DeleteRegValue HKCU "Software\Classes\.md\OpenWithProgids" "lmdv.md"
  DeleteRegValue HKCU "Software\RegisteredApplications" "lmdv"
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, i 0, i 0)'
SectionEnd
