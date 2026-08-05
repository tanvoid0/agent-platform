; Inno Setup script for Agent Platform (Windows-only desktop shell).
; Per-user install, no admin required — matches a small local-first tool.
;
; Compile with: iscc desktop\installer\agent-platform.iss
; (normally invoked via scripts/build_installer.py, which builds the exe and
; payload first)
;
; Expects, relative to this .iss file's SourceDir (desktop\):
;   target\release\agent-platform.exe   the compiled app (icon already embedded)
;   target\release\*.dll                llama.cpp + ggml, only in a local-llm build
;   payload\                            Python runtime + server (scripts/bundle_server.py)
;   crates\app\icon.ico                 app icon, reused for the installer/shortcuts
;
; payload\ is installed as {app}\server\ — shell.rs's resolve_server() looks for
; the bundled runtime at <exe dir>\server\runtime and <exe dir>\server\scripts.

#define MyAppName "Agent Platform"
#define MyAppVersion "0.2.0"
#define MyAppPublisher "Agent Platform"
#define MyAppExeName "agent-platform.exe"

[Setup]
AppId={{6C6E6F52-6167-4E74-506C-6174666F726D}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
; Per-user, no UAC prompt — installs under the current user's LOCALAPPDATA.
DefaultDirName={localappdata}\Programs\AgentPlatform
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir=..\..\dist
OutputBaseFilename=agent-platform-setup
SetupIconFile=..\crates\app\icon.ico
UninstallDisplayIcon={app}\{#MyAppExeName}
Compression=lzma2
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Additional shortcuts:"; Flags: unchecked

[Files]
Source: "..\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
; The `local-llm` feature forces llama.cpp to build as DLLs (two static ggmls
; will not link), and cargo drops them beside the exe. A default build has none,
; hence skipifsourcedoesntexist — the exe then never looks for them.
Source: "..\target\release\*.dll"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
Source: "..\payload\*"; DestDir: "{app}\server"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\{#MyAppExeName}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "Launch {#MyAppName}"; Flags: nowait postinstall skipifsilent
