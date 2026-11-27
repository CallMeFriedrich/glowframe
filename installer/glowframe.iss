; Glowframe installer (SPEC 9): per-user, no admin rights, start menu entry,
; uninstaller that optionally removes settings and cache. No autostart.
; Built by scripts/package.ps1, which passes /DAppVersion=<version>.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{6C1E5B9A-3F7D-4D2E-9B8A-1F4C2E7A9D30}
AppName=Glowframe
AppVersion={#AppVersion}
AppVerName=Glowframe {#AppVersion}
AppPublisher=Glowframe
DefaultDirName={autopf}\Glowframe
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.19041
; Ask the user to close a running instance (named mutex of the app).
AppMutex=Local\Glowframe.Instance
CloseApplications=no
OutputDir=..\dist
OutputBaseFilename=Glowframe-{#AppVersion}-setup
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\Glowframe.exe
UninstallDisplayName=Glowframe
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern

[Languages]
Name: "de"; MessagesFile: "compiler:Languages\German.isl"
Name: "en"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
de.DeleteData=Sollen auch die Einstellungen, der Cache (Cover, Lyrics) und die Logs von Glowframe gelöscht werden?
en.DeleteData=Do you also want to delete Glowframe's settings, cache (covers, lyrics) and logs?

[Files]
Source: "..\dist\stage\Glowframe.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Glowframe"; Filename: "{app}\Glowframe.exe"

[Run]
Filename: "{app}\Glowframe.exe"; Description: "{cm:LaunchProgram,Glowframe}"; Flags: nowait postinstall skipifsilent
; In-app update (SPEC 6.5): silent install with /relaunch=1 restarts the app.
Filename: "{app}\Glowframe.exe"; Parameters: "--tray"; Flags: nowait; Check: ShouldRelaunch

[Code]
function ShouldRelaunch: Boolean;
begin
  Result := WizardSilent and (ExpandConstant('{param:relaunch|0}') = '1');
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if (CurUninstallStep = usPostUninstall) and not UninstallSilent then
  begin
    if MsgBox(CustomMessage('DeleteData'), mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
    begin
      DelTree(ExpandConstant('{userappdata}\Glowframe'), True, True, True);
      DelTree(ExpandConstant('{localappdata}\Glowframe'), True, True, True);
    end;
  end;
end;
