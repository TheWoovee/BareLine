; SPDX-License-Identifier: MPL-2.0
; Local compile: ISCC /DAppVersion=0.1.0 /DPayloadDir=<absolute> /DOutputDir=<absolute> bareline.iss
#ifndef AppVersion
  #error AppVersion must be supplied
#endif
#ifndef PayloadDir
  #error PayloadDir must be supplied
#endif
#ifndef OutputDir
  #error OutputDir must be supplied
#endif
[Setup]
AppId={{B91880A1-9E41-4868-B472-DF08FD48B7E4}
AppName=Bareline
AppVersion={#AppVersion}
AppPublisher=Bareline
DefaultDirName={autopf}\Bareline
DefaultGroupName=Bareline
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog commandline
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.19045
OutputDir={#OutputDir}
OutputBaseFilename=bareline-{#AppVersion}-windows-x64-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\bareline.exe
CloseApplications=yes
RestartApplications=no
SetupLogging=yes
[Tasks]
Name: "contextmenu"; Description: "Add Open with Bareline to Explorer"; Flags: unchecked
Name: "association"; Description: "Register Bareline as an available text editor (does not change defaults)"; Flags: unchecked
[Files]
Source: "{#PayloadDir}\bareline.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PayloadDir}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PayloadDir}\THIRD-PARTY-NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PayloadDir}\SBOM.json"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PayloadDir}\bareline-update-helper.exe"; DestDir: "{app}"; Flags: ignoreversion
[Icons]
Name: "{group}\Bareline"; Filename: "{app}\bareline.exe"
[Registry]
Root: HKA; Subkey: "Software\Classes\*\shell\Bareline"; ValueType: string; ValueData: "Open with Bareline"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\*\shell\Bareline\command"; ValueType: string; ValueData: """{app}\bareline.exe"" -- ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\Applications\bareline.exe\shell\open\command"; ValueType: string; ValueData: """{app}\bareline.exe"" -- ""%1"""; Flags: uninsdeletekey; Tasks: association
Root: HKA; Subkey: "Software\Classes\Applications\bareline.exe\SupportedTypes"; ValueType: string; ValueName: ".txt"; ValueData: ""; Flags: uninsdeletekey; Tasks: association
; No user data entries in UninstallDelete: configuration, sessions and recovery survive.
