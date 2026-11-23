//! The cap and rotation rules of the private line files.
//!
//! The drop, good-hours, usage and connection logs are all "one line per
//! entry, newest kept" files. They share the same rules: a missing or
//! unreadable file is empty, blank lines do not count, the memory ring keeps
//! the newest [`load_lines`] entries, and the backing file is rewritten only
//! when it has grown past twice the ring cap. Each platform keeps its own
//! file I/O; this module owns only the rules.

/// The non-empty lines of `text`, oldest first, capped to the newest `max`.
pub fn load_lines(text: &str, max: usize) -> Vec<String> {
    let all: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let skip = all.len().saturating_sub(max);
    all[skip..].iter().map(|line| (*line).to_string()).collect()
}

/// How many non-empty lines `text` holds.
pub fn count_lines(text: &str) -> usize {
    text.lines().filter(|line| !line.trim().is_empty()).count()
}

/// Append one line to the newest `max` entries. At the cap the oldest goes.
pub fn push_capped(lines: &mut Vec<String>, line: String, max: usize) {
    if lines.len() == max {
        lines.remove(0);
    }
    lines.push(line);
}

/// True when the backing file should be rewritten from the ring.
pub fn should_compact(file_lines: usize, max: usize) -> bool {
    file_lines >= 2 * max
}

/// Every line with a trailing newline, ready for an atomic rewrite.
pub fn body(lines: &[String]) -> String {
    let mut text = String::new();
    for line in lines {
        text.push_str(line);
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_keeps_only_the_newest_lines_and_skips_blanks() {
        let text = "one\n\n two \nthree\n";
        assert_eq!(
            load_lines(text, 2),
            vec![" two ".to_string(), "three".to_string()]
        );
        assert_eq!(count_lines(text), 3);
        assert!(load_lines("", 2).is_empty());
    }

    #[test]
    fn the_ring_drops_the_oldest_at_the_cap() {
        let mut lines = vec!["a".to_string(), "b".to_string()];
        push_capped(&mut lines, "c".to_string(), 2);
        assert_eq!(lines, vec!["b".to_string(), "c".to_string()]);
    }

    #[test]
    fn compaction_starts_at_twice_the_cap() {
        assert!(!should_compact(3, 2));
        assert!(should_compact(4, 2));
        assert!(should_compact(5, 2));
    }

    #[test]
    fn the_body_ends_every_line_with_a_newline() {
        let lines = vec!["a".to_string(), "b".to_string()];
        assert_eq!(body(&lines), "a\nb\n");
        assert_eq!(body(&[]), "");
    }
}
