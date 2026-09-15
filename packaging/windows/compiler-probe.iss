; SPDX-License-Identifier: MPL-2.0
; Compile with /O- to validate the executing compiler without emitting a setup.
#if Ver != EncodeVer(6, 4, 3)
  #error Expected pinned Inno Setup 6.4.3
#endif
[Setup]
AppName=Bareline compiler probe
AppVersion=0.1.0
DefaultDirName={localappdata}\BarelineCompilerProbe
PrivilegesRequired=lowest
Uninstallable=no
CreateAppDir=no
