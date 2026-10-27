; WindyLand installer (Inno Setup 6). Built by build.ps1:
;   ISCC.exe /DAppVersion=0.1.0 /DOutDir=<folder> installer\windyland.iss

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#ifndef OutDir
  #define OutDir "..\dist"
#endif

[Setup]
AppId={{B064C2A6-7D1A-453A-AF15-9B046EEAE6A1}
AppName=WindyLand
AppVersion={#AppVersion}
AppVerName=WindyLand {#AppVersion}
AppPublisher=Shabab
AppPublisherURL=https://github.com/shabab239/WindyLand
AppSupportURL=https://github.com/shabab239/WindyLand/issues
AppUpdatesURL=https://github.com/shabab239/WindyLand/releases
AppCopyright=Copyright (C) 2026 Shabab
AppMutex=WindyLand.SingleInstance
CloseApplications=yes
DefaultDirName={autopf}\WindyLand
DefaultGroupName=WindyLand
DisableDirPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
LicenseFile=..\LICENSE
SetupIconFile=..\assets\windyland.ico
UninstallDisplayIcon={app}\windyland.exe
UninstallDisplayName=WindyLand
OutputDir={#OutDir}
OutputBaseFilename=WindyLandSetup-{#AppVersion}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
InfoBeforeFile=privacy.txt
VersionInfoCompany=Shabab
VersionInfoDescription=WindyLand Setup
VersionInfoProductName=WindyLand
VersionInfoProductVersion={#AppVersion}
VersionInfoVersion={#AppVersion}
VersionInfoCopyright=Copyright (C) 2026 Shabab

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
en.StartupTask=Start WindyLand when Windows starts
en.RemoveData=Also remove your WindyLand data (to-do list, water log, shelf, settings)?
en.UpdatesTask=Check for updates automatically (asks GitHub for the latest version about twice a day)

[Tasks]
Name: "startup"; Description: "{cm:StartupTask}"
Name: "desktop"; Description: "{cm:CreateDesktopIcon}"; Flags: unchecked
Name: "updates"; Description: "{cm:UpdatesTask}"

[Files]
Source: "..\target\release\windyland.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "WindyLand"; ValueData: """{app}\windyland.exe"""; Tasks: startup; Flags: uninsdeletevalue

[Icons]
Name: "{autoprograms}\WindyLand"; Filename: "{app}\windyland.exe"
Name: "{autodesktop}\WindyLand"; Filename: "{app}\windyland.exe"; Tasks: desktop

[Run]
Filename: "{app}\windyland.exe"; Description: "{cm:LaunchProgram,WindyLand}"; Flags: postinstall nowait

[UninstallDelete]
Type: filesandordirs; Name: "{app}"

[Code]
// Remember the "Check for updates automatically" choice in WindyLand's
// settings (a later line wins, so appending is enough).
procedure CurStepChanged(CurStep: TSetupStep);
var
  Dir, Line: String;
begin
  if CurStep = ssPostInstall then
  begin
    Dir := ExpandConstant('{userappdata}\WindyLand');
    ForceDirectories(Dir);
    if WizardIsTaskSelected('updates') then
      Line := 'update_check=1'
    else
      Line := 'update_check=0';
    SaveStringToFile(Dir + '\settings.txt', #13#10 + Line + #13#10, True);
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    if DirExists(ExpandConstant('{userappdata}\WindyLand')) then
      if MsgBox(CustomMessage('RemoveData'), mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
        DelTree(ExpandConstant('{userappdata}\WindyLand'), True, True, True);
end;
