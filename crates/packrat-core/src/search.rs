use std::cmp::Reverse;

use crate::db::Clip;

const SHORT_TEXT_BONUS_BELOW_CHARS: usize = 50;

pub fn search<'a>(clips: &'a [Clip], query: &str) -> Vec<&'a Clip> {
    let query: Vec<char> = query.chars().map(lower).collect();
    let mut matches: Vec<(usize, &Clip)> = clips
        .iter()
        .filter_map(|clip| Some((score(&clip.content, &query)?, clip)))
        .collect();
    matches.sort_by_key(|(score, _)| Reverse(*score));
    matches.into_iter().map(|(_, clip)| clip).collect()
}

fn score(text: &str, query: &[char]) -> Option<usize> {
    if query.is_empty() {
        return Some(0);
    }
    let len = text.chars().count();
    let mut total = 0;
    let mut matched = 0;
    let mut last_match = None;
    let mut run = 0;
    let mut prev = None;

    for (index, ch) in text.chars().enumerate() {
        if matched == query.len() {
            break;
        }
        if lower(ch) == query[matched] {
            total += len - index;

            if continues_run(last_match, index) {
                run += 1;
                total += run * 10;
            } else {
                run = 0;
            }

            if prev.is_none_or(is_word_boundary) {
                total += 15;
            }

            last_match = Some(index);
            matched += 1;
        }
        prev = Some(ch);
    }

    if matched < query.len() {
        return None;
    }
    Some(total + SHORT_TEXT_BONUS_BELOW_CHARS.saturating_sub(len) * 2)
}

fn continues_run(last_match: Option<usize>, index: usize) -> bool {
    match last_match {
        Some(last) => index == last + 1,
        None => index == 0,
    }
}

fn lower(ch: char) -> char {
    ch.to_lowercase().next().unwrap_or(ch)
}

fn is_word_boundary(ch: char) -> bool {
    matches!(ch, ' ' | '-' | '_' | '.' | '/' | '\\')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clips(contents: &[&str]) -> Vec<Clip> {
        contents
            .iter()
            .enumerate()
            .map(|(index, content)| Clip {
                hash: format!("hash{index}"),
                content: (*content).to_owned(),
                created_at_ms: 0,
                last_used_at_ms: 0,
                pinned: false,
                kind: crate::db::Kind::Text,
                size: content.len(),
            })
            .collect()
    }

    fn found(contents: &[&str], query: &str) -> Vec<String> {
        search(&clips(contents), query)
            .into_iter()
            .map(|clip| clip.content.clone())
            .collect()
    }

    fn score_of(text: &str, query: &str) -> Option<usize> {
        let query: Vec<char> = query.chars().map(lower).collect();
        score(text, &query)
    }

    #[test]
    fn scores_match_clippy() {
        let cases = [
            ("test", "test", 217),
            ("testing", "test", 223),
            ("test example", "test", 233),
            ("xyz test abc", "test", 177),
            ("another test", "test", 105),
            ("some-test-data", "test", 177),
            ("contest result", "test", 170),
            ("my_test_file", "test", 181),
            ("abcdef", "abc", 178),
            ("axbxcx", "abc", 125),
            ("abc123", "abc", 178),
            ("test with lots of additional content here", "test", 291),
            ("myTestFunction", "Test", 174),
            ("testingStuff", "Test", 233),
            ("TestCase", "Test", 225),
            ("Hello World", "HeLLo", 288),
            ("path/to/file", "file", 161),
            ("file.txt", "file", 225),
            ("email@example.com", "example", 303),
            ("test-case_example", "example", 253),
            ("测试内容", "测试", 144),
            ("test with 测试", "测试", 104),
            ("🚀 rocket test", "test", 116),
        ];
        for (text, query, expected) in cases {
            assert_eq!(
                score_of(text, query),
                Some(expected),
                "{text:?} / {query:?}"
            );
        }

        let long = format!("{}test", "a".repeat(60));
        assert_eq!(score_of(&long, "test"), Some(70));
    }

    #[test]
    fn text_without_the_subsequence_does_not_match() {
        assert_eq!(score_of("hello world", "xyz"), None);
        assert_eq!(score_of("", "test"), None);
        assert_eq!(score_of("  ", "test"), None);
        assert_eq!(score_of("tes", "test"), None);
    }

    #[test]
    fn empty_query_returns_everything_in_order() {
        assert_eq!(found(&["b", "a", "c"], ""), ["b", "a", "c"]);
    }

    #[test]
    fn no_clips_means_no_results() {
        assert!(found(&[], "test").is_empty());
    }

    #[test]
    fn non_matching_clips_are_left_out() {
        assert!(found(&["hello world", "foo bar"], "xyz").is_empty());
        assert_eq!(found(&["", "test", "  "], "test"), ["test"]);
    }

    #[test]
    fn matching_ignores_case() {
        let contents = ["Hello World", "GOODBYE"];
        for query in ["hello", "HELLO", "HeLLo"] {
            assert_eq!(found(&contents, query), ["Hello World"], "{query}");
        }
        for query in ["goodbye", "GoodBye"] {
            assert_eq!(found(&contents, query), ["GOODBYE"], "{query}");
        }
    }

    #[test]
    fn letters_need_not_be_adjacent() {
        assert_eq!(found(&["hello world", "foo"], "hwd"), ["hello world"]);
    }

    #[test]
    fn results_are_ordered_by_score() {
        let contents = ["xyz test abc", "test example", "another test", "testing"];
        assert_eq!(
            found(&contents, "test"),
            ["test example", "testing", "xyz test abc", "another test"]
        );
    }

    #[test]
    fn word_boundary_match_beats_mid_word_match() {
        let contents = ["contest result", "some-test-data", "my_test_file"];
        assert_eq!(
            found(&contents, "test"),
            ["my_test_file", "some-test-data", "contest result"]
        );
    }

    #[test]
    fn consecutive_match_beats_scattered_match() {
        assert_eq!(found(&["axbxcx", "abcdef"], "abc"), ["abcdef", "axbxcx"]);
    }

    #[test]
    fn equal_scores_keep_the_original_order() {
        assert_eq!(found(&["abc123", "abcdef"], "abc"), ["abc123", "abcdef"]);
        assert_eq!(found(&["abcdef", "abc123"], "abc"), ["abcdef", "abc123"]);
    }

    #[test]
    fn special_characters_are_matched_literally() {
        let contents = [
            "file.txt",
            "path/to/file",
            "test-case_example",
            "email@example.com",
        ];
        assert_eq!(found(&contents, "file"), ["file.txt", "path/to/file"]);
        assert_eq!(found(&contents, "txt"), ["file.txt"]);
        assert_eq!(found(&contents, "test"), ["test-case_example"]);
        assert_eq!(
            found(&contents, "example"),
            ["email@example.com", "test-case_example"]
        );
        assert_eq!(found(&contents, "@"), ["email@example.com"]);
    }

    #[test]
    fn unicode_text_and_queries_work() {
        let contents = ["测试内容", "test with 测试", "🚀 rocket test"];
        assert_eq!(
            found(&contents, "test"),
            ["test with 测试", "🚀 rocket test"]
        );
        assert_eq!(found(&contents, "测试"), ["测试内容", "test with 测试"]);
    }
}
