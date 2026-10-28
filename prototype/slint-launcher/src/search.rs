#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub aliases: Vec<String>,
}

pub fn search<'a>(entries: &'a [Entry], query: &str) -> Vec<&'a Entry> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return vec![];
    }
    entries
        .iter()
        .filter(|entry| {
            entry.name.to_lowercase().contains(&query)
                || entry
                    .aliases
                    .iter()
                    .any(|alias| alias.to_lowercase().contains(&query))
        })
        .take(8)
        .collect()
}

pub fn move_selection(current: i32, direction: i32, len: usize) -> i32 {
    if len == 0 {
        -1
    } else {
        (current + direction).clamp(0, len as i32 - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Vec<Entry> {
        (0..247)
            .map(|i| Entry {
                name: format!("记事本 Fixture App {i:03}"),
                aliases: vec![format!("alpha-{i:03}")],
            })
            .collect()
    }
    #[test]
    fn blank_query_has_no_results() {
        assert!(search(&fixture(), "  ").is_empty());
    }
    #[test]
    fn case_insensitive_query_is_capped_at_eight() {
        assert_eq!(search(&fixture(), "APP").len(), 8);
    }
    #[test]
    fn alias_selects_exact_entry() {
        assert!(search(&fixture(), "alpha-246")[0].name.ends_with("246"));
    }
    #[test]
    fn unicode_matches_without_corruption() {
        assert_eq!(search(&fixture(), "记事本").len(), 8);
    }
    #[test]
    fn no_match_is_empty() {
        assert!(search(&fixture(), "missing").is_empty());
    }
    #[test]
    fn selection_stays_in_bounds() {
        assert_eq!(move_selection(-1, 1, 0), -1);
        assert_eq!(move_selection(-1, 1, 8), 0);
        assert_eq!(move_selection(0, -1, 8), 0);
        assert_eq!(move_selection(7, 1, 8), 7);
    }
}
