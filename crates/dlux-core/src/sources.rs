//! Turning what the user typed, pasted or dropped into downloads: URLs, magnet links and
//! `.torrent` / `.metalink` files. Shared by the TUI's Add dialog and the web UI.

use crate::paths;
use crate::rpc::{self, Client, Gid, Options};
use std::fmt;
use std::path::{Path, PathBuf};
use url::Url;

/// URL schemes aria2 downloads directly.
const SCHEMES: &[&str] = &["http", "https", "ftp", "ftps", "sftp"];

/// One thing to download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// An HTTP(S)/FTP/SFTP URL (a remote `.torrent` or `.metalink` URL also works: aria2
    /// follows it).
    Url(String),
    Magnet(String),
    Torrent(PathBuf),
    Metalink(PathBuf),
}

impl Source {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Url(_) => "URL",
            Self::Magnet(_) => "magnet",
            Self::Torrent(_) => "torrent",
            Self::Metalink(_) => "metalink",
        }
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Url(s) | Self::Magnet(s) => f.write_str(s),
            Self::Torrent(p) | Self::Metalink(p) => write!(f, "{}", p.display()),
        }
    }
}

/// A piece of input that is not a download, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid {
    pub input: String,
    pub reason: &'static str,
}

/// The result of [`parse`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Parsed {
    pub sources: Vec<Source>,
    pub invalid: Vec<Invalid>,
}

impl Parsed {
    /// Short summary such as "2 URLs, 1 torrent".
    pub fn summary(&self) -> String {
        let mut counts: Vec<(&str, usize)> = Vec::new();
        for s in &self.sources {
            match counts.iter_mut().find(|(k, _)| *k == s.kind()) {
                Some((_, n)) => *n += 1,
                None => counts.push((s.kind(), 1)),
            }
        }
        counts
            .iter()
            .map(|(k, n)| {
                if *n == 1 {
                    format!("1 {k}")
                } else {
                    format!("{n} {k}s")
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Split free-form input into sources. Each line is one item, unless it is not a file path and
/// contains spaces, in which case every whitespace-separated word is an item. Quotes and
/// `file://` prefixes (as added by drag-and-drop) are stripped, and `~` is expanded.
pub fn parse(input: &str) -> Parsed {
    let mut parsed = Parsed::default();
    for line in input.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(path) = local_file(line) {
            push_file(&mut parsed, line, path);
            continue;
        }
        for word in line.split_whitespace() {
            classify(&mut parsed, word);
        }
    }
    parsed
}

fn unquote(s: &str) -> &str {
    let s = s.trim();
    for q in ['"', '\''] {
        if s.len() >= 2 && s.starts_with(q) && s.ends_with(q) {
            return &s[1..s.len() - 1];
        }
    }
    s
}

/// `s` as an existing local file, if it is one (plain path, `~/…`, or `file://` URI).
fn local_file(s: &str) -> Option<PathBuf> {
    let s = unquote(s);
    let path = if s.starts_with("file://") {
        Url::parse(s).ok()?.to_file_path().ok()?
    } else {
        paths::expand_tilde(s)
    };
    path.is_file().then_some(path)
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn push_file(parsed: &mut Parsed, input: &str, path: PathBuf) {
    match extension(&path).as_str() {
        "torrent" => parsed.sources.push(Source::Torrent(path)),
        "metalink" | "meta4" => parsed.sources.push(Source::Metalink(path)),
        _ => parsed.invalid.push(Invalid {
            input: input.to_owned(),
            reason: "only .torrent and .metalink files can be added",
        }),
    }
}

fn classify(parsed: &mut Parsed, word: &str) {
    let word = unquote(word);
    if word.is_empty() {
        return;
    }
    if let Some(path) = local_file(word) {
        return push_file(parsed, word, path);
    }
    let invalid = |reason| Invalid {
        input: word.to_owned(),
        reason,
    };
    if word.len() > 7 && word[..7].eq_ignore_ascii_case("magnet:") {
        if word.contains("xt=urn:") {
            parsed.sources.push(Source::Magnet(word.to_owned()));
        } else {
            parsed
                .invalid
                .push(invalid("magnet link without an xt=urn: hash"));
        }
        return;
    }
    match Url::parse(word) {
        Ok(url) if SCHEMES.contains(&url.scheme()) && url.host_str().is_some() => {
            parsed.sources.push(Source::Url(word.to_owned()));
        }
        Ok(url) if url.scheme() == "file" => parsed.invalid.push(invalid("file not found")),
        // "localhost:8080/x" parses with scheme "localhost": it is a host and port.
        Ok(url)
            if url.path().starts_with(|c: char| c.is_ascii_digit()) && !word.contains("://") =>
        {
            parsed
                .invalid
                .push(invalid("not a URL (add http:// or https://)"));
        }
        Ok(_) => parsed.invalid.push(invalid("unsupported URL scheme")),
        Err(_) => {
            let ext = extension(Path::new(word));
            let reason = if matches!(ext.as_str(), "torrent" | "metalink" | "meta4") {
                "file not found"
            } else if word.contains('.') && !word.contains(char::is_whitespace) {
                "not a URL (add http:// or https://)"
            } else {
                "not a URL, magnet link or file"
            };
            parsed.invalid.push(invalid(reason));
        }
    }
}

/// Options for [`add`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AddOptions {
    /// Download directory; aria2's default when `None`.
    pub dir: Option<String>,
    /// Add the downloads paused.
    pub paused: bool,
}

impl AddOptions {
    fn to_aria2(&self) -> Options {
        let mut o = Options::new();
        if let Some(dir) = self.dir.as_ref().filter(|d| !d.trim().is_empty()) {
            o.insert(
                "dir".into(),
                paths::expand_tilde(dir.trim()).display().to_string(),
            );
        }
        if self.paused {
            o.insert("pause".into(), "true".into());
        }
        o
    }
}

/// Why adding one source failed.
#[derive(Debug, thiserror::Error)]
pub enum AddError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error(transparent)]
    Rpc(#[from] rpc::Error),
}

/// Add every source; one result per source, in order.
pub async fn add(
    client: &Client,
    sources: &[Source],
    options: &AddOptions,
) -> Vec<Result<Vec<Gid>, AddError>> {
    let opts = options.to_aria2();
    let mut results = Vec::with_capacity(sources.len());
    for source in sources {
        results.push(add_one(client, source, &opts).await);
    }
    results
}

async fn add_one(client: &Client, source: &Source, opts: &Options) -> Result<Vec<Gid>, AddError> {
    let read = async |path: &Path| {
        tokio::fs::read(path)
            .await
            .map_err(|source| AddError::Read {
                path: path.display().to_string(),
                source,
            })
    };
    Ok(match source {
        Source::Url(u) | Source::Magnet(u) => {
            vec![client.add_uri(std::slice::from_ref(u), opts).await?]
        }
        Source::Torrent(p) => vec![client.add_torrent(&read(p).await?, opts).await?],
        Source::Metalink(p) => client.add_metalink(&read(p).await?, opts).await?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_and_magnets() {
        let p = parse(
            "https://example.com/a.iso\n\
             \n  ftp://mirror.example.org/b.tar.gz   http://x.org/c  \n\
             magnet:?xt=urn:btih:0123456789abcdef&dn=thing\n\
             # a comment\n",
        );
        assert_eq!(
            p.sources,
            vec![
                Source::Url("https://example.com/a.iso".into()),
                Source::Url("ftp://mirror.example.org/b.tar.gz".into()),
                Source::Url("http://x.org/c".into()),
                Source::Magnet("magnet:?xt=urn:btih:0123456789abcdef&dn=thing".into()),
            ]
        );
        assert!(p.invalid.is_empty());
        assert_eq!(p.summary(), "3 URLs, 1 magnet");
    }

    #[test]
    fn invalid_input_is_explained() {
        let p = parse(
            "example.com/file.zip\nmailto:a@b.c\nmagnet:?dn=x\nhello\n/no/such.torrent\nlocalhost:8765/x",
        );
        let reasons: Vec<_> = p.invalid.iter().map(|i| i.reason).collect();
        assert_eq!(
            reasons,
            vec![
                "not a URL (add http:// or https://)",
                "unsupported URL scheme",
                "magnet link without an xt=urn: hash",
                "not a URL, magnet link or file",
                "file not found",
                "not a URL (add http:// or https://)",
            ]
        );
        assert!(p.sources.is_empty());
        assert_eq!(p.summary(), "");
    }

    #[test]
    fn local_files_quotes_and_file_uris() {
        let dir = tempfile::tempdir().unwrap();
        let torrent = dir.path().join("my file.torrent");
        let meta = dir.path().join("x.META4");
        let other = dir.path().join("notes.txt");
        for f in [&torrent, &meta, &other] {
            std::fs::write(f, b"d").unwrap();
        }
        let file_uri = Url::from_file_path(&meta).unwrap().to_string();
        let input = format!(
            "{}\n'{}'\n{file_uri}\n{}",
            torrent.display(),
            torrent.display(),
            other.display()
        );
        let p = parse(&input);
        assert_eq!(
            p.sources,
            vec![
                Source::Torrent(torrent.clone()),
                Source::Torrent(torrent),
                Source::Metalink(meta),
            ]
        );
        assert_eq!(p.invalid.len(), 1);
        assert_eq!(
            p.invalid[0].reason,
            "only .torrent and .metalink files can be added"
        );
    }

    #[test]
    fn add_options() {
        let o = AddOptions {
            dir: Some("  /data/dl ".into()),
            paused: true,
        }
        .to_aria2();
        assert_eq!(o.get("dir").map(String::as_str), Some("/data/dl"));
        assert_eq!(o.get("pause").map(String::as_str), Some("true"));
        assert!(
            AddOptions {
                dir: Some(" ".into()),
                paused: false
            }
            .to_aria2()
            .is_empty()
        );
    }
}
