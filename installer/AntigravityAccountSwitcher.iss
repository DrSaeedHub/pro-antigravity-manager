; The Windows installer, built by `cargo xtask dist` with Inno Setup 6 or 7.
;
; A per-user install (no administrator rights) into %LOCALAPPDATA%\AntigravityAccountSwitcher:
; the folder the app itself treats as "the installed copy", so its own Install on this PC, its
; account shortcuts, Start with Windows and its self-update all work the same way. Shortcut names
; match the ones the app creates.
;
; Uninstalling: Installed apps runs the uninstaller with /SILENT /ASK, so Inno's own Yes/No box
; never shows; the uninstaller opens the app's own window instead (--uninstall-dialog), which
; asks whether to delete the saved accounts and the settings too, and answers with its exit code.
; Scripted: unins000.exe /VERYSILENT [/DELETEACCOUNTS] [/DELETESETTINGS] asks nothing.
;
; Updating: an installed copy that updated itself records nothing here, so after an update the
; app runs this installer quietly (/VERYSILENT /RELAUNCH) to bring the uninstaller and the
; Installed apps entry to its own version; /RELAUNCH opens the app again at the end.
; InstallerVersion in the Installed apps entry says which installer ran last.
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
#define AppGuid "6F0D6D3C-4C1E-4E7B-9A51-6B7E2C9A1D42"
; The single-copy names of crates/desktop/src/instance.rs.
#define RunningCopy "AntigravityAccountSwitcher"

[Setup]
AppId={{{#AppGuid}}
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

[Messages]
; Shown only when unins000.exe is started by hand (not from Installed apps), after the app's window.
ConfirmUninstall=Remove %1 now?

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
; After a quiet install started by the app itself: open it again.
Filename: "{app}\{#AppExe}"; Parameters: "--updated setup"; Flags: nowait; Check: HasSwitch('/RELAUNCH')

[UninstallDelete]
; What a self-update can leave behind.
Type: files; Name: "{app}\{#AppExe}.*.old"
Type: files; Name: "{app}\AntigravityAccountSwitcher.new.exe"
Type: files; Name: "{app}\AntigravityAccountSwitcher.download.zip"
Type: filesandordirs; Name: "{app}\updates"

[Code]
const
  EVENT_MODIFY_STATE = $0002;
  { The exit codes of --uninstall-dialog (crates/app/src/uninstall.rs). }
  DIALOG_REMOVE = 20;

var
  DeleteAccounts: Boolean;
  DeleteSettings: Boolean;
  Asked: Boolean;

function OpenEvent(DesiredAccess: Cardinal; InheritHandle: BOOL; Name: String): THandle;
  external 'OpenEventW@kernel32.dll stdcall';
function SetEvent(Event: THandle): BOOL;
  external 'SetEvent@kernel32.dll stdcall';
function CloseHandle(Handle: THandle): BOOL;
  external 'CloseHandle@kernel32.dll stdcall';

{ Whether the command line has the switch Name (any letter case). }
function HasSwitch(Name: String): Boolean;
var
  I: Integer;
begin
  Result := False;
  for I := 1 to ParamCount do
    if CompareText(ParamStr(I), Name) = 0 then
      Result := True;
end;

function UninstallKey(): String;
begin
  Result := 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{{#AppGuid}}_is1';
end;

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

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
  begin
    { Installed apps starts the uninstaller quietly with /ASK: no Inno box, the app's window. }
    RegWriteStringValue(HKA, UninstallKey(), 'UninstallString',
      '"' + ExpandConstant('{uninstallexe}') + '" /SILENT /ASK');
    RegWriteStringValue(HKA, UninstallKey(), 'InstallerVersion', '{#AppVersion}');
  end;
end;

{ Asks with the app's own window. False when the person keeps the app. A window that cannot
  open falls back to a plain question. }
function AskWhatToRemove(): Boolean;
var
  Code: Integer;
begin
  if Exec(ExpandConstant('{app}\{#AppExe}'), '--uninstall-dialog', '', SW_SHOWNORMAL,
          ewWaitUntilTerminated, Code) and (Code = 0) then
    Result := False
  else if (Code >= DIALOG_REMOVE) and (Code <= DIALOG_REMOVE + 3) then
  begin
    DeleteAccounts := ((Code - DIALOG_REMOVE) and 1) <> 0;
    DeleteSettings := ((Code - DIALOG_REMOVE) and 2) <> 0;
    Result := True;
  end
  else
    Result := MsgBox('Remove {#AppName}? Your saved accounts and settings stay on this PC.',
                     mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES;
end;

function InitializeUninstall(): Boolean;
begin
  DeleteAccounts := HasSwitch('/DELETEACCOUNTS');
  DeleteSettings := HasSwitch('/DELETESETTINGS');
  Asked := HasSwitch('/ASK') or not UninstallSilent;
  Result := True;
  if Asked then
    Result := AskWhatToRemove();
  if Result then
    AskRunningCopyToQuit;
end;

{ The app removes the shortcuts it made (one per saved account, and Start with Windows) and, when
  asked, the saved accounts themselves. Run from here rather than [UninstallRun], whose
  parameters are fixed when installing, before anyone has chosen. }
procedure CleanUp;
var
  Parameters: String;
  Code: Integer;
begin
  Parameters := '--uninstall';
  if DeleteAccounts then
    Parameters := Parameters + ' --delete-accounts';
  if not Exec(ExpandConstant('{app}\{#AppExe}'), Parameters, '', SW_HIDE, ewWaitUntilTerminated, Code) then
    Log('The app could not clean up: ' + SysErrorMessage(Code));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Kept: String;
begin
  if CurUninstallStep = usUninstall then
    CleanUp;
  if (CurUninstallStep = usPostUninstall) and DeleteSettings then
    DelTree(ExpandConstant('{app}'), True, True, True);
  if (CurUninstallStep = usDone) and Asked and UninstallSilent then
  begin
    if DeleteAccounts and DeleteSettings then
      Kept := 'Your saved accounts and settings were deleted too.'
    else if DeleteAccounts then
      Kept := 'Your saved accounts were deleted too; your settings stay.'
    else if DeleteSettings then
      Kept := 'Your settings were deleted too; your saved accounts stay.'
    else
      Kept := 'Your saved accounts and settings stay on this PC.';
    MsgBox('{#AppName} was removed. ' + Kept, mbInformation, MB_OK);
  end;
end;
