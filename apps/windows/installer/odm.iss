; Installer for the native Windows app (Inno Setup 6).
;
;   iscc /DAppVersion=0.2.0 /DArch=x64 /DSourceDir=<publish folder> /DOutputDir=<folder> odm.iss
;
; Installs per user by default (no administrator prompt), registers the
; odm:// link scheme and replaces an installed Tauri build of the app.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef Arch
  #define Arch "x64"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\..\dist\windows-native\" + Arch + "\app"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\..\dist\windows-native"
#endif
#define AppName "Open Download Manager"
#define AppExe "OpenDownloadManager.exe"

[Setup]
AppId={{B2D7B0E4-5B0B-4C7C-9B1E-0D6F2C6A51D3}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Open Download Manager
AppPublisherURL=https://github.com/zbreilly/open-download-manager
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog commandline
OutputDir={#OutputDir}
OutputBaseFilename=OpenDownloadManager-{#AppVersion}-Windows-{#Arch}-Setup
SetupIconFile=..\..\..\src-tauri\icons\icon.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
LicenseFile=..\..\..\LICENSE
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
MinVersion=10.0.17763
CloseApplications=yes
RestartApplications=no
#if Arch == "arm64"
ArchitecturesAllowed=arm64
ArchitecturesInstallIn64BitMode=arm64
#else
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
#endif

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
; the id lets Windows attribute the app's notifications to this shortcut
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"; AppUserModelID: "app.opendm.desktop"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; odm://add?url=... links open the app
Root: HKA; Subkey: "Software\Classes\odm"; ValueType: string; ValueName: ""; ValueData: "URL:{#AppName}"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\odm"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\odm\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"",0"
Root: HKA; Subkey: "Software\Classes\odm\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""
; "Start when I sign in" is set by the app; take it away on uninstall
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "{#AppName}"; Flags: dontcreatekey uninsdeletevalue

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[Code]
// The earlier Tauri build installs under the same name with its own (NSIS)
// uninstaller. Remove it first so only one copy is installed; the download
// list and settings live in %APPDATA%\app.opendm.desktop and stay.
procedure RemoveTauriBuild();
var
  Uninstaller: String;
  Code: Integer;
begin
  if not RegQueryStringValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Uninstall\Open Download Manager', 'UninstallString', Uninstaller) then
    exit;
  Uninstaller := RemoveQuotes(Uninstaller);
  if not FileExists(Uninstaller) then
    exit;
  // _?= makes the NSIS uninstaller run in place, so Exec can wait for it
  if Exec(Uninstaller, '/S _?=' + ExtractFileDir(Uninstaller), '', SW_HIDE, ewWaitUntilTerminated, Code) then
  begin
    DeleteFile(Uninstaller);
    RemoveDir(ExtractFileDir(Uninstaller));
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  RemoveTauriBuild();
end;
