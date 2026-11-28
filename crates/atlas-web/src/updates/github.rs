//! Reads the repositories skills were installed from, anonymously, through GitHub's REST
//! API, the way the owner's T3 Code does: a repository's whole tree once per check, a
//! folder's tree by its SHA, and each file by its blob SHA. Everything named by a SHA is
//! checked against it, so bytes GitHub (or anything between) changed are never used.
//!
//! Every request has a 10 second limit and a size cap. GitHub allows 60 requests an hour
//! without signing in; when it says the limit is used up, nothing more is asked until the
//! time it gives. Redirects (renamed repositories) are followed only on the same host.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use atlas_fs::lock::{TreeItem, git_blob_sha, git_tree_sha};
use base64::Engine as _;
use serde::Deserialize;

/// The real API. Tests point the hub at a stand-in on 127.0.0.1 instead.
pub(crate) const API: &str = "https://api.github.com";
const TIMEOUT: Duration = Duration::from_secs(10);
/// The largest tree listing read (GitHub caps a recursive listing near 7 MB).
const MAX_TREE_BYTES: usize = 10 * 1024 * 1024;
/// The largest file fetched, before base64 (as JSON it is a third bigger).
pub(crate) const MAX_BLOB_BYTES: usize = 5 * 1024 * 1024;
/// Fetched files kept in memory, by SHA, for previews and the update itself.
const MAX_CACHED_BYTES: usize = 32 * 1024 * 1024;
/// Used when GitHub says the limit is used up but not until when.
const DEFAULT_PAUSE: Duration = Duration::from_mins(1);

/// Why GitHub couldn't answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Problem {
    /// The unsigned-in limit is used up until this time.
    RateLimited(SystemTime),
    /// No such repository, branch or object, or a private repository.
    NotFound,
    /// Too big to read within Atlas's limits.
    TooBig,
    /// GitHub couldn't be reached or answered with something unexpected.
    Unavailable,
}

impl Problem {
    /// A sentence for the owner.
    pub(crate) fn sentence(&self) -> String {
        match self {
            Self::RateLimited(at) => format!(
                "GitHub's limit for checks without signing in is used up. Try again after {} UTC.",
                clock(*at)
            ),
            Self::NotFound => {
                "GitHub doesn't have that repository or branch (or it is private).".to_owned()
            }
            Self::TooBig => "The source is too big to check.".to_owned(),
            Self::Unavailable => "GitHub couldn't be reached. Try again later.".to_owned(),
        }
    }
}

/// `HH:MM` of a time, in UTC.
pub(crate) fn clock(at: SystemTime) -> String {
    let secs = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    format!("{:02}:{:02}", (secs / 3600) % 24, (secs / 60) % 60)
}

/// One entry of a tree listing.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Entry {
    pub path: String,
    pub mode: String,
    /// `blob` for a file or link, `tree` for a folder, `commit` for a submodule.
    #[serde(rename = "type")]
    pub kind: String,
    pub sha: String,
    #[serde(default)]
    pub size: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct RepoTree {
    pub sha: String,
    #[serde(default)]
    pub truncated: bool,
    pub tree: Vec<Entry>,
}

#[derive(Deserialize)]
struct Blob {
    content: String,
    encoding: String,
}

/// The anonymous GitHub reader, with its rate-limit pause and file cache.
#[derive(Debug)]
pub(crate) struct GitHub {
    client: Option<reqwest::Client>,
    api: String,
    paused_until: Mutex<Option<SystemTime>>,
    blobs: Mutex<BlobCache>,
}

#[derive(Debug, Default)]
struct BlobCache {
    bytes: usize,
    order: Vec<String>,
    by_sha: HashMap<String, Arc<[u8]>>,
}

pub(crate) fn is_sha(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Percent-encodes everything but unreserved characters, so a ref is one path segment.
fn segment(s: &str) -> String {
    s.bytes().fold(String::new(), |mut out, b| {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            use std::fmt::Write as _;
            let _ = write!(out, "%{b:02X}");
        }
        out
    })
}

impl GitHub {
    pub(crate) fn new(api: &str) -> Self {
        let host = reqwest::Url::parse(api)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned));
        let policy = reqwest::redirect::Policy::custom(move |attempt| {
            let same = attempt.url().host_str().map(str::to_owned) == host;
            if same && attempt.previous().len() < 3 {
                attempt.follow()
            } else {
                attempt.stop()
            }
        });
        let client = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .connect_timeout(TIMEOUT)
            .redirect(policy)
            .user_agent("atlas-hub-skill-updates")
            .build()
            .map_err(|e| tracing::warn!("no HTTP client for skill updates: {e}"))
            .ok();
        Self {
            client,
            api: api.trim_end_matches('/').to_owned(),
            paused_until: Mutex::new(None),
            blobs: Mutex::new(BlobCache::default()),
        }
    }

    /// A GET of the API; `Ok(None)` when GitHub says there is no such thing.
    async fn get(&self, endpoint: &str, cap: usize) -> Result<Option<Vec<u8>>, Problem> {
        let now = SystemTime::now();
        if let Some(until) = *self.paused_until.lock().map_err(|_| Problem::Unavailable)?
            && now < until
        {
            return Err(Problem::RateLimited(until));
        }
        let client = self.client.as_ref().ok_or(Problem::Unavailable)?;
        let mut res = client
            .get(format!("{}/{endpoint}", self.api))
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28")
            .send()
            .await
            .map_err(|e| {
                tracing::info!("GitHub request failed: {e}");
                Problem::Unavailable
            })?;
        if let Some(until) = rate_limited_until(&res, now) {
            if let Ok(mut p) = self.paused_until.lock() {
                *p = Some(until);
            }
            return Err(Problem::RateLimited(until));
        }
        let status = res.status().as_u16();
        // A 422 is GitHub refusing a ref or SHA it doesn't know.
        if status == 404 || status == 422 {
            return Ok(None);
        }
        if status != 200 {
            return Err(Problem::Unavailable);
        }
        if res.content_length().is_some_and(|n| n > cap as u64) {
            return Err(Problem::TooBig);
        }
        let mut body = Vec::new();
        while let Some(chunk) = res.chunk().await.map_err(|_| Problem::Unavailable)? {
            if body.len() + chunk.len() > cap {
                return Err(Problem::TooBig);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(Some(body))
    }

    async fn tree(&self, owner: &str, repo: &str, treeish: &str) -> Result<Option<RepoTree>, Problem> {
        let endpoint = format!(
            "repos/{owner}/{repo}/git/trees/{}?recursive=1",
            segment(treeish)
        );
        let Some(body) = self.get(&endpoint, MAX_TREE_BYTES).await? else {
            return Ok(None);
        };
        serde_json::from_slice(&body)
            .map(Some)
            .map_err(|_| Problem::Unavailable)
    }

    /// The repository's whole tree at `git_ref`, or its default branch. One request.
    pub(crate) async fn repo_tree(
        &self,
        owner: &str,
        repo: &str,
        git_ref: Option<&str>,
    ) -> Result<RepoTree, Problem> {
        self.tree(owner, repo, git_ref.unwrap_or("HEAD"))
            .await?
            .ok_or(Problem::NotFound)
    }

    /// Every file under the tree with this SHA, checked against it; `Ok(None)` when GitHub
    /// no longer has it.
    pub(crate) async fn tree_by_sha(
        &self,
        owner: &str,
        repo: &str,
        sha: &str,
    ) -> Result<Option<RepoTree>, Problem> {
        if !is_sha(sha) {
            return Ok(None);
        }
        let Some(tree) = self.tree(owner, repo, sha).await? else {
            return Ok(None);
        };
        let items: Vec<TreeItem> = tree
            .tree
            .iter()
            .filter(|e| e.kind != "tree")
            .map(|e| TreeItem {
                path: e.path.clone(),
                mode: e.mode.clone(),
                sha: e.sha.clone(),
            })
            .collect();
        if tree.truncated || tree.sha != sha || git_tree_sha(&items) != sha {
            return Err(Problem::Unavailable);
        }
        Ok(Some(tree))
    }

    /// A file's bytes by its blob SHA, checked against it.
    pub(crate) async fn blob(&self, owner: &str, repo: &str, sha: &str) -> Result<Arc<[u8]>, Problem> {
        if !is_sha(sha) {
            return Err(Problem::NotFound);
        }
        if let Some(b) = self.cached(sha) {
            return Ok(b);
        }
        let endpoint = format!("repos/{owner}/{repo}/git/blobs/{sha}");
        let body = self
            .get(&endpoint, MAX_BLOB_BYTES / 3 * 4 + 64 * 1024)
            .await?
            .ok_or(Problem::NotFound)?;
        let blob: Blob = serde_json::from_slice(&body).map_err(|_| Problem::Unavailable)?;
        let bytes = if blob.encoding == "base64" {
            let clean: String = blob.content.chars().filter(|c| !c.is_whitespace()).collect();
            base64::engine::general_purpose::STANDARD
                .decode(clean)
                .map_err(|_| Problem::Unavailable)?
        } else {
            blob.content.into_bytes()
        };
        // Content addressing: bytes that don't hash to the SHA asked for are never used.
        if git_blob_sha(&bytes) != sha {
            return Err(Problem::Unavailable);
        }
        let bytes: Arc<[u8]> = bytes.into();
        self.remember(sha, &bytes);
        Ok(bytes)
    }

    /// Keeps bytes known to have this SHA (the owner's own copy of a file, say), so a
    /// later step never fetches them.
    pub(crate) fn remember(&self, sha: &str, bytes: &Arc<[u8]>) {
        let Ok(mut c) = self.blobs.lock() else { return };
        if c.by_sha.contains_key(sha) || bytes.len() > MAX_CACHED_BYTES / 4 {
            return;
        }
        c.bytes += bytes.len();
        c.order.push(sha.to_owned());
        c.by_sha.insert(sha.to_owned(), Arc::clone(bytes));
        while c.bytes > MAX_CACHED_BYTES && !c.order.is_empty() {
            let old = c.order.remove(0);
            if let Some(b) = c.by_sha.remove(&old) {
                c.bytes -= b.len();
            }
        }
    }

    pub(crate) fn cached(&self, sha: &str) -> Option<Arc<[u8]>> {
        self.blobs.lock().ok()?.by_sha.get(sha).cloned()
    }
}

/// When GitHub says the limit is used up: from `retry-after`, else
/// `x-ratelimit-reset` with `x-ratelimit-remaining: 0`. A 429 is always a limit; a 403
/// without those headers is a refusal of another kind.
fn rate_limited_until(res: &reqwest::Response, now: SystemTime) -> Option<SystemTime> {
    let status = res.status().as_u16();
    if status != 403 && status != 429 {
        return None;
    }
    let header = |name: &str| {
        res.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
    };
    if let Some(secs) = header("retry-after").filter(|s| *s > 0) {
        return Some(now + Duration::from_secs(secs.min(24 * 3600)));
    }
    if header("x-ratelimit-remaining") != Some(0) {
        return (status == 429).then(|| now + DEFAULT_PAUSE);
    }
    let reset = header("x-ratelimit-reset")
        .map(|s| SystemTime::UNIX_EPOCH + Duration::from_secs(s))
        .filter(|t| *t > now && *t < now + Duration::from_secs(24 * 3600));
    Some(reset.unwrap_or(now + DEFAULT_PAUSE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_are_one_segment() {
        assert_eq!(segment("feature/x+y@1"), "feature%2Fx%2By%401");
        assert_eq!(segment("v1.2_3-a"), "v1.2_3-a");
    }

    #[test]
    fn clock_is_utc() {
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_795_737_600 + 14 * 3600 + 5 * 60);
        assert_eq!(clock(t), "14:05");
    }
}
