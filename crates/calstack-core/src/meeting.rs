use url::Url;

pub fn action_label(candidate: &str) -> Option<&'static str> {
    let url = recognized_url(candidate)?;
    let host = url.host_str()?;
    Some(match host {
        "meet.google.com" => "Open Google Meet",
        "teams.microsoft.com" | "teams.live.com" => "Open Microsoft Teams",
        "whereby.com" => "Open Whereby",
        "meet.jit.si" => "Open Jitsi Meet",
        "chime.aws" => "Open Amazon Chime",
        host if host == "zoom.us" || host.ends_with(".zoom.us") => "Open Zoom",
        host if host == "webex.com" || host.ends_with(".webex.com") => "Open Webex",
        _ => "Open GoTo Meeting",
    })
}

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
    fn actions_identify_validated_providers() {
        assert_eq!(
            action_label("https://meet.google.com/abc-defg-hij"),
            Some("Open Google Meet")
        );
        assert_eq!(
            action_label("https://us02web.zoom.us/j/84261573920"),
            Some("Open Zoom")
        );
        assert_eq!(action_label("https://zoom.us.evil.test/j/123"), None);
    }
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
