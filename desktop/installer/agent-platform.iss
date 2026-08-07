; Inno Setup script for Agent Platform (Windows-only desktop shell).
; Per-user install, no admin required — matches a small local-first tool.
;
; Compile with: iscc desktop\installer\agent-platform.iss
; (normally invoked via scripts/build_installer.py, which builds the exes first)
;
; Expects, relative to this .iss file's SourceDir (desktop\):
;   target\release\agent-platform.exe   the compiled app (icon already embedded)
;   target\release\agent-platformd.exe  the API server the app spawns (ADR 0007)
;   target\release\*.dll                llama.cpp + ggml, only in a local-llm build
;   ..\worker\                          the model-ops build worker (Python sources)
;   crates\app\icon.ico                 app icon, reused for the installer/shortcuts
;
; **No Python runtime ships any more.** The server used to be a Python process
; this installer carried an embedded CPython for, under payload\. It is Rust
; now. The only Python left is the LoRA training worker, which needs torch and
; therefore an interpreter the user points at with MODEL_OPS_PYTHON — so the
; worker's own sources ship (they are small and pure) and the interpreter does
; not. Model-ops build jobs are the only feature that needs one.

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
; Without this the app starts, finds no server binary, and serves nothing.
Source: "..\target\release\agent-platformd.exe"; DestDir: "{app}"; Flags: ignoreversion
; The `local-llm` feature forces llama.cpp to build as DLLs (two static ggmls
; will not link), and cargo drops them beside the exe. A default build has none,
; hence skipifsourcedoesntexist — the exe then never looks for them.
Source: "..\target\release\*.dll"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
; The model-ops build worker — a few hundred lines of Python that
; agent-platformd runs as a subprocess per pipeline stage. It looks for this at
; <exe dir>\worker unless MODEL_OPS_WORKER_PATH says otherwise.
Source: "..\..\worker\*"; DestDir: "{app}\worker"; Flags: ignoreversion recursesubdirs createallsubdirs

[UninstallDelete]
; Inno removes only the files it installed, and Python leaves __pycache__
; directories that were never in the manifest — one un-removable directory
; strands the whole tree above it, and the uninstaller then exits 0 having left
; the files behind, which is worse than failing because nothing says so. This
; used to guard ~50 MB of bundled runtime under {app}\server; the worker is far
; smaller but caches the same way.
Type: filesandordirs; Name: "{app}\worker"

[Icons]
Name: "{autoprograms}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\{#MyAppExeName}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "Launch {#MyAppName}"; Flags: nowait postinstall skipifsilent
