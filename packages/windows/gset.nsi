; GSET Windows installer.
;
; Builds a single-file .exe installer with NSIS 3. Two install scopes, default
; to per-machine:
;
;   * "Install for all users" (default)  -> %ProgramFiles%\GSET, system PATH,
;     HKLM uninstall key. Requires elevation; the installer re-launches itself
;     elevated when the chosen mode needs an administrator.
;   * "Install just for me"               -> %LOCALAPPDATA%\Programs\GSET, user
;     PATH, HKCU uninstall key.
;
; The script reads its inputs from defines so CI can stamp it per release:
;
;   makensis /DVERSION=3.3.0 /DBINARY=...\gset.exe /DLICENSE=...\LICENSE \
;            /DICON=...\gset.ico /DOUT=...\gset-3.3.0-setup.exe gset.nsi
;
; Every define has a sensible default so the script also compiles unmodified
; when the files sit next to it.

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"
!include "nsDialogs.nsh"
!include "StrFunc.nsh"
${StrStr}
${UnStrRep}

; --------------------------------------------------------------------------
; Identity (all overridable from the command line)
; --------------------------------------------------------------------------
!define /IfNDef PRODUCTNAME "GSET"
!define /IfNDef PUBLISHER   "GSET Team"
!define /IfNDef WEBSITE     "https://github.com/Crazygiscool/GSETLang"
!define /IfNDef VERSION     "3.3.0"
!define /IfNDef UNINST_KEY  "Software\Microsoft\Windows\CurrentVersion\Uninstall\GSET"

; File inputs. Defaults resolve relative to this script's directory.
!macro GSET_DEFAULT name value
  !ifndef ${name}
    !define ${name} "${__FILEDIR__}\${value}"
  !endif
!macroend
!insertmacro GSET_DEFAULT BINARY  "gset.exe"
!insertmacro GSET_DEFAULT LICENSE "LICENSE"
!insertmacro GSET_DEFAULT ICON    "gset.ico"
!insertmacro GSET_DEFAULT OUT     "gset-${VERSION}-setup.exe"

Name "${PRODUCTNAME} ${VERSION}"
OutFile "${OUT}"
Unicode true
RequestExecutionLevel user ; re-launched elevated for the all-users scope
InstallDir "$PROGRAMFILES64\${PRODUCTNAME}"

; --------------------------------------------------------------------------
; Interface
; --------------------------------------------------------------------------
!define MUI_ICON     "${ICON}"
!define MUI_UNICON   "${ICON}"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_NOAUTOCLOSE

!insertmacro MUI_PAGE_WELCOME

; The mode choice must happen before the directory and license pages.
Page custom PageModeCreate PageModeLeave

!insertmacro MUI_PAGE_LICENSE "${LICENSE}"

!define MUI_PAGE_CUSTOMFUNCTION_PRE SetInstallDirDefault
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

; --------------------------------------------------------------------------
; State
; --------------------------------------------------------------------------
Var RegisterAllUsers   ; 1 = all users (default), 0 = just me
Var ElevatedHere       ; 1 = this process was re-launched elevated
Var ModeDialog
Var RadioAll
Var RadioUser

; --------------------------------------------------------------------------
; Entry point
; --------------------------------------------------------------------------
Function .onInit
  StrCpy $RegisterAllUsers 1
  StrCpy $ElevatedHere 0
  ${GetParameters} $0
  ${GetOptions} $0 "/ALLUSERS" $1
  ${IfNot} ${Errors}
    StrCpy $RegisterAllUsers 1
    StrCpy $ElevatedHere 1
  ${EndIf}
  ${GetOptions} $0 "/CURRENTUSER" $1
  ${IfNot} ${Errors}
    StrCpy $RegisterAllUsers 0
  ${EndIf}
FunctionEnd

; Skips the mode page in the elevated re-launch (the choice was already made)
; and when running silently.
Function PageModeCreate
  ${If} $ElevatedHere == 1
    Abort
  ${EndIf}
  nsDialogs::Create 1018
  Pop $ModeDialog
  ${If} $ModeDialog == error
    Abort
  ${EndIf}
  ${NSD_CreateLabel}      0 0u   100% 20u "Choose how ${PRODUCTNAME} ${VERSION} will be installed:"
  Pop $0
  ${NSD_CreateRadioButton} 0 30u 100% 16u "Install for all users (recommended, requires administrator)"
  Pop $RadioAll
  ${NSD_CreateRadioButton} 0 56u 100% 16u "Install just for me"
  Pop $RadioUser
  ${If} $RegisterAllUsers == 1
    ${NSD_Check} $RadioAll
  ${Else}
    ${NSD_Check} $RadioUser
  ${EndIf}
  nsDialogs::Show
FunctionEnd

Function PageModeLeave
  ${NSD_GetState} $RadioAll $0
  ${If} $0 == 1
    StrCpy $RegisterAllUsers 1
  ${Else}
    StrCpy $RegisterAllUsers 0
  ${EndIf}

  ; Elevate when all-users was requested from a non-admin process.
  ${If} $RegisterAllUsers == 1
  ${AndIf} $ElevatedHere == 0
    UserInfo::GetAccountType
    Pop $0
    ${If} $0 != "admin"
      MessageBox MB_ICONINFORMATION|MB_OK \
        "${PRODUCTNAME} needs administrator rights to install for all users.$\r$\n\
         Setup will restart with elevated rights."
      ExecShell "runas" "$EXEPATH" "/ALLUSERS"
      Quit
    ${EndIf}
  ${EndIf}
FunctionEnd

; The install directory default depends on the chosen scope. A previous
; install of the same scope is remembered and preferred.
Function SetInstallDirDefault
  StrCpy $1 ""
  ${If} $RegisterAllUsers == 1
    ReadRegStr $1 HKLM "${UNINST_KEY}" "InstallLocation"
    ${If} $1 == ""
      StrCpy $INSTDIR "$PROGRAMFILES64\${PRODUCTNAME}"
    ${Else}
      StrCpy $INSTDIR $1
    ${EndIf}
  ${Else}
    ReadRegStr $1 HKCU "${UNINST_KEY}" "InstallLocation"
    ${If} $1 == ""
      StrCpy $INSTDIR "$LOCALAPPDATA\Programs\${PRODUCTNAME}"
    ${Else}
      StrCpy $INSTDIR $1
    ${EndIf}
  ${EndIf}
FunctionEnd

; --------------------------------------------------------------------------
; Installer
; --------------------------------------------------------------------------
Section "Install"
  SectionIn RO

  SetOutPath "$INSTDIR"
  File "${BINARY}"
  File "${LICENSE}"

  WriteUninstaller "$INSTDIR\Uninstall ${PRODUCTNAME}.exe"
  SetRegView 64

  ${If} $RegisterAllUsers == 1
    WriteRegStr HKLM "${UNINST_KEY}" "DisplayName"      "${PRODUCTNAME}"
    WriteRegStr HKLM "${UNINST_KEY}" "DisplayVersion"   "${VERSION}"
    WriteRegStr HKLM "${UNINST_KEY}" "DisplayIcon"      "$INSTDIR\gset.exe"
    WriteRegStr HKLM "${UNINST_KEY}" "Publisher"        "${PUBLISHER}"
    WriteRegStr HKLM "${UNINST_KEY}" "InstallLocation"  "$INSTDIR"
    WriteRegStr HKLM "${UNINST_KEY}" "URLInfoAbout"     "${WEBSITE}"
    WriteRegStr HKLM "${UNINST_KEY}" "UninstallString"  '"$INSTDIR\Uninstall ${PRODUCTNAME}.exe"'
    WriteRegStr HKLM "${UNINST_KEY}" "InstallMode"      "all-users"
    WriteRegDWORD HKLM "${UNINST_KEY}" "NoModify"       1
    WriteRegDWORD HKLM "${UNINST_KEY}" "NoRepair"       1
    Call AddToSystemPath
  ${Else}
    WriteRegStr HKCU "${UNINST_KEY}" "DisplayName"      "${PRODUCTNAME}"
    WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion"   "${VERSION}"
    WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon"      "$INSTDIR\gset.exe"
    WriteRegStr HKCU "${UNINST_KEY}" "Publisher"        "${PUBLISHER}"
    WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation"  "$INSTDIR"
    WriteRegStr HKCU "${UNINST_KEY}" "URLInfoAbout"     "${WEBSITE}"
    WriteRegStr HKCU "${UNINST_KEY}" "UninstallString"  '"$INSTDIR\Uninstall ${PRODUCTNAME}.exe"'
    WriteRegStr HKCU "${UNINST_KEY}" "InstallMode"      "just-me"
    WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify"       1
    WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair"       1
    Call AddToUserPath
  ${EndIf}

  ; Start menu entries.
  CreateDirectory "$SMPROGRAMS\${PRODUCTNAME}"
  CreateShortcut "$SMPROGRAMS\${PRODUCTNAME}\${PRODUCTNAME}.lnk" \
                 "$INSTDIR\gset.exe"
  CreateShortcut "$SMPROGRAMS\${PRODUCTNAME}\Uninstall ${PRODUCTNAME}.lnk" \
                 "$INSTDIR\Uninstall ${PRODUCTNAME}.exe"

  Call BroadcastEnvironmentChange
SectionEnd

; Appends $INSTDIR to the system PATH (REG_EXPAND_SZ) if it is not already
; there, keeping the value expand-safe so %SystemRoot% mentions survive.
Function AddToSystemPath
  ReadRegStr $1 HKLM "SYSTEM\CurrentControlSet\Control\Session Manager\Environment" "Path"
  ${StrStr} $2 $1 "$INSTDIR"
  ${If} $2 != ""
    Return
  ${EndIf}
  ${If} $1 == ""
    StrCpy $1 "$INSTDIR"
  ${Else}
    StrCpy $2 $1 1 -1
    ${If} $2 == ";"
      StrCpy $1 "$1$INSTDIR"
    ${Else}
      StrCpy $1 "$1;$INSTDIR"
    ${EndIf}
  ${EndIf}
  WriteRegExpandStr HKLM "SYSTEM\CurrentControlSet\Control\Session Manager\Environment" "Path" "$1"
FunctionEnd

; Same for the user PATH.
Function AddToUserPath
  ReadRegStr $1 HKCU "Environment" "Path"
  ${StrStr} $2 $1 "$INSTDIR"
  ${If} $2 != ""
    Return
  ${EndIf}
  ${If} $1 == ""
    StrCpy $1 "$INSTDIR"
  ${Else}
    StrCpy $2 $1 1 -1
    ${If} $2 == ";"
      StrCpy $1 "$1$INSTDIR"
    ${Else}
      StrCpy $1 "$1;$INSTDIR"
    ${EndIf}
  ${EndIf}
  WriteRegExpandStr HKCU "Environment" "Path" "$1"
FunctionEnd

; Tell running processes (including explorer.exe) that the environment
; changed, so new consoles pick up the PATH without a reboot.
Function BroadcastEnvironmentChange
  System::Call 'user32.dll::SendMessage(i 0xFFFF, i 0x001A, i 0, t "Environment")'
FunctionEnd

; --------------------------------------------------------------------------
; Uninstaller
; --------------------------------------------------------------------------
Section "Uninstall"
  SetRegView 64

  ; Remove whichever scope this install used. Both keys are checked because
  ; the running copy may be a different scope than the one uninstalling.
  ReadRegStr $0 HKLM "${UNINST_KEY}" "InstallLocation"
  ${If} $0 != ""
    Call un.RemoveFromSystemPath
    DeleteRegKey HKLM "${UNINST_KEY}"
  ${EndIf}

  ReadRegStr $0 HKCU "${UNINST_KEY}" "InstallLocation"
  ${If} $0 != ""
    Call un.RemoveFromUserPath
    DeleteRegKey HKCU "${UNINST_KEY}"
  ${EndIf}

  RMDir /r "$SMPROGRAMS\${PRODUCTNAME}"
  RMDir /r "$INSTDIR"
  Call un.BroadcastEnvironmentChange
SectionEnd

Function un.RemoveFromSystemPath
  ReadRegStr $1 HKLM "SYSTEM\CurrentControlSet\Control\Session Manager\Environment" "Path"
  ${UnStrRep} $1 $1 ";$INSTDIR;" ";"
  ${UnStrRep} $1 $1 ";$INSTDIR" ""
  ${UnStrRep} $1 $1 "$INSTDIR;" ""
  WriteRegExpandStr HKLM "SYSTEM\CurrentControlSet\Control\Session Manager\Environment" "Path" "$1"
FunctionEnd

Function un.RemoveFromUserPath
  ReadRegStr $1 HKCU "Environment" "Path"
  ${UnStrRep} $1 $1 ";$INSTDIR;" ";"
  ${UnStrRep} $1 $1 ";$INSTDIR" ""
  ${UnStrRep} $1 $1 "$INSTDIR;" ""
  WriteRegExpandStr HKCU "Environment" "Path" "$1"
FunctionEnd

Function un.BroadcastEnvironmentChange
  System::Call 'user32.dll::SendMessage(i 0xFFFF, i 0x001A, i 0, t "Environment")'
FunctionEnd