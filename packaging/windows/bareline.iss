; SPDX-License-Identifier: MPL-2.0
#if Ver != EncodeVer(6, 4, 3)
  #error Expected pinned Inno Setup 6.4.3
#endif
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
AppPublisherURL=https://github.com/TheWoovee/BareLine
; Privacy summary page; the full policy is PRIVACY.md in the repository.
InfoBeforeFile=installer-privacy.txt
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
SetupIconFile=bareline.ico
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
#if FileExists(PayloadDir + "\bareline.release-authority.json")
Source: "{#PayloadDir}\bareline.release-authority.json"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PayloadDir}\bareline.release-authority.minisig"; DestDir: "{app}"; Flags: ignoreversion
#endif
#if FileExists(PayloadDir + "\bareline.root-transitions.json")
Source: "{#PayloadDir}\bareline.root-transitions.json"; DestDir: "{app}"; Flags: ignoreversion
#endif
[Icons]
Name: "{group}\Bareline"; Filename: "{app}\bareline.exe"; IconFilename: "{app}\bareline.exe"
[Registry]
Root: HKA; Subkey: "Software\Classes\*\shell\Bareline"; ValueType: string; ValueData: "Open with Bareline"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\*\shell\Bareline\command"; ValueType: string; ValueData: """{app}\bareline.exe"" -- ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\Applications\bareline.exe"; Flags: uninsdeletekey; Tasks: association
Root: HKA; Subkey: "Software\Classes\Applications\bareline.exe\shell\open\command"; ValueType: string; ValueData: """{app}\bareline.exe"" -- ""%1"""; Flags: uninsdeletekey; Tasks: association
Root: HKA; Subkey: "Software\Classes\Applications\bareline.exe\SupportedTypes"; ValueType: string; ValueName: ".txt"; ValueData: ""; Flags: uninsdeletekey; Tasks: association
; No user data entries in UninstallDelete: configuration, sessions and recovery survive.
; Update helper output (SEC-17): staged, backup and retained executables and receipts,
; delivered trust staging and journals, legacy install-root ledgers and lock, and the
; per-user update state (ledgers, lock, launch counter). Nothing else under Bareline.
; SignedUninstaller needs the owner's certificate at compile time. This script has no
; signing switch yet, so the uninstaller stays unsigned (deferred to P3-01 signing).
[UninstallDelete]
Type: files; Name: "{app}\bareline.pending.exe"
Type: files; Name: "{app}\bareline.rollback.exe"
Type: files; Name: "{app}\bareline.failed.exe"
Type: files; Name: "{app}\bareline.update.json"
Type: files; Name: "{app}\bareline.update.minisig"
Type: files; Name: "{app}\bareline.update-journal"
Type: files; Name: "{app}\bareline.trust-journal"
Type: files; Name: "{app}\bareline.pending-*"
Type: files; Name: "{app}\*.retained-*"
Type: files; Name: "{app}\bareline.root-transitions.json"
Type: files; Name: "{app}\bareline.update-lock"
Type: files; Name: "{app}\bareline.update-versions"
Type: files; Name: "{app}\bareline.root-keys"
Type: files; Name: "{app}\bareline.root-versions"
Type: filesandordirs; Name: "{localappdata}\Bareline\update"
