; Inno Setup script for Rewrite. Normally compiled by  cargo xtask package,
; which passes /DVersion, /DSrc, /DOutDir and /DIcon. The program is in bin\, next to GTK's DLLs.
#ifndef Version
  #define Version "2.0.1"
#endif
#ifndef Src
  #define Src "..\..\build\app\Rewrite"
#endif
#ifndef OutDir
  #define OutDir "..\..\dist\installers"
#endif
#ifndef Icon
  #define Icon "..\..\build\rewrite.ico"
#endif

[Setup]
AppId={{6E4C2A91-3B7F-4D8E-A15C-9F02B7D6E318}
AppName=Rewrite
AppVersion={#Version}
AppPublisher=Samrat Dutta
AppPublisherURL=https://github.com/samratduttaofficial
AppSupportURL=https://github.com/samratduttaofficial
AppCopyright=Copyright (C) Samrat Dutta
VersionInfoCompany=Samrat Dutta
VersionInfoDescription=Rewrite Setup
DefaultDirName={autopf}\Rewrite
DefaultGroupName=Rewrite
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutDir}
OutputBaseFilename=Rewrite-{#Version}-Windows-Setup
SetupIconFile={#Icon}
UninstallDisplayIcon={app}\bin\Rewrite.exe
UninstallDisplayName=Rewrite
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ChangesAssociations=yes

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#Src}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[InstallDelete]
; what the earlier (Python) versions installed
Type: filesandordirs; Name: "{app}\_internal"
Type: files; Name: "{app}\Rewrite.exe"

[Icons]
Name: "{autoprograms}\Rewrite"; Filename: "{app}\bin\Rewrite.exe"
Name: "{autodesktop}\Rewrite"; Filename: "{app}\bin\Rewrite.exe"; Tasks: desktopicon

[Registry]
; Adds Rewrite to "Open with" for text files.
Root: HKA; Subkey: "Software\Classes\Applications\Rewrite.exe"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "Rewrite"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Applications\Rewrite.exe\shell\open\command"; ValueType: string; ValueData: """{app}\bin\Rewrite.exe"" ""%1"""
Root: HKA; Subkey: "Software\Classes\Applications\Rewrite.exe\SupportedTypes"; ValueType: string; ValueName: ".txt"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\Rewrite.exe\SupportedTypes"; ValueType: string; ValueName: ".md"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\Rewrite.exe\SupportedTypes"; ValueType: string; ValueName: ".tex"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\Rewrite.exe\SupportedTypes"; ValueType: string; ValueName: ".docx"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\Rewrite.exe\SupportedTypes"; ValueType: string; ValueName: ".doc"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\Rewrite.exe\SupportedTypes"; ValueType: string; ValueName: ".odt"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\Rewrite.exe\SupportedTypes"; ValueType: string; ValueName: ".rtf"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\Rewrite.exe\SupportedTypes"; ValueType: string; ValueName: ".pdf"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\.txt\OpenWithList\Rewrite.exe"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\.md\OpenWithList\Rewrite.exe"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\.tex\OpenWithList\Rewrite.exe"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\.docx\OpenWithList\Rewrite.exe"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\.doc\OpenWithList\Rewrite.exe"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\.odt\OpenWithList\Rewrite.exe"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\.rtf\OpenWithList\Rewrite.exe"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\.pdf\OpenWithList\Rewrite.exe"; Flags: uninsdeletekey

[Run]
Filename: "{app}\bin\Rewrite.exe"; Description: "{cm:LaunchProgram,Rewrite}"; Flags: nowait postinstall skipifsilent
