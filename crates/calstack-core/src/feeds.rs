//! Bounded feed downloads and private, last-good disk caching.
use crate::{calendar::events_for_day, config::Feed, Event};
use anyhow::{bail, Context, Result};
use chrono::NaiveDate;
use reqwest::{blocking::Client, header, StatusCode};
use rrule::Tz;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

const MAX_BYTES: u64 = 10 * 1024 * 1024;
#[derive(Default)]
pub struct FeedUpdate {
    /// Only successful feeds are included. Callers retain failed feeds' prior events.
    pub calendars: Vec<(String, Vec<Event>)>,
    pub warnings: Vec<String>,
}
#[derive(Serialize, Deserialize)]
struct Cached {
    body: String,
    etag: Option<String>,
    modified: Option<String>,
}
#[cfg(target_os = "macos")]
pub fn cache_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join("Library/Caches/calstack/calendars")
}
#[cfg(not(target_os = "macos"))]
pub fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache")
        })
        .join("calstack/calendars")
}
fn cache_path(root: &Path, feed: &Feed) -> PathBuf {
    let source = feed
        .url
        .clone()
        .unwrap_or_else(|| feed.path.as_ref().unwrap().to_string_lossy().into_owned());
    root.join(format!("{:x}.json", Sha256::digest(source.as_bytes())))
}
fn read_bounded(path: &Path, limit: u64) -> Result<String> {
    let file = fs::File::open(path)?;
    let mut data = String::new();
    file.take(limit + 1).read_to_string(&mut data)?;
    if data.len() as u64 > limit {
        bail!("calendar exceeds size limit");
    }
    Ok(data)
}
fn read_cache(path: &Path) -> Option<Cached> {
    serde_json::from_str(&read_bounded(path, MAX_BYTES * 2).ok()?).ok()
}
fn write_cache(path: &Path, cached: &Cached) -> Result<()> {
    let parent = path.parent().context("cache directory missing")?;
    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(&serde_json::to_vec(cached)?)?;
    file.sync_all()?;
    fs::rename(temporary, path)?;
    Ok(())
}
fn client() -> Result<Client> {
    Ok(Client::builder()
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(8))
        .user_agent(concat!("calstack/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 {
                attempt.error("too many redirects")
            } else if attempt
                .previous()
                .last()
                .is_some_and(|u| u.scheme() == "https")
                && attempt.url().scheme() != "https"
            {
                attempt.error("insecure redirect")
            } else {
                attempt.follow()
            }
        }))
        .build()?)
}
fn fetch(client: &Client, feed: &Feed, cached: Option<&Cached>) -> Result<Cached> {
    let mut request = client
        .get(feed.remote_url()?)
        .header(header::ACCEPT, "text/calendar, */*;q=0.1");
    if let Some(cache) = cached {
        if let Some(etag) = &cache.etag {
            request = request.header(header::IF_NONE_MATCH, etag);
        }
        if let Some(modified) = &cache.modified {
            request = request.header(header::IF_MODIFIED_SINCE, modified);
        }
    }
    let response = request.send()?;
    if response.status() == StatusCode::NOT_MODIFIED {
        let cache = cached.context("server returned 304 without a cached calendar")?;
        return Ok(Cached {
            body: cache.body.clone(),
            etag: cache.etag.clone(),
            modified: cache.modified.clone(),
        });
    }
    if response.status() != StatusCode::OK {
        bail!("unexpected HTTP status");
    }
    if response.content_length().is_some_and(|len| len > MAX_BYTES) {
        bail!("calendar exceeds size limit");
    }
    let etag = response
        .headers()
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let modified = response
        .headers()
        .get(header::LAST_MODIFIED)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let mut body = String::new();
    response.take(MAX_BYTES + 1).read_to_string(&mut body)?;
    if body.len() as u64 > MAX_BYTES {
        bail!("calendar exceeds size limit");
    }
    Ok(Cached {
        body,
        etag,
        modified,
    })
}

/// Cache-only startup and online refresh share normalization. Errors intentionally
/// omit transport/parser details, which may contain private feed tokens or content.
pub fn load(feeds: &[Feed], root: &Path, day: NaiveDate, online: bool) -> FeedUpdate {
    load_in_zone(feeds, root, day, online, Tz::LOCAL)
}
fn load_in_zone(feeds: &[Feed], root: &Path, day: NaiveDate, online: bool, zone: Tz) -> FeedUpdate {
    let client = online.then(client);
    let mut update = FeedUpdate::default();
    for feed in feeds.iter().filter(|feed| feed.enabled) {
        let path = cache_path(root, feed);
        let cached = read_cache(&path);
        let candidate = if let Some(local) = &feed.path {
            Some(read_bounded(local, MAX_BYTES).map(|body| Cached {
                body,
                etag: None,
                modified: None,
            }))
        } else if online {
            Some(
                client
                    .as_ref()
                    .unwrap()
                    .as_ref()
                    .map_err(|_| anyhow::anyhow!("HTTP client unavailable"))
                    .and_then(|client| fetch(client, feed, cached.as_ref())),
            )
        } else {
            None
        };
        let mut applied = false;
        if let Some(candidate) = candidate {
            match candidate {
                Ok(data) => match events_for_day(&data.body, feed, day, zone) {
                    Ok(events) => {
                        if write_cache(&path, &data).is_err() {
                            update.warnings.push(format!(
                                "{}: calendar loaded, but cache could not be saved",
                                feed.name
                            ));
                        }
                        update.calendars.push((feed.name.clone(), events));
                        applied = true;
                    }
                    Err(_) => update.warnings.push(format!(
                        "{}: invalid or unsupported ICS; keeping last good data",
                        feed.name
                    )),
                },
                Err(_) => update.warnings.push(format!(
                    "{}: calendar could not be fetched/read; keeping last good data",
                    feed.name
                )),
            }
        }
        if !applied {
            if let Some(cache) = cached {
                if let Ok(events) = events_for_day(&cache.body, feed, day, zone) {
                    update.calendars.push((feed.name.clone(), events));
                }
            }
        }
    }
    update
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    #[test]
    fn failed_and_disabled_feeds_do_not_block_other_calendars() {
        let root = std::env::temp_dir().join(format!(
            "calstack-isolation-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("good.ics");
        fs::write(&path, "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:a\nDTSTART:20260926T090000Z\nDURATION:PT1H\nEND:VEVENT\nEND:VCALENDAR\n").unwrap();
        let good = Feed {
            name: "Good".into(),
            url: None,
            path: Some(path),
            color: None,
            enabled: true,
        };
        let mut bad = good.clone();
        bad.name = "Missing".into();
        bad.path = Some(root.join("missing.ics"));
        let mut disabled = bad.clone();
        disabled.name = "Disabled".into();
        disabled.enabled = false;
        let result = load_in_zone(
            &[bad, disabled, good],
            &root.join("cache"),
            "2026-09-26".parse().unwrap(),
            true,
            Tz::UTC,
        );
        assert_eq!(result.calendars.len(), 1);
        assert_eq!(result.calendars[0].0, "Good");
        assert_eq!(result.calendars[0].1.len(), 1);
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].starts_with("Missing:"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn http_cache_revalidation_offline_and_bad_response() {
        let root = std::env::temp_dir().join(format!(
            "calstack-feed-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "http://{}/private?token=secret",
            listener.local_addr().unwrap()
        );
        let server = std::thread::spawn(move || {
            let body = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:a\r\nDTSTART:20260926T090000Z\r\nDTEND:20260926T100000Z\r\nSUMMARY:Remote meeting\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
            for step in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    stream.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                }
                let request = String::from_utf8(request).unwrap().to_lowercase();
                let response = match step {
                    0 => format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: \"v1\"\r\nLast-Modified: Fri, 25 Sep 2026 00:00:00 GMT\r\nConnection: close\r\n\r\n{body}", body.len()),
                    1 => {
                        assert!(request.contains("if-none-match: \"v1\""));
                        assert!(request.contains("if-modified-since:"));
                        "HTTP/1.1 304 Not Modified\r\nConnection: close\r\n\r\n".into()
                    }
                    _ => "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nwrong".into(),
                };
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        let feed = Feed {
            name: "Work".into(),
            url: Some(url),
            path: None,
            color: None,
            enabled: true,
        };
        let feeds = [feed];
        let date = "2026-09-26".parse().unwrap();
        for step in 0..3 {
            let result = load_in_zone(&feeds, &root, date, true, Tz::UTC);
            assert_eq!(result.calendars[0].1[0].title, "Remote meeting");
            assert_eq!(result.calendars[0].1[0].start, 540);
            assert_eq!(result.warnings.is_empty(), step < 2);
            assert!(!result.warnings.join(" ").contains("secret"));
        }
        server.join().unwrap();
        let offline = load_in_zone(&feeds, &root, date, true, Tz::UTC);
        assert_eq!(offline.calendars[0].1.len(), 1);
        assert!(!offline.warnings.is_empty());
        assert_eq!(
            load_in_zone(&feeds, &root, date, false, Tz::UTC).calendars[0]
                .1
                .len(),
            1
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(cache_path(&root, &feeds[0]))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(root).unwrap();
    }
}
