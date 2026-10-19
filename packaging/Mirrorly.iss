; Inno Setup script for Mirrorly. Built by packaging/build-release.ps1:
;   ISCC /DAppVersion=0.1.0 /DStageDir=<staged files> packaging\Mirrorly.iss

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef StageDir
  #define StageDir "..\dist\stage"
#endif

[Setup]
AppId={{8C5E2C1B-4F3A-4E7B-9A51-6B2D1E0F7A11}
AppName=Mirrorly
AppVersion={#AppVersion}
AppVerName=Mirrorly {#AppVersion}
AppPublisher=AnasSAV
AppPublisherURL=https://github.com/AnasSAV/mirrorly
AppSupportURL=https://github.com/AnasSAV/mirrorly/issues
AppUpdatesURL=https://github.com/AnasSAV/mirrorly/releases
; Per-user install: no administrator prompt.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
DefaultDirName={autopf}\Mirrorly
DisableProgramGroupPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Windows 10 1809+ (flip-model tearing, Windows 11 recommended).
MinVersion=10.0.17763
LicenseFile=..\LICENSE
SetupIconFile=mirrorly.ico
UninstallDisplayIcon={app}\Mirrorly.exe
OutputDir=..\dist
OutputBaseFilename=Mirrorly-Setup-{#AppVersion}
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Messages]
WelcomeLabel2=This will install [name/ver] on your computer.%n%nMirrorly shows your iPhone's or iPad's screen on this PC and lets you control it with your mouse and keyboard, over USB or Wi-Fi.
FinishedLabel=Mirrorly is installed.%n%nTo connect your iPhone or iPad (iOS / iPadOS 27 or later):%n%n1.  Install Apple Devices from the Microsoft Store (free).%n2.  Plug it in, unlock it and tap Trust.%n3.  Turn on Developer Mode: Settings > Privacy & Security > Developer Mode.%n%nMirrorly shows these steps too, and ticks them off as you go.

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#StageDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs

[Icons]
Name: "{autoprograms}\Mirrorly"; Filename: "{app}\Mirrorly.exe"
Name: "{autodesktop}\Mirrorly"; Filename: "{app}\Mirrorly.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\Mirrorly.exe"; Description: "{cm:LaunchProgram,Mirrorly}"; Flags: nowait postinstall skipifsilent

[Code]
const
  AppleService = 'SYSTEM\CurrentControlSet\Services\Apple Mobile Device Service';
  AppleDevicesStore = 'ms-windows-store://pdp/?productid=9NP83LWLPZ9K';

// Mirrorly talks to the iPhone through Apple's USB driver service, which
// comes with the free Apple Devices app (or iTunes).
procedure CurStepChanged(CurStep: TSetupStep);
var
  Code: Integer;
begin
  if (CurStep = ssPostInstall) and not WizardSilent and not RegKeyExists(HKLM, AppleService) then
  begin
    if MsgBox('Mirrorly needs Apple Devices (free, from the Microsoft Store). ' +
              'It provides the driver Windows uses to talk to an iPhone.' + #13#10#13#10 +
              'Open the Microsoft Store page now?', mbConfirmation, MB_YESNO) = IDYES then
      ShellExec('open', AppleDevicesStore, '', '', SW_SHOWNORMAL, ewNoWait, Code);
  end;
end;
