; Per-user installer for Rile (no administrator rights needed).
; Built by scripts/bundle-windows.ps1:
;   iscc /DAppVersion=<version> /DSourceExe=<path\to\rile.exe> /DOutputDir=<dir> packaging\windows\rile.iss

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceExe
  #define SourceExe "..\..\target\release\rile.exe"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif

[Setup]
AppId={{6F1D2C3B-7A5E-4E2B-9C1D-52494C450001}
AppName=Rile
AppVersion={#AppVersion}
AppPublisher=Rile
AppPublisherURL=https://github.com/RickyDane/rile
DefaultDirName={localappdata}\Programs\Rile
DefaultGroupName=Rile
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename=Rile-{#AppVersion}-x64-setup
SetupIconFile=..\..\crates\rile-app\assets\Rile.ico
UninstallDisplayIcon={app}\rile.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
; Rile may be running (it is single-instance); let Setup close and restart it.
CloseApplications=yes
RestartApplications=yes

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "rile.exe"; Flags: ignoreversion

[Icons]
; The AppUserModelID lets the backup helper post toasts as Rile.
Name: "{autoprograms}\Rile"; Filename: "{app}\rile.exe"; AppUserModelID: "dev.rile.Rile"
Name: "{autodesktop}\Rile"; Filename: "{app}\rile.exe"; Tasks: desktopicon

[Registry]
; rile:// links (the backup helper opens pages through them) and "Open in
; Rile" on folders, drives and folder backgrounds. Rile refreshes these on
; every start; making it the default folder handler is a separate opt-in in
; Rile's settings.
Root: HKCU; Subkey: "Software\Classes\rile"; ValueType: string; ValueName: ""; ValueData: "URL:Rile"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\rile"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\rile\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\rile.exe"",0"
Root: HKCU; Subkey: "Software\Classes\rile\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\rile.exe"" ""%1"""
Root: HKCU; Subkey: "Software\Classes\Directory\shell\rile"; ValueType: string; ValueName: ""; ValueData: "Open in Rile"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\Directory\shell\rile"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\rile.exe"",0"
Root: HKCU; Subkey: "Software\Classes\Directory\shell\rile\command"; ValueType: string; ValueName: ""; ValueData: """{app}\rile.exe"" ""%1"""
Root: HKCU; Subkey: "Software\Classes\Drive\shell\rile"; ValueType: string; ValueName: ""; ValueData: "Open in Rile"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\Drive\shell\rile"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\rile.exe"",0"
Root: HKCU; Subkey: "Software\Classes\Drive\shell\rile\command"; ValueType: string; ValueName: ""; ValueData: """{app}\rile.exe"" ""%1"""
Root: HKCU; Subkey: "Software\Classes\Directory\Background\shell\rile"; ValueType: string; ValueName: ""; ValueData: "Open in Rile"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\Directory\Background\shell\rile"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\rile.exe"",0"
Root: HKCU; Subkey: "Software\Classes\Directory\Background\shell\rile\command"; ValueType: string; ValueName: ""; ValueData: """{app}\rile.exe"" ""%V"""

[Run]
Filename: "{app}\rile.exe"; Description: "{cm:LaunchProgram,Rile}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; Removes the default-folder-handler opt-in, the backup task and the
; notification-area helper's startup entry.
Filename: "{app}\rile.exe"; Parameters: "--unregister"; Flags: runhidden waituntilterminated; RunOnceId: "RileUnregister"
