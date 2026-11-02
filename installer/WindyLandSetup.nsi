; WindyLand + Tobi (AI assistant) installer (NSIS 3).  Build:  makensis /DVERSION=0.3.0 installer/WindyLandSetup.nsi
; Per-user install (no admin rights). The wizard asks for the AI provider and API key, then installs
; WindyLand and sets up the agent (Python if missing, packages, settings) automatically.

Unicode true
!ifndef VERSION
  !define VERSION "0.3.0"
!endif
!ifndef EXE
  !define EXE "..\target\x86_64-pc-windows-gnu\release\windyland.exe"
!endif
!ifndef OUTDIR
  !define OUTDIR "..\dist"
!endif

!define APP "WindyLand"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\WindyLand"
!define RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"

Name "${APP} with Tobi"
OutFile "${OUTDIR}\WindyLandTobiSetup-${VERSION}.exe"
InstallDir "$LOCALAPPDATA\Programs\WindyLand"
InstallDirRegKey HKCU "${UNINST_KEY}" "InstallLocation"
RequestExecutionLevel user
SetCompressor /SOLID lzma
ShowInstDetails show
BrandingText "WindyLand ${VERSION}"

!include "MUI2.nsh"
!include "nsDialogs.nsh"
!include "LogicLib.nsh"
!include "WinMessages.nsh"

!define MUI_ICON "..\assets\windyland.ico"
!define MUI_UNICON "..\assets\windyland.ico"
!define MUI_ABORTWARNING
!define MUI_WELCOMEPAGE_TITLE "Welcome to WindyLand with Tobi"
!define MUI_WELCOMEPAGE_TEXT "This installs WindyLand, the notch for Windows, together with Tobi, your AI assistant.$\r$\n$\r$\nTobi chats with you, plays music and anime, humanizes your Word files, remembers what you like and runs commands on this PC. In the next steps you enter your OpenAI API key; everything Tobi needs (Python and its packages) is downloaded and installed for you.$\r$\n$\r$\nAn internet connection is required."
!define MUI_FINISHPAGE_RUN
!define MUI_FINISHPAGE_RUN_TEXT "Start WindyLand and say hi to Tobi"
!define MUI_FINISHPAGE_RUN_FUNCTION LaunchApp
!define MUI_FINISHPAGE_TITLE "WindyLand and Tobi are installed"
!define MUI_FINISHPAGE_TEXT "Here is Tobi's MEMORY FOLDER:$\r$\n$\r$\n    $DOCUMENTS\Tobi\Knowledge$\r$\n$\r$\nDrop any file in it (Word, Excel, PDF, PowerPoint, text) and Tobi will read it and memorize it. Then ask him about it, for example: What does my report say about the budget?$\r$\n$\r$\nYou can open this folder any time with the folder button in Tobi's chat, or the Start Menu shortcut Tobi Knowledge Folder."
!define MUI_FINISHPAGE_SHOWREADME ""
!define MUI_FINISHPAGE_SHOWREADME_NOTCHECKED
!define MUI_FINISHPAGE_SHOWREADME_TEXT "Open Tobi's memory folder now (drop your files in)"
!define MUI_FINISHPAGE_SHOWREADME_FUNCTION OpenKnowledge

Var Dlg
Var hProvider
Var hModel
Var hKey
Var hBase
Var hHelp
Var hShow
Var Provider
Var Model
Var ApiKey
Var BaseUrl
Var AgentOk

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "..\LICENSE"
!insertmacro MUI_PAGE_COMPONENTS
Page custom AiPageCreate AiPageLeave
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

; ----------------------------------------------------------------------------- AI settings page
; index in the drop-down -> provider id (same order as the list below)
Function AiPageCreate
  !insertmacro MUI_HEADER_TEXT "The AI behind Tobi" "Choose who provides the AI and paste your API key (or keep what you chose before)."
  nsDialogs::Create 1018
  Pop $Dlg
  ${If} $Dlg == error
    Abort
  ${EndIf}

  ${NSD_CreateLabel} 0 0 100% 12u "AI provider"
  Pop $0
  ${NSD_CreateDropList} 0 13u 100% 100u ""
  Pop $hProvider
  ${NSD_CB_AddString} $hProvider "OpenAI (GPT)"
  ${NSD_CB_AddString} $hProvider "Anthropic (Claude)"
  ${NSD_CB_AddString} $hProvider "Google Gemini"
  ${NSD_CB_AddString} $hProvider "OpenRouter (many models)"
  ${NSD_CB_AddString} $hProvider "Ollama (local, no key)"
  ${NSD_CB_AddString} $hProvider "Custom (OpenAI-compatible)"
  ${NSD_CB_AddString} $hProvider "Keep my current settings (update / reinstall)"
  ; an update must not silently replace the provider (for example Ollama) you chose before
  IfFileExists "$APPDATA\WindyLand\agent\config.json" 0 +3
    ${NSD_CB_SelectString} $hProvider "Keep my current settings (update / reinstall)"
    Goto +2
  ${NSD_CB_SelectString} $hProvider "OpenAI (GPT)"
  ${NSD_OnChange} $hProvider AiProviderChanged

  ${NSD_CreateLabel} 0 32u 100% 12u "API key"
  Pop $0
  ${NSD_CreatePassword} 0 45u 82% 12u "$ApiKey"
  Pop $hKey
  ${NSD_CreateCheckbox} 84% 46u 16% 12u "Show"
  Pop $hShow
  ${NSD_OnClick} $hShow AiShowKey

  ${NSD_CreateLabel} 0 60u 100% 12u ""
  Pop $hHelp

  ${NSD_CreateLabel} 0 76u 100% 12u "Model (leave empty for the default)"
  Pop $0
  ${NSD_CreateText} 0 89u 100% 12u "$Model"
  Pop $hModel

  ${NSD_CreateLabel} 0 106u 100% 12u "API address (leave empty unless you use a proxy or a local server)"
  Pop $0
  ${NSD_CreateText} 0 119u 100% 12u "$BaseUrl"
  Pop $hBase

  ${NSD_CreateLabel} 0 138u 100% 22u "You can change all of this later in Tobi's window (gear icon). The key is stored only on this PC, in your user profile."
  Pop $0

  Call AiProviderChanged
  nsDialogs::Show
FunctionEnd

Function AiShowKey
  ${NSD_GetState} $hShow $0
  ; ES_PASSWORD: pass the password character (0 = show text, '*' = hide)
  ${If} $0 == ${BST_CHECKED}
    SendMessage $hKey ${EM_SETPASSWORDCHAR} 0 0
  ${Else}
    SendMessage $hKey ${EM_SETPASSWORDCHAR} 0x25CF 0
  ${EndIf}
  System::Call 'user32::InvalidateRect(p $hKey, p 0, i 1)'
FunctionEnd

Function AiProviderChanged
  SendMessage $hProvider ${CB_GETCURSEL} 0 0 $0
  ${Switch} $0
    ${Case} 0
      ${NSD_SetText} $hHelp "Get a key at platform.openai.com > API keys.   Default model: gpt-4.1"
      SendMessage $hModel 0x1501 1 "STR:gpt-4.1"
      SendMessage $hBase 0x1501 1 "STR:https://api.openai.com/v1"
      ${Break}
    ${Case} 1
      ${NSD_SetText} $hHelp "Get a key at console.anthropic.com > API keys.   Default model: claude-sonnet-5-5"
      SendMessage $hModel 0x1501 1 "STR:claude-sonnet-5-5"
      SendMessage $hBase 0x1501 1 "STR:https://api.anthropic.com"
      ${Break}
    ${Case} 2
      ${NSD_SetText} $hHelp "Get a key at aistudio.google.com > Get API key.   Default model: gemini-2.5-flash"
      SendMessage $hModel 0x1501 1 "STR:gemini-2.5-flash"
      SendMessage $hBase 0x1501 1 "STR:https://generativelanguage.googleapis.com/v1beta/openai"
      ${Break}
    ${Case} 3
      ${NSD_SetText} $hHelp "Get a key at openrouter.ai > Keys.   Type any model name from their list."
      SendMessage $hModel 0x1501 1 "STR:anthropic/claude-sonnet-4.5"
      SendMessage $hBase 0x1501 1 "STR:https://openrouter.ai/api/v1"
      ${Break}
    ${Case} 4
      ${NSD_SetText} $hHelp "No key needed. Install Ollama first and pull a model (for example: ollama pull llama3.1)."
      SendMessage $hModel 0x1501 1 "STR:llama3.1"
      SendMessage $hBase 0x1501 1 "STR:http://localhost:11434/v1"
      ${Break}
    ${Case} 5
      ${NSD_SetText} $hHelp "Any server that speaks the OpenAI chat API. Enter its address and model name below."
      SendMessage $hModel 0x1501 1 "STR:model name"
      SendMessage $hBase 0x1501 1 "STR:https://your-server/v1"
      ${Break}
    ${Default}
      ${NSD_SetText} $hHelp "Nothing changes: your provider, model and key stay as you set them (change them any time with the gear icon)."
      SendMessage $hModel 0x1501 1 "STR:"
      SendMessage $hBase 0x1501 1 "STR:"
      ${Break}
  ${EndSwitch}
FunctionEnd

Function AiPageLeave
  SendMessage $hProvider ${CB_GETCURSEL} 0 0 $0
  ${Switch} $0
    ${Case} 0
      StrCpy $Provider "openai"
      ${Break}
    ${Case} 1
      StrCpy $Provider "anthropic"
      ${Break}
    ${Case} 2
      StrCpy $Provider "gemini"
      ${Break}
    ${Case} 3
      StrCpy $Provider "openrouter"
      ${Break}
    ${Case} 4
      StrCpy $Provider "ollama"
      ${Break}
    ${Case} 5
      StrCpy $Provider "custom"
      ${Break}
    ${Default}
      StrCpy $Provider ""
  ${EndSwitch}
  ${NSD_GetText} $hKey $ApiKey
  ${NSD_GetText} $hModel $Model
  ${NSD_GetText} $hBase $BaseUrl
  ; keep quotes and line breaks out of values that become environment variables
  ${If} $ApiKey == ""
  ${AndIf} $Provider != ""
  ${AndIf} $Provider != "ollama"
  ${AndIf} $Provider != "custom"
    MessageBox MB_YESNO|MB_ICONQUESTION "No API key entered.$\r$\n$\r$\nTobi cannot answer without one. Continue anyway and add the key later in his window (gear icon)?" IDYES +2
    Abort
  ${EndIf}
  ${If} $Provider == "custom"
  ${AndIf} $BaseUrl == ""
    MessageBox MB_OK|MB_ICONEXCLAMATION "Please enter the API address of your server."
    Abort
  ${EndIf}
FunctionEnd

; ----------------------------------------------------------------------------- install
Section "WindyLand and Tobi (required)" SecMain
  SectionIn RO
  SetOutPath "$INSTDIR"

  ; stop a running copy so files can be replaced
  IfFileExists "$INSTDIR\agent\venv\Scripts\pythonw.exe" 0 no_old_agent
    SetOutPath "$INSTDIR\agent"
    nsExec::Exec '"$INSTDIR\agent\venv\Scripts\pythonw.exe" -m windyagent.launcher --stop'
    Pop $0
  no_old_agent:
  nsExec::Exec 'taskkill /F /IM windyland.exe'
  Pop $0
  Sleep 800
  SetOutPath "$INSTDIR"

  DetailPrint "Installing WindyLand..."
  File "${EXE}"
  File "..\LICENSE"
  File "..\README.md"
  SetOutPath "$INSTDIR\assets"
  File "..\assets\windyland.ico"

  DetailPrint "Copying Tobi..."
  SetOutPath "$INSTDIR\agent"
  File "..\agent\requirements.txt"
  File "..\agent\setup_agent.ps1"
  SetOutPath "$INSTDIR\agent\windyagent"
  File /r /x "__pycache__" /x "*.pyc" "..\agent\windyagent\*.*"

  ; shortcuts
  CreateShortcut "$SMPROGRAMS\WindyLand.lnk" "$INSTDIR\windyland.exe" "" "$INSTDIR\assets\windyland.ico"

  ; uninstaller + Apps & features entry
  SetOutPath "$INSTDIR"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "WindyLand with Tobi"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "WindyLand"
  WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\assets\windyland.ico"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1

  ; settings typed in the wizard travel as environment variables (not on a command line, not in a file)
  System::Call 'kernel32::SetEnvironmentVariable(t "WL_PROVIDER", t "$Provider")'
  System::Call 'kernel32::SetEnvironmentVariable(t "WL_MODEL", t "$Model")'
  System::Call 'kernel32::SetEnvironmentVariable(t "WL_API_KEY", t "$ApiKey")'
  System::Call 'kernel32::SetEnvironmentVariable(t "WL_BASE_URL", t "$BaseUrl")'

  DetailPrint ""
  DetailPrint "Setting up Tobi (this can take a few minutes the first time)..."
  nsExec::ExecToLog '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\agent\setup_agent.ps1" -AppDir "$INSTDIR"'
  Pop $AgentOk

  System::Call 'kernel32::SetEnvironmentVariable(t "WL_API_KEY", t "")'
  StrCpy $ApiKey ""

  ${If} $AgentOk == "0"
    SetOutPath "$INSTDIR\agent"
    CreateShortcut "$SMPROGRAMS\Tobi.lnk" "$INSTDIR\agent\venv\Scripts\pythonw.exe" "-m windyagent.launcher" "$INSTDIR\assets\windyland.ico"
    CreateShortcut "$SMPROGRAMS\Tobi Knowledge Folder.lnk" "$INSTDIR\agent\venv\Scripts\pythonw.exe" "-m windyagent.launcher --knowledge" "$INSTDIR\assets\windyland.ico"
    nsExec::Exec '"$INSTDIR\agent\venv\Scripts\pythonw.exe" -m windyagent.launcher --knowledge-init'
    Pop $0
    DetailPrint ""
    DetailPrint "Tobi's memory folder: $DOCUMENTS\Tobi\Knowledge"
    DetailPrint "Drop any file in it (Word, Excel, PDF, PowerPoint, text) and Tobi will memorize it."
  ${Else}
    StrCpy $3 "(no details)"
    ClearErrors
    FileOpen $2 "$INSTDIR\agent\setup_error.txt" r
    ${IfNot} ${Errors}
      FileRead $2 $3
      FileClose $2
    ${EndIf}
    MessageBox MB_OK|MB_ICONEXCLAMATION "WindyLand is installed, but Tobi could not be set up (code $AgentOk).$\r$\n$\r$\nReason: $3$\r$\n$\r$\nThe lines in the install window show each step. Run this setup again to retry; nothing else is lost."
  ${EndIf}
SectionEnd

Section "Start WindyLand when Windows starts" SecStartup
  WriteRegStr HKCU "${RUN_KEY}" "WindyLand" '"$INSTDIR\windyland.exe"'
SectionEnd

Section /o "Desktop shortcut" SecDesktop
  CreateShortcut "$DESKTOP\WindyLand.lnk" "$INSTDIR\windyland.exe" "" "$INSTDIR\assets\windyland.ico"
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecMain} "The notch, Tobi, and everything Tobi needs."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecStartup} "Starts the notch at sign-in. Tobi opens from the notch's Tobi tab."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} "Adds a WindyLand icon to the desktop."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Function OpenKnowledge
  ${If} $AgentOk == "0"
    SetOutPath "$INSTDIR\agent"
    Exec '"$INSTDIR\agent\venv\Scripts\pythonw.exe" -m windyagent.launcher --knowledge'
  ${Else}
    CreateDirectory "$DOCUMENTS\Tobi\Knowledge"
    ExecShell "open" "$DOCUMENTS\Tobi\Knowledge"
  ${EndIf}
FunctionEnd

Function LaunchApp
  SetOutPath "$INSTDIR"
  Exec '"$INSTDIR\windyland.exe"'
  ${If} $AgentOk == "0"
    SetOutPath "$INSTDIR\agent"
    Exec '"$INSTDIR\agent\venv\Scripts\pythonw.exe" -m windyagent.launcher'
  ${EndIf}
FunctionEnd

; ----------------------------------------------------------------------------- uninstall
Section "Uninstall"
  IfFileExists "$INSTDIR\agent\venv\Scripts\pythonw.exe" 0 un_no_agent
    SetOutPath "$INSTDIR\agent"
    nsExec::Exec '"$INSTDIR\agent\venv\Scripts\pythonw.exe" -m windyagent.launcher --stop'
    Pop $0
  un_no_agent:
  nsExec::Exec 'taskkill /F /IM windyland.exe'
  Pop $0
  Sleep 800
  SetOutPath "$TEMP"

  DeleteRegValue HKCU "${RUN_KEY}" "WindyLand"
  DeleteRegKey HKCU "${UNINST_KEY}"
  Delete "$SMPROGRAMS\WindyLand.lnk"
  Delete "$SMPROGRAMS\Tobi.lnk"
  Delete "$SMPROGRAMS\Tobi Knowledge Folder.lnk"
  Delete "$SMPROGRAMS\WindyLand AI Agent.lnk"
  Delete "$DESKTOP\WindyLand.lnk"

  StrCpy $1 ""
  IfFileExists "$INSTDIR\agent\python\python.exe" 0 no_private_python
    StrCpy $1 "yes"
  no_private_python:
  RMDir /r "$INSTDIR"
  ${If} $1 != ""
    MessageBox MB_OK|MB_ICONINFORMATION "Setup had installed Python 3.12 for the agent. Windows still lists it under Settings > Apps; remove it there if you no longer need it."
  ${EndIf}

  IfFileExists "$APPDATA\WindyLand\*.*" 0 done
    MessageBox MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON2 "Also remove your WindyLand data (to-do list, water log, shelf, settings) and Tobi's saved API key, memory and chat history?" IDNO done
    RMDir /r "$APPDATA\WindyLand"
  done:
SectionEnd
