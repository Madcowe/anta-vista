/// Tokenize a filename stem into cleaned human-readable terms.
/// Input: filename with or without extension (e.g. "cheesy_fish-01.jpg" or "cheesy_fish-01")
/// Output: "cheesy fish" (drops numeric-only "01", joins rest)
pub fn tokenize_filename(filename: &str) -> String {
    // Strip extension
    let stem = match filename.rfind('.') {
        Some(i) if i > 0 => &filename[..i],
        _ => filename,
    };

    // Split on separators
    let tokens: Vec<&str> = stem
        .split(|c: char| c == '_' || c == '-' || c == '.' || c == ' ' || c == '+')
        .collect();

    // Clean and filter
    let words: Vec<String> = tokens
        .iter()
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty() && t.len() > 1 && !t.chars().all(|c| c.is_ascii_digit()))
        .collect();

    words.join(" ")
}

/// Returns None if no useful tokens were found.
pub fn tokenize_filename_opt(filename: &str) -> Option<String> {
    let s = tokenize_filename(filename);
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}
