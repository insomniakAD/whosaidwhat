//! Google Meet URL classification (platform-independent, dependency-free).
//!
//! A browser tab counts as a meeting only for `meet.google.com/<code>` URLs
//! (codes look like `abc-defg-hij`) or `lookup/<alias>` paths. The bare
//! landing page (`meet.google.com`, title "Meet - Google Meet") must NOT
//! match — that false positive is documented in shipped detectors (see
//! docs/02-process-detection.md).

/// True for meet.google.com meeting URLs, false for the landing page and
/// every other URL.
pub fn is_meet_meeting_url(url: &str) -> bool {
    let url = url.trim();
    let Some(rest) = url
        .strip_prefix("https://meet.google.com/")
        .or_else(|| url.strip_prefix("http://meet.google.com/"))
    else {
        return false;
    };
    let path = rest.split(&['?', '#'][..]).next().unwrap_or("");
    let is_code = {
        let parts: Vec<&str> = path.split('-').collect();
        parts.len() == 3
            && parts[0].len() == 3
            && parts[1].len() == 4
            && parts[2].len() == 3
            && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphabetic()))
    };
    is_code || path.starts_with("lookup/")
}

#[cfg(test)]
mod tests {
    use super::is_meet_meeting_url;

    #[test]
    fn meeting_code_urls_match() {
        assert!(is_meet_meeting_url("https://meet.google.com/abc-defg-hij"));
        assert!(is_meet_meeting_url("https://meet.google.com/abc-defg-hij?authuser=0"));
        assert!(is_meet_meeting_url("http://meet.google.com/xyz-qrst-uvw#frag"));
        assert!(is_meet_meeting_url("  https://meet.google.com/abc-defg-hij\n"));
    }

    #[test]
    fn lookup_urls_match() {
        assert!(is_meet_meeting_url("https://meet.google.com/lookup/team-standup"));
    }

    #[test]
    fn landing_page_and_lobby_do_not_match() {
        assert!(!is_meet_meeting_url("https://meet.google.com/"));
        assert!(!is_meet_meeting_url("https://meet.google.com"));
        assert!(!is_meet_meeting_url("https://meet.google.com/landing"));
        assert!(!is_meet_meeting_url("https://meet.google.com/new"));
    }

    #[test]
    fn other_urls_do_not_match() {
        assert!(!is_meet_meeting_url("https://calendar.google.com/abc-defg-hij"));
        assert!(!is_meet_meeting_url("https://example.com/https://meet.google.com/abc-defg-hij"));
        assert!(!is_meet_meeting_url(""));
        assert!(!is_meet_meeting_url("https://meet.google.com/ab1-defg-hij")); // digit in code
        assert!(!is_meet_meeting_url("https://meet.google.com/abc-defg-hij-x"));
    }
}
