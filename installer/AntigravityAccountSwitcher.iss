; The Windows installer, built by `cargo xtask dist` with Inno Setup 6 or 7.
;
; A per-user install (no administrator rights) into %LOCALAPPDATA%\AntigravityAccountSwitcher:
; the folder the app itself treats as "the installed copy", so its own Install on this PC, its
; account shortcuts, Start with Windows and its self-update all work the same way. Shortcut names
; match the ones the app creates. Uninstalling removes the program and the shortcuts the app
; made; settings, logs and saved accounts (in Windows Credential Manager) stay.
;
; xtask passes: AppVersion (1.2.0 or 1.3.0-beta.1), NumericVersion (1.2.0), SourceDir (the
; repository), BuildDir (where the release build is), OutputDir and Repository (the GitHub
; address from Cargo.toml).

#ifndef AppVersion
  #error AppVersion is missing: build the installer with cargo xtask dist
#endif
#ifndef NumericVersion
  #define NumericVersion AppVersion
#endif
#ifndef SourceDir
  #define SourceDir ".."
#endif
#ifndef BuildDir
  #define BuildDir SourceDir + "\target\release"
#endif
#ifndef OutputDir
  #define OutputDir SourceDir + "\dist"
#endif
#ifndef Repository
  #error Repository is missing: build the installer with cargo xtask dist
#endif

#define AppName "Antigravity Account Switcher"
#define AppExe "AntigravityAccountSwitcher.exe"
#define AppFolder "AntigravityAccountSwitcher"
; The single-copy names of crates/desktop/src/instance.rs.
#define RunningCopy "AntigravityAccountSwitcher"

[Setup]
AppId={{6F0D6D3C-4C1E-4E7B-9A51-6B7E2C9A1D42}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=DrS
AppPublisherURL={#Repository}
AppSupportURL={#Repository}/issues
AppUpdatesURL={#Repository}/releases
DefaultDirName={localappdata}\{#AppFolder}
DisableDirPage=yes
DisableProgramGroupPage=yes
DisableReadyPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#OutputDir}
OutputBaseFilename=AntigravityAccountSwitcher-{#AppVersion}-setup
SetupIconFile={#SourceDir}\crates\app\assets\icon.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
WizardStyle=modern
Compression=lzma2/ultra64
SolidCompression=yes
CloseApplications=force
RestartApplications=no
VersionInfoVersion={#NumericVersion}
VersionInfoProductVersion={#NumericVersion}
VersionInfoProductTextVersion={#AppVersion}
VersionInfoDescription={#AppName} setup

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Shortcuts:"
Name: "startup"; Description: "Start with Windows (in the tray)"; GroupDescription: "Shortcuts:"; Flags: unchecked

[Files]
Source: "{#BuildDir}\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\{#AppName}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{userdesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon
Name: "{userstartup}\{#AppName}"; Filename: "{app}\{#AppExe}"; Parameters: "--tray"; Tasks: startup

[Run]
Filename: "{app}\{#AppExe}"; Description: "Open {#AppName}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; The app removes the shortcuts it made: one per saved account, and Start with Windows.
Filename: "{app}\{#AppExe}"; Parameters: "--uninstall"; RunOnceId: "RemoveShortcuts"; Flags: runhidden waituntilterminated

[UninstallDelete]
; What a self-update can leave behind.
Type: files; Name: "{app}\{#AppExe}.*.old"
Type: files; Name: "{app}\AntigravityAccountSwitcher.new.exe"
Type: files; Name: "{app}\AntigravityAccountSwitcher.download.zip"

[Code]
const
  EVENT_MODIFY_STATE = $0002;

function OpenEvent(DesiredAccess: Cardinal; InheritHandle: BOOL; Name: String): THandle;
  external 'OpenEventW@kernel32.dll stdcall';
function SetEvent(Event: THandle): BOOL;
  external 'SetEvent@kernel32.dll stdcall';
function CloseHandle(Handle: THandle): BOOL;
  external 'CloseHandle@kernel32.dll stdcall';

{ A running copy keeps going in the tray when its window is closed, so it is asked to quit
  through its quit event and given five seconds. A copy too old to listen is closed by
  CloseApplications. }
procedure AskRunningCopyToQuit;
var
  Event: THandle;
  Waited: Integer;
begin
  Event := OpenEvent(EVENT_MODIFY_STATE, False, 'Local\{#RunningCopy}.quit');
  if Event = 0 then
    Exit;
  SetEvent(Event);
  CloseHandle(Event);
  Waited := 0;
  while CheckForMutexes('Local\{#RunningCopy}.instance') and (Waited < 50) do
  begin
    Sleep(100);
    Waited := Waited + 1;
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  AskRunningCopyToQuit;
  Result := '';
end;

function InitializeUninstall(): Boolean;
begin
  AskRunningCopyToQuit;
  Result := True;
end;
