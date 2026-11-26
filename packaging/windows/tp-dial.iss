; tp-dial installer, built with Inno Setup 6 by .github/workflows/windows.yml:
;   iscc /DAppVersion=0.2.0 packaging\windows\tp-dial.iss
; Expects the bundled app in dist\tp-dial (packaging/windows/bundle.sh).
; Per-user install: no administrator rights needed.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{15D75603-585D-4AF8-AC1C-FCADEBAD258F}
AppName=tp-dial
AppVersion={#AppVersion}
AppVerName=tp-dial {#AppVersion}
AppPublisher=Xexaphone
AppPublisherURL=https://github.com/Xexaphone/tp-dial
AppSupportURL=https://github.com/Xexaphone/tp-dial/issues
DefaultDirName={localappdata}\Programs\tp-dial
DefaultGroupName=tp-dial
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\..\dist
OutputBaseFilename=tp-dial-setup
SetupIconFile=tp-dial.ico
UninstallDisplayIcon={app}\tp-dial.ico
UninstallDisplayName=tp-dial
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=no

[Tasks]
Name: autostart; Description: "Start tp-dial when I sign in"

[Files]
Source: "..\..\dist\tp-dial\*"; DestDir: "{app}"; Flags: recursesubdirs createallsubdirs ignoreversion

[Icons]
Name: "{group}\tp-dial"; Filename: "{app}\bin\tp-dial.exe"; IconFilename: "{app}\tp-dial.ico"; Comment: "Start the TrackPoint dial"
Name: "{group}\tp-dial Settings"; Filename: "{app}\bin\tp-dial.exe"; Parameters: "--settings"; IconFilename: "{app}\tp-dial.ico"
Name: "{group}\Find my TrackPoint"; Filename: "{app}\bin\tp-dial.exe"; Parameters: "--find-device"; IconFilename: "{app}\tp-dial.ico"
Name: "{group}\Uninstall tp-dial"; Filename: "{uninstallexe}"

[Registry]
; Same entry as `tp-dial --install-autostart`.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "tp-dial"; ValueData: """{app}\bin\tp-dial.exe"""; Flags: uninsdeletevalue; Tasks: autostart

[Run]
Filename: "{app}\bin\tp-dial.exe"; Parameters: "--find-device"; Description: "Find my TrackPoint now (push it when asked)"; Flags: postinstall waituntilterminated skipifsilent; Check: NeedsDevice
Filename: "{app}\bin\tp-dial.exe"; Description: "Start tp-dial now"; Flags: postinstall nowait skipifsilent

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/IM tp-dial.exe /F"; Flags: runhidden; RunOnceId: "StopTpDial"

[Code]
// Stop a running copy so its files can be replaced (it holds no state worth saving).
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Code: Integer;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/IM tp-dial.exe /F', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Result := '';
end;

// Offer "Find my TrackPoint" only if the config doesn't already name a device.
function NeedsDevice(): Boolean;
var
  Text: AnsiString;
begin
  Result := True;
  if LoadStringFromFile(ExpandConstant('{userappdata}\tp-dial\config.toml'), Text) then
    if (Pos('device = "', Text) > 0) and (Pos('device = ""', Text) = 0) then
      Result := False;
end;
