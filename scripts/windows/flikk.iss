; Inno Setup script for Flikk's Windows installer, built by
; scripts/package.sh:
;
;   ISCC /DVersion=0.2.0 /DSourceDir=<folder with flikk.exe>
;        /DOutputDir=<dist> /DOutputName=flikk-0.2.0-windows-x86_64-setup
;        scripts\windows\flikk.iss
;
; It installs for the current user only, into %LOCALAPPDATA%\Programs\Flikk,
; without asking for an administrator: a folder Flikk can write to, so its
; in-app updates can replace its files (crates/flikk/src/update.rs).

#ifndef Version
  #error Pass /DVersion=X.Y.Z
#endif
#ifndef SourceDir
  #error Pass /DSourceDir=<folder with flikk.exe>
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif
#ifndef OutputName
  #define OutputName "flikk-" + Version + "-windows-x86_64-setup"
#endif

[Setup]
; The same for every version, so each installs over the last.
AppId={{D2AAF4C9-9CDB-43CA-9C1E-ECC4A522F391}
AppName=Flikk
AppVersion={#Version}
AppVerName=Flikk {#Version}
AppPublisher=Flikk
VersionInfoVersion={#Version}
; Per user: {autopf} is %LOCALAPPDATA%\Programs without an administrator.
PrivilegesRequired=lowest
DefaultDirName={autopf}\Flikk
DisableProgramGroupPage=yes
UsePreviousAppDir=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename={#OutputName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\flikk.exe
UninstallDisplayName=Flikk
ChangesAssociations=yes
CloseApplications=yes

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\Flikk"; Filename: "{app}\flikk.exe"
Name: "{autodesktop}\Flikk"; Filename: "{app}\flikk.exe"; Tasks: desktopicon

; .flikk projects open in Flikk (`flikk Film.flikk`); HKA is the user's
; own classes in a per-user install.
[Registry]
Root: HKA; Subkey: "Software\Classes\.flikk"; ValueType: string; ValueName: ""; ValueData: "Flikk.Project"; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\Flikk.Project"; ValueType: string; ValueName: ""; ValueData: "Flikk Project"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Flikk.Project\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\flikk.exe,0"
Root: HKA; Subkey: "Software\Classes\Flikk.Project\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\flikk.exe"" ""%1"""

[Run]
Filename: "{app}\flikk.exe"; Description: "{cm:LaunchProgram,Flikk}"; Flags: nowait postinstall skipifsilent

; In-app updates may add libraries the installer never listed, and leave
; files they moved aside. User data in %USERPROFILE%\.flikk stays.
[UninstallDelete]
Type: files; Name: "{app}\*.exe"
Type: files; Name: "{app}\*.dll"
Type: files; Name: "{app}\*.flikk-old"
Type: files; Name: "{app}\*.flikk-new"
Type: dirifempty; Name: "{app}"
