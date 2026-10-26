; Inno Setup script for j2k viewer (Windows installer + uninstaller).
;
; Build with scripts/build-installer.ps1, or by hand:
;   ISCC.exe /DAppVersion=0.1.0 /DArch=x86_64 /DBinDir=..\target\release installer\j2k-viewer.iss
;
; Installs for the current user only (no administrator prompt) into %LOCALAPPDATA%\Programs,
; adds a Start menu entry, and registers the JPEG 2000 file types so the viewer shows up under
; "Open with" and in Settings > Apps > Default apps. Windows does not let installers take over a
; file type silently, so the last page offers to open the Default apps settings for you.

#ifndef AppVersion
  #define AppVersion "0.0.0-dev"
#endif
#ifndef Arch
  #define Arch "x86_64"
#endif
#ifndef BinDir
  #define BinDir "..\target\release"
#endif

#define AppName "j2k viewer"
#define ProgId "j2kviewer.image"
#define RegApp "j2k viewer"

[Setup]
; Never change this GUID: it is how Windows recognises the app for upgrades and uninstall.
AppId={{B6A5C2E4-7F1D-4C9A-9D3B-2E8F5A1C6D07}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=haydenport
AppPublisherURL=https://github.com/haydenport/j2k-viewer
AppSupportURL=https://github.com/haydenport/j2k-viewer/issues
AppUpdatesURL=https://github.com/haydenport/j2k-viewer/releases
VersionInfoVersion={#AppVersion}
VersionInfoDescription={#AppName} setup
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
UninstallDisplayName={#AppName}
UninstallDisplayIcon={app}\viewer.exe
SetupIconFile=..\assets\icon.ico
LicenseFile=..\installer\license.txt
OutputDir=..\dist
OutputBaseFilename=j2k-viewer-{#AppVersion}-windows-{#Arch}-setup
Compression=lzma2/ultra
SolidCompression=yes
WizardStyle=modern
; Per-user install: no UAC prompt, and the registry entries below go to HKEY_CURRENT_USER.
PrivilegesRequired=lowest
ChangesAssociations=yes
CloseApplications=yes
RestartApplications=no
#if Arch == "aarch64"
ArchitecturesAllowed=arm64
ArchitecturesInstallIn64BitMode=arm64
#else
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
#endif

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "associate"; Description: "Register {#AppName} for JPEG 2000 files (.jp2 .j2k .j2c .jph .jhc .jpc)"; GroupDescription: "File types:"
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Shortcuts:"; Flags: unchecked

[Files]
Source: "{#BinDir}\viewer.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinDir}\tools.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE-MIT"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE-APACHE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\viewer.exe"; Comment: "View huge JPEG 2000 / HTJ2K images"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\viewer.exe"; Tasks: desktopicon

[Registry]
; --- the file type description ("ProgID") and how to open it
Root: HKA; Subkey: "Software\Classes\{#ProgId}"; ValueType: string; ValueData: "JPEG 2000 image"; Flags: uninsdeletekey; Tasks: associate
Root: HKA; Subkey: "Software\Classes\{#ProgId}\DefaultIcon"; ValueType: string; ValueData: """{app}\viewer.exe"",0"; Tasks: associate
Root: HKA; Subkey: "Software\Classes\{#ProgId}\shell\open"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "{#AppName}"; Tasks: associate
Root: HKA; Subkey: "Software\Classes\{#ProgId}\shell\open\command"; ValueType: string; ValueData: """{app}\viewer.exe"" ""%1"""; Tasks: associate

; --- offer it in "Open with" for each extension (this does NOT change the current default app).
; The two key lines per extension remove the (possibly just-created) empty keys on uninstall, but
; leave them alone if anything else still lives there. Uninstall runs these entries bottom-up, so
; the value is deleted first, then OpenWithProgids, then the extension key.
Root: HKA; Subkey: "Software\Classes\.jp2"; Flags: uninsdeletekeyifempty; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.jp2\OpenWithProgids"; Flags: uninsdeletekeyifempty; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.jp2\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.j2k"; Flags: uninsdeletekeyifempty; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.j2k\OpenWithProgids"; Flags: uninsdeletekeyifempty; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.j2k\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.j2c"; Flags: uninsdeletekeyifempty; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.j2c\OpenWithProgids"; Flags: uninsdeletekeyifempty; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.j2c\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.jph"; Flags: uninsdeletekeyifempty; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.jph\OpenWithProgids"; Flags: uninsdeletekeyifempty; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.jph\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.jhc"; Flags: uninsdeletekeyifempty; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.jhc\OpenWithProgids"; Flags: uninsdeletekeyifempty; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.jhc\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.jpc"; Flags: uninsdeletekeyifempty; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.jpc\OpenWithProgids"; Flags: uninsdeletekeyifempty; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.jpc\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate

; --- list the app in Settings > Apps > Default apps
Root: HKA; Subkey: "Software\{#RegApp}"; Flags: uninsdeletekey; Tasks: associate
Root: HKA; Subkey: "Software\{#RegApp}\Capabilities"; ValueType: string; ValueName: "ApplicationName"; ValueData: "{#AppName}"; Tasks: associate
Root: HKA; Subkey: "Software\{#RegApp}\Capabilities"; ValueType: string; ValueName: "ApplicationDescription"; ValueData: "View huge lossless JPEG 2000 / HTJ2K images"; Tasks: associate
Root: HKA; Subkey: "Software\{#RegApp}\Capabilities\FileAssociations"; ValueType: string; ValueName: ".jp2"; ValueData: "{#ProgId}"; Tasks: associate
Root: HKA; Subkey: "Software\{#RegApp}\Capabilities\FileAssociations"; ValueType: string; ValueName: ".j2k"; ValueData: "{#ProgId}"; Tasks: associate
Root: HKA; Subkey: "Software\{#RegApp}\Capabilities\FileAssociations"; ValueType: string; ValueName: ".j2c"; ValueData: "{#ProgId}"; Tasks: associate
Root: HKA; Subkey: "Software\{#RegApp}\Capabilities\FileAssociations"; ValueType: string; ValueName: ".jph"; ValueData: "{#ProgId}"; Tasks: associate
Root: HKA; Subkey: "Software\{#RegApp}\Capabilities\FileAssociations"; ValueType: string; ValueName: ".jhc"; ValueData: "{#ProgId}"; Tasks: associate
Root: HKA; Subkey: "Software\{#RegApp}\Capabilities\FileAssociations"; ValueType: string; ValueName: ".jpc"; ValueData: "{#ProgId}"; Tasks: associate
Root: HKA; Subkey: "Software\RegisteredApplications"; ValueType: string; ValueName: "{#RegApp}"; ValueData: "Software\{#RegApp}\Capabilities"; Flags: uninsdeletevalue; Tasks: associate

[Run]
Filename: "{app}\viewer.exe"; Description: "Launch {#AppName}"; Flags: nowait postinstall skipifsilent
Filename: "ms-settings:defaultapps?registeredAppUser={#RegApp}"; Description: "Open Windows Default apps to choose {#AppName} for JPEG 2000 files"; Flags: shellexec postinstall skipifsilent unchecked; Tasks: associate
