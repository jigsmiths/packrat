//! Commands the popup calls.

use packrat_core::history::History;
use packrat_core::pasteboard::SystemPasteboard;
use packrat_core::search::search;
use serde::Serialize;
use tauri::{AppHandle, State};

use crate::capture::now_ms;
use crate::{popup, AppState};

const MAX_ROWS: usize = 500;
const SUMMARY_CHARS: usize = 200;
// Larger clips are cut in the preview so the webview stays responsive.
const PREVIEW_BYTES: usize = 100 * 1024;

#[derive(Serialize)]
pub struct Row {
    hash: String,
    summary: String,
    pinned: bool,
}

#[derive(Serialize)]
pub struct Listing {
    rows: Vec<Row>,
    /// Matches before the row limit was applied.
    matched: usize,
    total: usize,
}

#[derive(Serialize)]
pub struct Preview {
    text: String,
    truncated: bool,
    chars: usize,
}

fn with_history<T>(
    state: &AppState,
    action: impl FnOnce(&mut History) -> packrat_core::Result<T>,
) -> Result<T, String> {
    let mut history = state.history();
    match history.as_mut() {
        Some(history) => action(history).map_err(|err| err.to_string()),
        None => Err(state
            .problem()
            .clone()
            .unwrap_or_else(|| "history is not open".to_owned())),
    }
}

/// One line of at most `SUMMARY_CHARS` characters, with whitespace collapsed.
fn summarize(content: &str) -> String {
    let mut summary = String::new();
    let mut chars = 0;
    for word in content.split_whitespace() {
        if chars > 0 {
            summary.push(' ');
            chars += 1;
        }
        for ch in word.chars() {
            if chars >= SUMMARY_CHARS {
                summary.push('…');
                return summary;
            }
            summary.push(ch);
            chars += 1;
        }
    }
    summary
}

fn preview(content: &str) -> Preview {
    let mut end = content.len().min(PREVIEW_BYTES);
    while !content.is_char_boundary(end) {
        end -= 1;
    }
    Preview {
        text: content[..end].to_owned(),
        truncated: end < content.len(),
        chars: content.chars().count(),
    }
}

#[tauri::command]
pub fn list_clips(state: State<AppState>, query: String) -> Result<Listing, String> {
    with_history(&state, |history| {
        let matches = search(history.clips(), &query);
        Ok(Listing {
            matched: matches.len(),
            total: history.clips().len(),
            rows: matches
                .into_iter()
                .take(MAX_ROWS)
                .map(|clip| Row {
                    hash: clip.hash.clone(),
                    summary: summarize(&clip.content),
                    pinned: clip.pinned,
                })
                .collect(),
        })
    })
}

#[tauri::command]
pub fn preview_clip(state: State<AppState>, hash: String) -> Result<Preview, String> {
    with_history(&state, |history| {
        history
            .clips()
            .iter()
            .find(|clip| clip.hash == hash)
            .map(|clip| preview(&clip.content))
            .ok_or(packrat_core::Error::NotFound)
    })
}

/// Puts the clip on the pasteboard, moves it to the top and hides the popup.
#[tauri::command]
pub fn copy_clip(app: AppHandle, state: State<AppState>, hash: String) -> Result<(), String> {
    let content = with_history(&state, |history| {
        let content = history
            .clips()
            .iter()
            .find(|clip| clip.hash == hash)
            .map(|clip| clip.content.clone())
            .ok_or(packrat_core::Error::NotFound)?;
        history.touch(&hash, now_ms())?;
        Ok(content)
    })?;
    if !SystemPasteboard.write_text(&content) {
        return Err("could not write to the clipboard".to_owned());
    }
    log::info!("clip copied: {}", short(&hash));
    popup::hide(&app);
    Ok(())
}

#[tauri::command]
pub fn toggle_pin(state: State<AppState>, hash: String) -> Result<bool, String> {
    let pinned = with_history(&state, |history| history.toggle_pin(&hash))?;
    log::info!("clip pinned={pinned}: {}", short(&hash));
    Ok(pinned)
}

#[tauri::command]
pub fn delete_clip(state: State<AppState>, hash: String) -> Result<(), String> {
    with_history(&state, |history| history.delete(&hash))?;
    log::info!("clip deleted: {}", short(&hash));
    Ok(())
}

/// Enough of a hash to follow one clip through the log, which never holds content.
fn short(hash: &str) -> &str {
    hash.get(..8).unwrap_or(hash)
}

#[tauri::command]
pub fn hide_popup(app: AppHandle) {
    popup::hide(&app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_collapses_whitespace_to_one_line() {
        assert_eq!(
            summarize("  fn main() {\n\tprintln!();\n}\n"),
            "fn main() { println!(); }"
        );
        assert_eq!(summarize(" \n\t "), "");
    }

    #[test]
    fn summary_is_cut_at_the_character_limit() {
        let long = "é".repeat(SUMMARY_CHARS + 50);
        let summary = summarize(&long);
        assert_eq!(summary.chars().count(), SUMMARY_CHARS + 1);
        assert!(summary.ends_with('…'));

        let exact = "a".repeat(SUMMARY_CHARS);
        assert_eq!(summarize(&exact), exact);
    }

    #[test]
    fn short_clips_are_previewed_whole() {
        let shown = preview("héllo");
        assert_eq!(shown.text, "héllo");
        assert!(!shown.truncated);
        assert_eq!(shown.chars, 5);
    }

    #[test]
    fn long_clips_are_cut_on_a_character_boundary() {
        // Two-byte characters, offset by one so the limit falls inside a character.
        let content = format!("a{}", "é".repeat(PREVIEW_BYTES));
        let shown = preview(&content);
        assert!(shown.truncated);
        assert_eq!(shown.text.len(), PREVIEW_BYTES - 1);
        assert_eq!(shown.chars, PREVIEW_BYTES + 1);
    }
}
