//! Pull a GGUF straight off Hugging Face into the app's data dir.
//!
//! The gap this closes: [`local_llm`](crate::local_llm) will answer with any
//! GGUF on disk, but until now getting one there meant pulling it through
//! Ollama and pointing the picker at a `blobs/sha256-…` file. Hugging Face
//! serves the weights over a plain redirecting `GET`, so this is a download and
//! a rename — not an SDK, not a registry client.
//!
//! Deliberately not here: repo search, resume, and a hash check. All three want
//! the HF API rather than the CDN, and the failure they cover (a 20 GB transfer
//! dying at 90%) has not happened yet. What *is* here is the `.part` rename,
//! because the alternative is a truncated file quietly becoming
//! `local_model_path` and llama.cpp failing on it later.

use futures::{SinkExt, Stream, StreamExt};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

/// Bytes between progress messages. Chunks arrive far too fast to redraw on.
const TICK: u64 = 4 << 20;

#[derive(Debug, Clone)]
pub enum Progress {
    Downloading { received: u64, total: Option<u64> },
    /// The finished file, ready to be `local_model_path`.
    Done(String),
    Failed(String),
}

/// What the card needs to draw the row, all of it derived from [`Progress`].
#[derive(Default)]
pub struct State {
    pub input: String,
    pub active: bool,
    pub received: u64,
    pub total: Option<u64>,
    pub error: Option<String>,
    /// Aborts the transfer. Dropping the stream is the whole cancel — but it
    /// drops it mid-write, so the half-file below has to be swept by hand.
    pub handle: Option<iced::task::Handle>,
    pub part: Option<PathBuf>,
}

/// A pasted reference to a GGUF → the URL to fetch and the name to save it as.
///
/// Three spellings, because all three are things a person actually has in the
/// clipboard: the file's download link, the *page* it sits on (`/blob/`, which
/// serves HTML — rewritten rather than rejected, since the difference is
/// invisible in the address bar), and the `owner/repo/file.gguf` shorthand the
/// model cards print.
pub fn resolve(input: &str) -> Result<(String, String), String> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err("Paste a Hugging Face link or owner/repo/file.gguf.".into());
    }

    let url = if raw.starts_with("https://") || raw.starts_with("http://") {
        // `/blob/` is the browser's view of the file; `/resolve/` is the file.
        raw.replacen("/blob/", "/resolve/", 1)
    } else {
        let mut parts = raw.trim_start_matches('/').splitn(3, '/');
        match (parts.next(), parts.next(), parts.next()) {
            (Some(owner), Some(repo), Some(path))
                if !owner.is_empty() && !repo.is_empty() && !path.is_empty() =>
            {
                format!("https://huggingface.co/{owner}/{repo}/resolve/main/{path}")
            }
            _ => return Err(format!("Not a model reference: {raw}")),
        }
    };

    let name = url
        .split('?')
        .next()
        .unwrap_or(&url)
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();
    if !name.to_ascii_lowercase().ends_with(".gguf") {
        return Err(format!("{name} is not a .gguf — pick the file, not the repo."));
    }
    Ok((url, name))
}

/// Where the transfer writes until it is whole. Named here rather than inside
/// [`fetch`] because a cancelled download is dropped mid-write and somebody
/// outside has to delete what it left.
pub fn part_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.part"))
}

/// Stream the file to `dir/name`, reporting as it goes. Every exit is a
/// terminal [`Progress`], so the caller can clear its spinner on any of them.
pub fn download(url: String, dir: PathBuf, name: String) -> impl Stream<Item = Progress> {
    iced::stream::channel(16, async move |mut out| {
        let msg = match fetch(&url, &dir, &name, &mut out).await {
            Ok(path) => Progress::Done(path),
            Err(e) => Progress::Failed(e),
        };
        let _ = out.send(msg).await;
    })
}

async fn fetch(
    url: &str,
    dir: &Path,
    name: &str,
    out: &mut futures::channel::mpsc::Sender<Progress>,
) -> Result<String, String> {
    tokio::fs::create_dir_all(dir).await.map_err(|e| e.to_string())?;
    let final_path = dir.join(name);
    let part = part_path(dir, name);

    let resp = reqwest::Client::new()
        .get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    if !status.is_success() {
        return Err(match status {
            // The one failure worth naming: nothing here sends a token, so a
            // gated repo answers this way no matter how right the URL is.
            reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
                format!("{status} — a gated repo needs a token, and this sends none.")
            }
            _ => format!("{status} from {url}"),
        });
    }
    let total = resp.content_length();

    let write = async {
        let mut file = tokio::fs::File::create(&part).await?;
        let mut received = 0u64;
        let mut ticked = 0u64;
        let mut body = resp.bytes_stream();
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(std::io::Error::other)?;
            file.write_all(&chunk).await?;
            received += chunk.len() as u64;
            if received - ticked >= TICK {
                ticked = received;
                let _ = out.send(Progress::Downloading { received, total }).await;
            }
        }
        file.flush().await?;
        Ok::<_, std::io::Error>(())
    };

    match write.await {
        Ok(()) => {
            tokio::fs::rename(&part, &final_path)
                .await
                .map_err(|e| e.to_string())?;
            Ok(final_path.display().to_string())
        }
        Err(e) => {
            // A half-written `.part` is worth nothing and costs gigabytes.
            let _ = tokio::fs::remove_file(&part).await;
            Err(e.to_string())
        }
    }
}

/// `12.3 GB` — for a progress line, not for arithmetic.
///
/// The only part of this module the view owns, so it is the only part a build
/// without the engine has no caller for.
#[cfg_attr(not(feature = "local-llm"), allow(dead_code))]
pub fn human(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut n = bytes as f64;
    let mut unit = 0;
    while n >= 1024.0 && unit < UNITS.len() - 1 {
        n /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{n:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shorthand_becomes_a_resolve_url() {
        let (url, name) = resolve("unsloth/Qwen3-8B-GGUF/Qwen3-8B-Q4_K_M.gguf").unwrap();
        assert_eq!(
            url,
            "https://huggingface.co/unsloth/Qwen3-8B-GGUF/resolve/main/Qwen3-8B-Q4_K_M.gguf"
        );
        assert_eq!(name, "Qwen3-8B-Q4_K_M.gguf");
    }

    #[test]
    fn a_blob_page_url_is_rewritten_to_the_file() {
        let (url, _) =
            resolve("https://huggingface.co/unsloth/Qwen3-8B-GGUF/blob/main/q4.gguf").unwrap();
        assert!(url.contains("/resolve/main/"), "{url}");
        assert!(!url.contains("/blob/"), "{url}");
    }

    #[test]
    fn a_download_url_is_left_alone_and_query_stripped_from_the_name() {
        let (url, name) =
            resolve("https://huggingface.co/o/r/resolve/main/sub/q4.gguf?download=true").unwrap();
        assert!(url.ends_with("?download=true"), "{url}");
        assert_eq!(name, "q4.gguf");
    }

    #[test]
    fn nested_paths_survive_the_shorthand() {
        let (url, name) = resolve("o/r/split/model-00001-of-00002.gguf").unwrap();
        assert!(url.ends_with("/resolve/main/split/model-00001-of-00002.gguf"), "{url}");
        assert_eq!(name, "model-00001-of-00002.gguf");
    }

    #[test]
    fn a_repo_without_a_file_is_refused() {
        assert!(resolve("unsloth/Qwen3-8B-GGUF").is_err());
        assert!(resolve("https://huggingface.co/unsloth/Qwen3-8B-GGUF").is_err());
        assert!(resolve("   ").is_err());
    }

    /// The only part the parser tests cannot reach: a real redirect, a real
    /// `Content-Length`, and the `.part` rename. Opt-in — CI does not have the
    /// network and should not want it.
    ///
    /// ```bash
    /// cargo test -p agent-platform-desktop model_download -- --ignored --nocapture
    /// ```
    #[tokio::test]
    #[ignore = "hits huggingface.co"]
    async fn a_real_gguf_lands_whole() {
        // 19 MB of tinyllamas, from the repo that publishes llama.cpp's own
        // test weights — small enough to be a test, real enough to be a GGUF.
        let (url, name) =
            resolve("ggml-org/models/tinyllamas/stories15M-q4_0.gguf").expect("resolves");
        let dir = std::env::temp_dir().join("agp-model-download-test");
        let _ = std::fs::remove_dir_all(&dir);

        let mut last = None;
        let mut ticks = 0;
        let mut stream = std::pin::pin!(download(url, dir.clone(), name.clone()));
        while let Some(p) = stream.next().await {
            if matches!(p, Progress::Downloading { .. }) {
                ticks += 1;
            }
            eprintln!("{p:?}");
            last = Some(p);
        }

        let path = match last {
            Some(Progress::Done(p)) => p,
            other => panic!("expected Done, got {other:?}"),
        };
        assert_eq!(std::path::Path::new(&path), dir.join(&name));
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 19_077_344);
        assert!(!dir.join(format!("{name}.part")).exists(), "the .part survived");
        assert!(ticks >= 4, "19 MB should tick at least 4 times, got {ticks}");
        // A GGUF starts with its magic; a redirect page served as the file
        // would pass every check above and none of this one.
        let head = std::fs::read(&path).unwrap()[..4].to_vec();
        assert_eq!(&head, b"GGUF");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Cancel is "drop the stream", which leaves the half-file behind for the
    /// caller to sweep — so the only way this breaks is the two sides naming
    /// that file differently. Proves they do not.
    #[tokio::test]
    #[ignore = "hits huggingface.co"]
    async fn a_dropped_download_leaves_exactly_the_part_file() {
        let (url, name) =
            resolve("ggml-org/models/tinyllamas/stories15M-q4_0.gguf").expect("resolves");
        let dir = std::env::temp_dir().join("agp-model-download-cancel-test");
        let _ = std::fs::remove_dir_all(&dir);

        {
            let mut stream = std::pin::pin!(download(url, dir.clone(), name.clone()));
            // One tick is 4 MB in, which is mid-transfer for a 19 MB file.
            let first = stream.next().await;
            assert!(
                matches!(first, Some(Progress::Downloading { .. })),
                "expected a tick, got {first:?}"
            );
        } // dropped here — this is the whole of what Cancel does.

        let part = part_path(&dir, &name);
        assert!(part.is_file(), "no {} to sweep", part.display());
        assert!(std::fs::metadata(&part).unwrap().len() > 0);
        assert!(!dir.join(&name).exists(), "an aborted transfer was renamed whole");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sizes_read_as_sizes() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(4 << 30), "4.0 GB");
    }
}
