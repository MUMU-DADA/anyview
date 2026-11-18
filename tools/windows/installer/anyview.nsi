; Anyview Windows 安装程序脚本（NSIS 3.11）。
; 由 tools/windows/build-installer.sh 编译，以下宏通过 -D 传入：
;   APP_VERSION     版本号（Cargo.toml 的）
;   APP_VERSION4    四段的版本号，写进安装程序的文件属性
;   APP_DIR         在 Windows 上编好、测过的程序文件夹（out/windows/app）
;   APP_ICON        图标
;   WELCOME_BMP     欢迎页、完成页左边的图（tools/icon/make-icons.py 生成）
;   UNINSTALL_LIST  卸载时要删的文件清单（按 APP_DIR 里的文件生成）
;   OUT_FILE        安装程序写到哪里
;
; 安装程序自己只做三件事：放文件、建快捷方式、生成卸载程序。往系统里登记（“打开方式”和默认应用、资源管理器的
; 缩略图和右键菜单、“应用和功能”里的卸载项）交给装好的 Anyview.exe --register，卸载时 Anyview.exe --unregister
; 照登记时的记录原样撤销：写什么、怎么撤都只在核心里有一份（crates/anyview-core/src/winshell.rs）。
; 装在当前用户自己的文件夹里，登记也只写当前用户的那一半注册表，不需要管理员权限。
; 这个文件是带 BOM 的 UTF-8。

Unicode true
!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "LogicLib.nsh"

Name "Anyview"
OutFile "${OUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\Anyview"
; 以前装过就默认装到原来的位置（升级）。
InstallDirRegKey HKCU "Software\Anyview" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma
ManifestDPIAware true
BrandingText "Anyview ${APP_VERSION}"
VIProductVersion "${APP_VERSION4}"
VIAddVersionKey "ProductName" "Anyview"
VIAddVersionKey "FileDescription" "Anyview Setup"
VIAddVersionKey "FileVersion" "${APP_VERSION}"
VIAddVersionKey "ProductVersion" "${APP_VERSION}"
VIAddVersionKey "LegalCopyright" ""

Var Upgrade        ; 1 = 这台电脑上已经装着（升级），完成页不再默认勾“设为默认看图软件”
Var PurgeSettings  ; 卸载时：1 = 连设置一起删

!define MUI_ICON "${APP_ICON}"
!define MUI_UNICON "${APP_ICON}"
!define MUI_ABORTWARNING
!define MUI_COMPONENTSPAGE_NODESC
!define MUI_WELCOMEFINISHPAGE_BITMAP "${WELCOME_BMP}"

!define MUI_WELCOMEPAGE_TEXT "$(TextWelcome)"
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_INSTFILES
; 完成页的两个勾：运行 Anyview；打开系统的“默认应用”设置（Windows 不让程序自己改默认程序，只能带用户去选）。
; 说明文字区用大的那种（7 行）：别的语言的说明比英文长。
!define MUI_FINISHPAGE_TEXT_LARGE
!define MUI_FINISHPAGE_TEXT "$(TextFinish)"
!define MUI_FINISHPAGE_RUN "$INSTDIR\Anyview.exe"
!define MUI_FINISHPAGE_RUN_TEXT "$(TextRun)"
!define MUI_FINISHPAGE_SHOWREADME ""
!define MUI_FINISHPAGE_SHOWREADME_TEXT "$(TextDefaultApps)"
!define MUI_FINISHPAGE_SHOWREADME_FUNCTION OpenDefaultApps
!define MUI_PAGE_CUSTOMFUNCTION_SHOW FinishShow
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

; 19 种语言和安装程序自己写的那几句：由核心的测试按 crates/anyview-core/src/packaging.rs 生成。
!include "languages.nsh"

; NSIS 按系统的界面语言选：没有完全一样的就找同一大类的，“中文”这一类不分简繁。和程序的选法一致：
; 香港、澳门用繁体，新加坡用简体。
!macro PickChinese
  System::Call 'kernel32::GetUserDefaultUILanguage() i .r0'
  ${If} $0 = 3076
  ${OrIf} $0 = 5124
    StrCpy $LANGUAGE ${LANG_TRADCHINESE}
  ${ElseIf} $0 = 4100
    StrCpy $LANGUAGE ${LANG_SIMPCHINESE}
  ${EndIf}
!macroend

Function .onInit
  !insertmacro PickChinese
  StrCpy $Upgrade 0
  ReadRegStr $0 HKCU "Software\Anyview" "InstallDir"
  ${If} $0 != ""
    StrCpy $Upgrade 1
  ${EndIf}
FunctionEnd

Function FinishShow
  ${If} $Upgrade == 1
    SendMessage $mui.FinishPage.ShowReadme ${BM_SETCHECK} ${BST_UNCHECKED} 0
  ${EndIf}
FunctionEnd

Function OpenDefaultApps
  ExecShell "open" "ms-settings:defaultapps"
FunctionEnd

; Anyview 开着时它的文件换不掉、删不掉：等用户关掉。静默安装、卸载时不问，直接失败（返回码 2）。
!macro WaitUntilClosed
  ${Do}
    ${IfNot} ${FileExists} "$INSTDIR\Anyview.exe"
      ${ExitDo}
    ${EndIf}
    ClearErrors
    FileOpen $0 "$INSTDIR\Anyview.exe" a
    ${IfNot} ${Errors}
      FileClose $0
      ${ExitDo}
    ${EndIf}
    ${If} ${Silent}
      SetErrorLevel 2
      Abort
    ${EndIf}
    MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "$(TextRunning)" IDRETRY +2
    Abort
  ${Loop}
!macroend

Section "$(TextMain)" SecMain
  SectionIn RO
  ; 升级：让旧的程序先结束占着核心 DLL 的缩略图进程（资源管理器用的时候会再开）。
  ${If} ${FileExists} "$INSTDIR\Anyview.exe"
    ExecWait '"$INSTDIR\Anyview.exe" --stop-hosts'
  ${EndIf}
  !insertmacro WaitUntilClosed
  SetOutPath "$INSTDIR"
  File /r /x *.pdb "${APP_DIR}/*"
  ; 许可证：build-installer.sh 换成 Windows 换行的两份。
  File "${LICENSE_DIR}/LICENSE.txt"
  File "${LICENSE_DIR}/THIRD-PARTY-NOTICES.txt"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  ; 登记到系统。装好后的大小（KB）显示在“应用和功能”里。
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  ExecWait '"$INSTDIR\Anyview.exe" --register --version ${APP_VERSION} --size-kb $0' $3
  ${If} $3 != 0
    ${IfNot} ${Silent}
      MessageBox MB_OK|MB_ICONEXCLAMATION "$(TextRegisterFailed)"
    ${EndIf}
    SetErrorLevel 3
  ${EndIf}
SectionEnd

Section "$(TextStartMenu)" SecStartMenu
  CreateShortcut "$SMPROGRAMS\Anyview.lnk" "$INSTDIR\Anyview.exe"
SectionEnd

Section /o "$(TextDesktop)" SecDesktop
  CreateShortcut "$DESKTOP\Anyview.lnk" "$INSTDIR\Anyview.exe"
SectionEnd

; 卸载程序平常是把自己复制到临时文件夹再运行的（这样才删得掉安装文件夹里的那个）。NSIS 让系统在下次重启时删这个
; 副本，这要管理员权限，我们没有，副本就会留在临时文件夹里。这里自己收拾：退出几秒后由一条不显示
; 窗口的命令删掉那个临时文件夹。只在自己确实是从那种临时文件夹里运行时才做。
Function un.CleanTempCopy
  StrLen $1 "$TEMP\~nsu"
  StrCpy $2 "$EXEDIR" $1
  ${If} $2 == "$TEMP\~nsu"
    ExecShell "open" "$SYSDIR\cmd.exe" '/c ping -n 4 127.0.0.1 >nul & rd /s /q "$EXEDIR"' SW_HIDE
  ${EndIf}
FunctionEnd

; 有界面时：窗口关掉、马上要退出的时候。
Function un.onGUIEnd
  Call un.CleanTempCopy
FunctionEnd

Function un.onInit
  !insertmacro PickChinese
  ; 静默卸载默认留着设置，加 /PURGE 才删；平常卸载时问一句，默认删（卸载后不留东西）。
  StrCpy $PurgeSettings 0
  ${GetParameters} $0
  ClearErrors
  ${GetOptions} $0 "/PURGE" $1
  ${IfNot} ${Errors}
    StrCpy $PurgeSettings 1
  ${EndIf}
FunctionEnd

Section "Uninstall"
  !insertmacro WaitUntilClosed
  ${IfNot} ${Silent}
    MessageBox MB_YESNO|MB_ICONQUESTION "$(TextPurge)" IDNO +2
    StrCpy $PurgeSettings 1
  ${EndIf}
  ; 撤销登记，并结束占着核心 DLL 的缩略图进程。
  ${If} ${FileExists} "$INSTDIR\Anyview.exe"
    ExecWait '"$INSTDIR\Anyview.exe" --unregister'
  ${EndIf}
  ; 只删装进来的那些文件（清单是打包时按程序文件夹生成的），不整个删文件夹：万一装在放着别的东西的文件夹里。
  !include "${UNINSTALL_LIST}"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\Anyview.lnk"
  Delete "$DESKTOP\Anyview.lnk"
  ${If} $PurgeSettings == 1
    RMDir /r "$APPDATA\Anyview"
  ${EndIf}
  ; 静默卸载没有窗口，做完就退出：在这里收拾临时副本。
  ${If} ${Silent}
    Call un.CleanTempCopy
  ${EndIf}
SectionEnd
