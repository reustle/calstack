use url::Url;

pub fn recognized_url(candidate: &str) -> Option<Url> {
    let url = Url::parse(candidate).ok()?;
    if !["https", "http"].contains(&url.scheme())
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let host = url.host_str()?;
    let exact = [
        "meet.google.com",
        "teams.microsoft.com",
        "teams.live.com",
        "whereby.com",
        "meet.jit.si",
        "chime.aws",
    ];
    let subdomain = ["zoom.us", "webex.com", "gotomeeting.com"];
    (exact.contains(&host)
        || subdomain
            .iter()
            .any(|domain| host == *domain || host.ends_with(&format!(".{domain}"))))
    .then_some(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_recognized_hosts() {
        for url in [
            "https://meet.google.com/abc",
            "https://us02web.zoom.us/j/123",
            "https://teams.microsoft.com/l/meetup-join/test",
        ] {
            assert!(recognized_url(url).is_some());
        }
        for url in [
            "https://zoom.us.evil.test/",
            "https://evilzoom.us/",
            "javascript:alert(1)",
            "https://zoom.us@evil.test/",
            "https://user@zoom.us/",
        ] {
            assert!(recognized_url(url).is_none());
        }
    }
}
