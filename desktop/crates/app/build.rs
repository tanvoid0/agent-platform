// Embeds icon.ico into agent-platform.exe on Windows (Phase 5 packaging).
// No-op on other platforms — this app has only ever shipped for Windows.
fn main() {
    #[cfg(feature = "local-llm")]
    copy_llama_dlls();

    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("icon.ico");
        // Embedding any resource replaces the linker's default manifest, which
        // is where rfd's common-controls-v6 dependency lived. Without it the
        // loader binds comctl32 v5, which lacks TaskDialogIndirect, and the exe
        // dies at startup with STATUS_ENTRYPOINT_NOT_FOUND.
        res.set_manifest(
            r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0" processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
</assembly>"#,
        );
        if let Err(e) = res.compile() {
            // Fail the build loudly rather than shipping an unbranded exe silently.
            panic!("failed to embed icon.ico: {e}");
        }
    }
}

/// `local-llm` builds llama.cpp as DLLs (it has to — see the feature's comment
/// in Cargo.toml), and cargo leaves them in the sys crate's `OUT_DIR`, where
/// neither the exe nor `cargo test` can find them. Copy them next to the
/// binaries, which is also where they have to ship from.
///
/// `llama-cpp-sys-2` publishes no metadata key for that directory, so it is
/// found by walking up from our own `OUT_DIR` — both live under
/// `target/<profile>/build/<crate>-<hash>/out`.
#[cfg(feature = "local-llm")]
fn copy_llama_dlls() {
    use std::path::PathBuf;

    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    // .../target/<profile>/build/<us>-<hash>/out → .../target/<profile>
    let Some(profile_dir) = out.ancestors().nth(3) else { return };
    let build_dir = profile_dir.join("build");
    let Ok(entries) = std::fs::read_dir(&build_dir) else { return };

    let mut copied = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("llama-cpp-sys-2-") {
            continue;
        }
        let Ok(dlls) = std::fs::read_dir(entry.path().join("out").join("bin")) else { continue };
        for dll in dlls.flatten() {
            if dll.path().extension().is_some_and(|e| e.eq_ignore_ascii_case("dll")) {
                // `deps/` is where the test harness runs from; the profile root
                // is where the app runs from. Both need them.
                for dest in [profile_dir.to_path_buf(), profile_dir.join("deps")] {
                    let _ = std::fs::copy(dll.path(), dest.join(dll.file_name()));
                }
                copied += 1;
            }
        }
    }
    if copied == 0 {
        println!("cargo:warning=local-llm: no llama.cpp DLLs found under {}", build_dir.display());
    }
}
