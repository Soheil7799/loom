//! Recipes: prompts in steps, saved as Markdown files in ~/.config/loom/recipes (one file
//! each, named as the recipe, its steps between lines of `---`), run on an agent one step per
//! turn (`run_recipe`: the next goes when the turn before it ends; Esc ends the recipe).

use std::path::PathBuf;

/// Made the first time, as an example: a review, fixed, then reviewed again.
const EXAMPLE: &str = "Review the changes on this branch for bugs, risks and code that isn't needed (use /ponytail-review if you have it). Report what you find; don't fix anything yet.
---
Fix what the review found that is worth fixing, then run the tests.
---
Review the changes again (use /code-review if you have it): what did the fixes break or miss?
---
Fix what that found, run the tests, and commit.
";

pub fn folder() -> PathBuf {
    crate::paths::config_dir().join("loom/recipes")
}

/// Every recipe, by name: its steps. The folder, with the example, made the first time.
pub fn list() -> Vec<(String, Vec<String>)> {
    let dir = folder();
    if !dir.exists() && std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(dir.join("Review, fix, review again.md"), EXAMPLE);
    }
    let mut all: Vec<(String, Vec<String>)> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .filter_map(|p| Some((p.file_stem()?.to_string_lossy().into_owned(), steps(&std::fs::read_to_string(&p).ok()?))))
        .filter(|(_, steps)| !steps.is_empty())
        .collect();
    all.sort();
    all
}

/// A recipe's steps: the text between lines of `---`, each trimmed, empty ones left out.
pub fn steps(text: &str) -> Vec<String> {
    let mut steps = vec![String::new()];
    for line in text.lines() {
        if line.trim() == "---" {
            steps.push(String::new());
        } else if let Some(step) = steps.last_mut() {
            step.push_str(line);
            step.push('\n');
        }
    }
    steps.into_iter().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_are_what_lies_between_the_rules() {
        assert_eq!(steps(EXAMPLE).len(), 4);
        assert_eq!(steps("one\n\n---\n---\n  two\nlines \n---\n"), vec!["one".to_owned(), "two\nlines".to_owned()]);
        assert!(steps("").is_empty());
    }
}
