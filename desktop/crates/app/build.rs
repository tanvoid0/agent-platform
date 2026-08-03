// Embeds icon.ico into agent-platform.exe on Windows (Phase 5 packaging).
// No-op on other platforms — this app has only ever shipped for Windows.
fn main() {
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("icon.ico");
        if let Err(e) = res.compile() {
            // Fail the build loudly rather than shipping an unbranded exe silently.
            panic!("failed to embed icon.ico: {e}");
        }
    }
}
