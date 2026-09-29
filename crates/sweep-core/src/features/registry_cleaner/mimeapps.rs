//! `mimeapps.list`: parse the associations and remove single desktop ids, leaving every other
//! byte of the file (comments, ordering, line endings, spacing) untouched.

/// Sections whose entries name applications the user relies on. `[Removed Associations]` only
/// blacklists, so a missing application there is harmless and is left alone.
pub const SCANNED_SECTIONS: [&str; 2] = ["Default Applications", "Added Associations"];

#[derive(Debug, Clone, PartialEq)]
pub struct Assoc {
    pub section: String,
    pub mime: String,
    pub ids: Vec<String>,
}

fn section_of(trimmed: &str) -> Option<&str> {
    (trimmed.starts_with('[') && trimmed.ends_with(']') && trimmed.len() >= 2)
        .then(|| &trimmed[1..trimmed.len() - 1])
}

/// Every `mime=a.desktop;b.desktop;` line in the scanned sections.
pub fn associations(content: &str) -> Vec<Assoc> {
    let mut out = Vec::new();
    let mut section = String::new();
    for line in content.lines() {
        let t = line.trim();
        if let Some(s) = section_of(t) {
            section = s.to_string();
            continue;
        }
        if t.is_empty() || t.starts_with('#') || !SCANNED_SECTIONS.contains(&section.as_str()) {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let ids: Vec<String> = v
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        out.push(Assoc {
            section: section.clone(),
            mime: k.trim().to_string(),
            ids,
        });
    }
    out
}

/// Remove the `(section, mime, desktop id)` triples from `content`. A line whose list becomes
/// empty is dropped entirely. Returns the new content and how many ids were removed.
pub fn remove_ids(content: &str, removals: &[(String, String, String)]) -> (String, usize) {
    let mut out = String::with_capacity(content.len());
    let mut section = String::new();
    let mut removed = 0usize;
    for line in content.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        let eol = &line[body.len()..];
        let t = body.trim();
        if let Some(s) = section_of(t) {
            section = s.to_string();
            out.push_str(line);
            continue;
        }
        if t.is_empty() || t.starts_with('#') {
            out.push_str(line);
            continue;
        }
        if let Some((k, v)) = body.split_once('=') {
            let mime = k.trim();
            let here: Vec<&str> = removals
                .iter()
                .filter(|(s, m, _)| *s == section && m == mime)
                .map(|(_, _, id)| id.as_str())
                .collect();
            if !here.is_empty() {
                let parts: Vec<&str> = v.split(';').collect();
                let kept: Vec<&str> = parts
                    .iter()
                    .copied()
                    .filter(|p| {
                        let p = p.trim();
                        p.is_empty() || !here.contains(&p)
                    })
                    .collect();
                let dropped = parts.len() - kept.len();
                if dropped == 0 {
                    out.push_str(line);
                    continue;
                }
                removed += dropped;
                if kept.iter().all(|p| p.trim().is_empty()) {
                    continue; // nothing left: drop the line
                }
                // keep the spacing after `=` even when the first id was the one removed
                let lead = &v[..v.len() - v.trim_start().len()];
                out.push_str(k);
                out.push('=');
                out.push_str(lead);
                out.push_str(kept.join(";").trim_start());
                out.push_str(eol);
                continue;
            }
        }
        out.push_str(line);
    }
    (out, removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# my associations\n[Default Applications]\ntext/plain=gedit.desktop;\nimage/png = gone.desktop;eog.desktop;\nx-scheme-handler/http=gone.desktop\n\n[Added Associations]\ntext/plain=gone.desktop;gedit.desktop;\n\n[Removed Associations]\ntext/html=gone.desktop;\n";

    fn r(s: &str, m: &str, i: &str) -> (String, String, String) {
        (s.into(), m.into(), i.into())
    }

    #[test]
    fn parses_scanned_sections_only() {
        let a = associations(SAMPLE);
        assert_eq!(a.len(), 4);
        assert_eq!(a[0].section, "Default Applications");
        assert_eq!(a[0].mime, "text/plain");
        assert_eq!(a[0].ids, ["gedit.desktop"]);
        assert_eq!(a[1].mime, "image/png");
        assert_eq!(a[1].ids, ["gone.desktop", "eog.desktop"]);
        assert_eq!(a[2].ids, ["gone.desktop"]);
        assert_eq!(a[3].section, "Added Associations");
        assert!(!a.iter().any(|x| x.section == "Removed Associations"));
    }

    #[test]
    fn removal_is_byte_exact_elsewhere() {
        let (out, n) = remove_ids(
            SAMPLE,
            &[
                r("Default Applications", "image/png", "gone.desktop"),
                r("Default Applications", "x-scheme-handler/http", "gone.desktop"),
                r("Added Associations", "text/plain", "gone.desktop"),
            ],
        );
        assert_eq!(n, 3);
        assert_eq!(
            out,
            "# my associations\n[Default Applications]\ntext/plain=gedit.desktop;\nimage/png = eog.desktop;\n\n[Added Associations]\ntext/plain=gedit.desktop;\n\n[Removed Associations]\ntext/html=gone.desktop;\n"
        );
    }

    #[test]
    fn same_mime_in_another_section_is_untouched() {
        let (out, n) = remove_ids(SAMPLE, &[r("Removed Associations", "text/html", "gone.desktop")]);
        assert_eq!(n, 1);
        assert!(!out.contains("text/html"));
        assert!(out.contains("text/plain=gone.desktop;gedit.desktop;"));
        let (same, n) = remove_ids(SAMPLE, &[r("Default Applications", "text/html", "gone.desktop")]);
        assert_eq!((same.as_str(), n), (SAMPLE, 0));
    }

    #[test]
    fn crlf_missing_final_newline_and_no_trailing_semicolon() {
        let src = "[Default Applications]\r\na/b=x.desktop;y.desktop\r\nc/d=z.desktop";
        let (out, n) = remove_ids(
            src,
            &[r("Default Applications", "a/b", "y.desktop"), r("Default Applications", "c/d", "z.desktop")],
        );
        assert_eq!(n, 2);
        assert_eq!(out, "[Default Applications]\r\na/b=x.desktop\r\n");
        let (out, _) = remove_ids(src, &[r("Default Applications", "a/b", "x.desktop")]);
        assert_eq!(out, "[Default Applications]\r\na/b=y.desktop\r\nc/d=z.desktop");
    }

    #[test]
    fn nothing_to_remove_returns_identical_content() {
        for src in [SAMPLE, "", "no newline", "\n\n", "[X]\nk=v"] {
            let (out, n) = remove_ids(src, &[r("X", "zzz", "q.desktop")]);
            assert_eq!(out, src);
            assert_eq!(n, 0);
        }
    }

    #[test]
    fn duplicate_lines_are_all_handled() {
        let src = "[Default Applications]\na/b=x.desktop;\na/b=x.desktop;y.desktop;\n";
        let (out, n) = remove_ids(src, &[r("Default Applications", "a/b", "x.desktop")]);
        assert_eq!(n, 2);
        assert_eq!(out, "[Default Applications]\na/b=y.desktop;\n");
    }
}
