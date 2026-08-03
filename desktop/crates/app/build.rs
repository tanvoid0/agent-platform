// Embeds icon.ico into agent-platform.exe on Windows (Phase 5 packaging).
// No-op on other platforms — this app has only ever shipped for Windows.
fn main() {
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
