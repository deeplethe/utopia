//! Canonical HTML conversion shared by file, URL and feed ingestion.
#[cfg(test)]
mod evidence_tests {
    use super::*;

    const REPORT: &str = "Researchers examined captcha verification across public services and found that older residents struggled with distorted images. Their study followed participants through several independent tasks, recording completion times and interviewing people about accessibility barriers. The published results describe how alternative authentication methods improved access without increasing fraudulent registrations. Local authorities are now reviewing procurement standards, while independent security specialists recommend measuring actual abuse rather than assuming every additional challenge provides protection.";

    #[test]
    fn substantive_marker_prose_survives_every_html_entry_point() {
        let raw = format!("<html><body><article><p>{REPORT}</p></article></body></html>");
        for markdown in [
            page_to_markdown(&raw, Some("https://example.com/story")),
            fragment_to_markdown(&raw),
            crate::parse("story.html", raw.as_bytes())
                .map(|p| p.text)
                .map_err(|e| HtmlError::Conversion(e.to_string())),
        ] {
            assert!(markdown.unwrap().contains("captcha verification"));
        }
    }

    #[test]
    fn substantive_raw_fallback_survives_newsletter_marker() {
        let raw = format!("<article><p>{REPORT}</p></article><form><p>Subscribe to read our newsletter</p><input type='email'></form>");
        assert!(dom_smoothie::Readability::new(raw.as_str(), Some("not a URL"), None).is_err());
        assert!(page_to_markdown(&raw, Some("not a URL"))
            .unwrap()
            .contains(REPORT));
        assert!(fragment_to_markdown(&raw).unwrap().contains(REPORT));
    }

    #[test]
    fn normal_paragraph_breaks_preserve_article_evidence() {
        let split = REPORT.replace(". ", ".</p><p>");
        for prose in [REPORT.to_string(), split] {
            let raw = format!("<article><p>{prose}</p></article><form><p>Subscribe to read our newsletter</p><input type='email'></form>");
            for result in [
                page_to_markdown(&raw, Some("https://example.com/story")),
                page_to_markdown(&raw, Some("not a URL")),
                fragment_to_markdown(&raw),
                crate::parse("story.html", raw.as_bytes())
                    .map(|p| p.text)
                    .map_err(|e| HtmlError::Conversion(e.to_string())),
            ] {
                assert!(result.unwrap().contains("captcha verification"));
            }
        }
    }

    #[test]
    fn short_and_padded_marker_shells_remain_rejected() {
        for marker in [
            "captcha verification",
            "Subscribe to read",
            "Sign in to continue",
        ] {
            for padding in [
                String::new(),
                "Account access requires verification of your identity before proceeding. "
                    .repeat(100),
            ] {
                let raw = format!("<h1>{marker}</h1><p>{padding}</p>");
                assert_eq!(fragment_to_markdown(&raw), Err(HtmlError::Interstitial));
                assert_eq!(
                    page_to_markdown(&raw, Some("https://example.com")),
                    Err(HtmlError::Interstitial)
                );
            }
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum HtmlError {
    #[error("HTML conversion failed: {0}")]
    Conversion(String),
    #[error("HTML is an authentication or player interstitial")]
    Interstitial,
}

/// Extract a full page; Readability failure uses one raw conversion fallback.
pub fn page_to_markdown(html: &str, base_url: Option<&str>) -> Result<String, HtmlError> {
    // Judge the extracted body, not newsletter/login controls in page chrome.
    // The raw fallback still goes through the same fail-closed converter.
    let article = dom_smoothie::Readability::new(html, base_url, None)
        .and_then(|mut readability| readability.parse());
    match article {
        Ok(article) => {
            // Readability can discard every control on a login-only page and
            // return its tiny label as an article. Such a result is not body
            // evidence; use the guarded fallback rather than laundering it.
            if article
                .text_content
                .chars()
                .filter(|ch| !ch.is_whitespace())
                .count()
                < 200
            {
                return markdown_from_html(html);
            }
            let markdown = markdown_from_html(&article.content)?;
            let title = article.title.trim();
            if title.is_empty() || markdown.contains(title) {
                Ok(markdown)
            } else {
                Ok(sanitize_markdown_links(&format!("# {title}\n\n{markdown}")))
            }
        }
        Err(_) => markdown_from_html(html),
    }
}

/// Feed fragments are already scoped; never run Readability over them.
pub fn fragment_to_markdown(html: &str) -> Result<String, HtmlError> {
    markdown_from_html(html)
}

fn looks_like_challenge_html(html: &str) -> bool {
    let lowered = html.to_ascii_lowercase();
    let auth_form = lowered.contains("<form")
        && (lowered.contains("password")
            || lowered.contains("type=\"email\"")
            || lowered.contains("type='email'"));
    let player_shell = [
        "jwplayer",
        "brightcove",
        "data-player",
        "<video",
        "youtube.com/embed",
        "player.vimeo.com",
    ]
    .iter()
    .filter(|marker| lowered.contains(**marker))
    .count()
        >= 2;
    auth_form || player_shell
}

// A marker is not a verdict: reporting may quote challenge text, and raw
// fallback may retain newsletter controls. Require developed, varied prose,
// not merely a long body (repeated instructions and word-number padding fail).
fn has_substantive_prose(markdown: &str) -> bool {
    // News paragraph layout is presentation, not evidence quality. Aggregate
    // prose across blocks, retaining vocabulary diversity to reject repeated
    // instruction padding. Exclude heading/control-link lines, not entire
    // paragraphs: inserting a blank line must not change their contribution.
    let prose = markdown
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#') && !line.contains("]("))
        .collect::<Vec<_>>()
        .join(" ");
    let sentences = prose.matches(['.', '!', '?', '。', '！', '？']).count();
    let words = prose
        .split(|ch: char| !ch.is_alphabetic())
        .filter(|word| word.chars().count() >= 3)
        .map(str::to_lowercase)
        .collect::<std::collections::HashSet<_>>();
    let cjk = prose
        .chars()
        .filter(|ch| {
            matches!(*ch as u32,
        0x3400..=0x4dbf | 0x4e00..=0x9fff | 0x3040..=0x30ff | 0xac00..=0xd7af)
        })
        .collect::<std::collections::HashSet<_>>();
    sentences >= 3
        && prose.chars().filter(|ch| !ch.is_whitespace()).count() >= 400
        && (words.len() >= 40 || cjk.len() >= 40)
}

/// Detect authentication/consent shells in extracted text, not page chrome.
pub fn looks_like_challenge_shell(markdown: &str) -> bool {
    let lowered = markdown.to_ascii_lowercase();
    const HIGH_CONFIDENCE_MARKERS: &[&str] = &[
        "verify you are human",
        "checking your browser",
        "enable javascript and cookies",
        "accept cookies to continue",
        "subscribe to continue",
        "subscribe to read",
        "sign in to continue",
        "log in to continue",
        "this content is behind a paywall",
        "content is behind a paywall",
        "unlock this article",
        "captcha challenge",
        "complete the captcha",
        "solve the captcha",
        "enter the captcha",
        "captcha verification",
    ];
    if HIGH_CONFIDENCE_MARKERS
        .iter()
        .any(|marker| lowered.contains(marker))
        || lowered
            .lines()
            .take(3)
            .any(|line| line.trim().trim_start_matches('#').trim() == "access denied")
    {
        return !has_substantive_prose(markdown);
    }

    // Readability can retain a long login/consent shell without any of the
    // exact phrases above. Require two short, control-like lines so ordinary
    // articles that mention one of these topics are not rejected.
    const STRUCTURAL_MARKERS: &[&str] = &[
        "sign in",
        "log in",
        "login",
        "create account",
        "register",
        "cookie settings",
        "consent preferences",
        "accept all cookies",
        "enable javascript",
        "access denied",
        "play video",
        "watch now",
    ];
    let structural_hits = lowered
        .lines()
        .take(40)
        .filter(|line| {
            let line = line.trim().trim_start_matches('#').trim();
            line.len() <= 96
                && STRUCTURAL_MARKERS
                    .iter()
                    .any(|marker| line.contains(marker))
        })
        .count();
    let has_auth_fields = (lowered.contains("password")
        && (lowered.contains("email") || lowered.contains("username")))
        || lowered.contains("<form");
    (structural_hits >= 2 || (structural_hits >= 1 && has_auth_fields))
        && !has_substantive_prose(markdown)
}

fn markdown_from_html(html: &str) -> Result<String, HtmlError> {
    let markdown = htmd::HtmlToMarkdown::builder()
        .skip_tags(vec![
            "script", "style", "iframe", "object", "embed", "img", "svg", "math",
        ])
        .scripting_enabled(false)
        .build()
        .convert(html)
        .map_err(|e| HtmlError::Conversion(e.to_string()))?;
    // Controls alone are not an interstitial: even the raw fallback may
    // contain a complete article alongside a newsletter or login modal.
    if looks_like_challenge_shell(&markdown)
        || (markdown.chars().filter(|ch| !ch.is_whitespace()).count() < 200
            && looks_like_challenge_html(html))
    {
        return Err(HtmlError::Interstitial);
    }
    normalize_markdown(&markdown)
}

/// Normalize direct Markdown with the same link policy as HTML conversion.
pub fn normalize_markdown(markdown: &str) -> Result<String, HtmlError> {
    Ok(sanitize_markdown_links(&post_process(markdown)))
}

fn post_process(markdown: &str) -> String {
    let mut result = String::with_capacity(markdown.len());
    let mut newlines = 0;
    for ch in markdown.chars() {
        if ch == '\n' {
            newlines += 1;
            if newlines <= 3 {
                result.push(ch);
            }
        } else {
            newlines = 0;
            result.push(ch);
        }
    }
    result.trim().to_string()
}

fn sanitize_markdown_links(markdown: &str) -> String {
    let mut result = String::with_capacity(markdown.len());
    let bytes = markdown.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        let Some(relative_start) = markdown[cursor..].find('[') else {
            result.push_str(&markdown[cursor..]);
            break;
        };
        let start = cursor + relative_start;
        result.push_str(&markdown[cursor..start]);
        if start > 0 && bytes[start - 1] == b'!' {
            result.push('[');
            cursor = start + 1;
            continue;
        }
        let Some(relative_close_label) = markdown[start + 1..].find("](") else {
            result.push('[');
            cursor = start + 1;
            continue;
        };
        let close_label = start + 1 + relative_close_label;
        let Some(close_link) = find_unescaped_byte(bytes, close_label + 2, b')') else {
            result.push('[');
            cursor = start + 1;
            continue;
        };
        let destination = &markdown[close_label + 2..close_link];
        if !is_unsafe_markdown_destination(destination) {
            result.push_str(&markdown[start..=close_link]);
        } else {
            result.push_str(&markdown[start + 1..close_label]);
        }
        cursor = close_link + 1;
    }
    result
}

fn find_unescaped_byte(bytes: &[u8], start: usize, wanted: u8) -> Option<usize> {
    let mut escaped = false;
    for (offset, byte) in bytes.iter().enumerate().skip(start) {
        if escaped {
            escaped = false;
        } else if *byte == b'\\' {
            escaped = true;
        } else if *byte == wanted {
            return Some(offset);
        }
    }
    None
}

fn is_unsafe_markdown_destination(destination: &str) -> bool {
    let destination = destination
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim();
    let decoded = percent_encoding::percent_decode_str(destination).decode_utf8_lossy();
    let decoded =
        decoded.trim_start_matches(|ch: char| ch.is_ascii_control() || ch.is_ascii_whitespace());
    let Some((scheme, _)) = decoded.split_once(':') else {
        return false;
    };
    matches!(
        scheme.to_ascii_lowercase().as_str(),
        "javascript" | "vbscript" | "data" | "file" | "blob"
    )
}
