; uRec installer (Inno Setup 6). Built by ci/build-installer.ps1.
; Per-user install by default: no administrator rights needed.

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\dist\uRec"
#endif

#define AppName "uRec"
#define AppExe "uRec.exe"

[Setup]
AppId={{5B7E2C64-9A1D-4F3B-B8E1-2D6C0A9F4E17}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppName}
AppPublisherURL=https://github.com/u2alr/uRec
AppSupportURL=https://github.com/u2alr/uRec/issues
DefaultDirName={autopf}\{#AppName}
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
OutputDir=..\dist
OutputBaseFilename=uRec-{#AppVersion}-setup
SetupIconFile=..\crates\app\assets\app.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
WizardStyle=modern
Compression=lzma2/ultra64
SolidCompression=yes
CloseApplications=yes
RestartApplications=no
LicenseFile={#SourceDir}\LICENSE.txt

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked
Name: "startup"; Description: "Start uRec when I sign in to Windows"; Flags: unchecked

[Files]
Source: "{#SourceDir}\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\ffmpeg.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\THIRD-PARTY-NOTICES.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\FFMPEG-LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\README.txt"; DestDir: "{app}"; Flags: ignoreversion isreadme

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "{#AppName}"; ValueData: """{app}\{#AppExe}"" --minimized"; Tasks: startup
; Remove the startup entry on uninstall, even if it was turned on from inside the app.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueName: "{#AppName}"; Flags: dontcreatekey uninsdeletevalue

[Run]
Filename: "{app}\{#AppExe}"; Description: "Open uRec"; Flags: nowait postinstall skipifsilent
; Automatic updates (uRec runs this installer with /VERYSILENT /update=1 /relaunch=...).
Filename: "{app}\{#AppExe}"; Flags: nowait runasoriginaluser; Check: Relaunch('1')
Filename: "{app}\{#AppExe}"; Parameters: "--minimized"; Flags: nowait runasoriginaluser; Check: Relaunch('min')

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/im {#AppExe} /t"; Flags: runhidden; RunOnceId: "CloseApp"

[UninstallDelete]
; Temporary data only: the Instant Replay buffer, thumbnails and logs.
Type: filesandordirs; Name: "{localappdata}\uRec"

[Code]
var
  RemoveSettings: Boolean;

function Relaunch(Mode: String): Boolean;
begin
  Result := ExpandConstant('{param:relaunch|0}') = Mode;
end;

function InitializeSetup(): Boolean;
var
  Waited: Integer;
begin
  // During an automatic update uRec has just been asked to quit; give it time to finish.
  if ExpandConstant('{param:update|0}') = '1' then
  begin
    Waited := 0;
    while CheckForMutexes('Local\uRec.SingleInstance') and (Waited < 30000) do
    begin
      Sleep(250);
      Waited := Waited + 250;
    end;
  end;
  Result := True;
end;

function InitializeUninstall(): Boolean;
begin
  RemoveSettings := False;
  if not UninstallSilent() then
    RemoveSettings := MsgBox('Also remove your uRec settings?' + #13#10#13#10 +
      'Your recordings are never deleted by uninstalling.', mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES;
  Result := True;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if (CurUninstallStep = usPostUninstall) and RemoveSettings then
    DelTree(ExpandConstant('{userappdata}\uRec'), True, True, True);
end;
