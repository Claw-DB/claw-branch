//! Branch name validation and deterministic slug generation.

use crate::error::{BranchError, BranchResult};

/// Returns `true` when every character in `name` is in `[a-zA-Z0-9_/.-]`
/// and the length is in `[1, 128]`.
fn chars_valid(name: &str) -> bool {
    let len = name.len();
    if len == 0 || len > 128 {
        return false;
    }
    name.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'/' || b == b'.' || b == b'-')
}

/// Validates branch names and derives canonical slugs.
pub struct NamingValidator;

impl NamingValidator {
    /// Validates a branch name against reserved names and formatting rules.
    pub fn validate(name: &str) -> BranchResult<()> {
        if !chars_valid(name) {
            return Err(BranchError::InvalidBranchName(name.to_string()));
        }
        if name.starts_with('/') {
            return Err(BranchError::InvalidBranchName(name.to_string()));
        }
        if name.split('/').any(|segment| segment == "..") {
            return Err(BranchError::InvalidBranchName(name.to_string()));
        }
        Ok(())
    }

    /// Produces a lowercase URL-safe slug from an arbitrary branch label.
    pub fn slugify(name: &str) -> String {
        let mut slug = String::with_capacity(name.len());
        let mut last_was_dash = false;

        for ch in name.chars().flat_map(char::to_lowercase) {
            match ch {
                'a'..='z' | '0'..='9' | '_' | '/' | '.' => {
                    if ch == '/' && slug.ends_with('/') {
                        continue;
                    }
                    slug.push(ch);
                    last_was_dash = false;
                }
                '-' => {
                    if slug.is_empty() || last_was_dash {
                        continue;
                    }
                    slug.push('-');
                    last_was_dash = true;
                }
                c if c.is_whitespace() => {
                    if !slug.is_empty() && !last_was_dash {
                        slug.push('-');
                        last_was_dash = true;
                    }
                }
                _ => {}
            }
            if slug.len() >= 128 {
                break;
            }
        }

        slug.truncate(128);
        slug = slug
            .trim_matches(|ch| matches!(ch, '-' | '_' | '/'))
            .to_string();
        if slug.is_empty() {
            slug = "branch".to_string();
        }
        if Self::is_reserved(&slug) {
            slug.push_str("-branch");
        }
        if slug.len() > 128 {
            slug.truncate(128);
            slug = slug
                .trim_matches(|ch| matches!(ch, '-' | '_' | '/'))
                .to_string();
        }
        if Self::validate(&slug).is_err() {
            slug = slug.replace('/', "-");
            slug = slug
                .trim_matches(|ch| matches!(ch, '-' | '_' | '/'))
                .to_string();
        }
        if Self::validate(&slug).is_err() {
            "branch".to_string()
        } else {
            slug
        }
    }

    /// Appends a numeric suffix until the slug is unique among existing slugs.
    pub fn make_unique(slug: &str, existing_slugs: &[String]) -> String {
        if !existing_slugs.iter().any(|existing| existing == slug) {
            return slug.to_string();
        }

        let mut index = 2_u32;
        loop {
            let candidate = format!("{slug}-{index}");
            if !existing_slugs.iter().any(|existing| existing == &candidate) {
                return candidate;
            }
            index += 1;
        }
    }

    /// Returns true when the name is reserved for trunk-like semantics.
    pub fn is_reserved(name: &str) -> bool {
        matches!(
            name.to_ascii_lowercase().as_str(),
            "trunk" | "main" | "master" | "head"
        )
    }

    /// Returns the namespace prefix for a namespaced branch name.
    pub fn namespace(name: &str) -> Option<&str> {
        name.split_once('/').map(|(namespace, _)| namespace)
    }

    /// Returns the last path segment of a namespaced branch name.
    pub fn short_name(name: &str) -> &str {
        name.rsplit('/').next().unwrap_or(name)
    }
}

#[cfg(test)]
mod tests {
    use super::NamingValidator;

    #[test]
    fn accepts_simple_name() {
        assert!(NamingValidator::validate("feature1").is_ok());
    }

    #[test]
    fn accepts_namespaced_name() {
        assert!(NamingValidator::validate("experiment/pricing-v2").is_ok());
    }

    #[test]
    fn accepts_underscores() {
        assert!(NamingValidator::validate("feature_alpha").is_ok());
    }

    #[test]
    fn rejects_empty_name() {
        assert!(NamingValidator::validate("").is_err());
    }

    #[test]
    fn rejects_too_long_name() {
        let name = "a".repeat(129);
        assert!(NamingValidator::validate(&name).is_err());
    }

    #[test]
    fn rejects_whitespace() {
        assert!(NamingValidator::validate("bad name").is_err());
    }

    #[test]
    fn rejects_control_chars() {
        assert!(NamingValidator::validate("bad\nname").is_err());
    }

    #[test]
    fn accepts_leading_hyphen() {
        assert!(NamingValidator::validate("-bad").is_ok());
    }

    #[test]
    fn accepts_leading_underscore() {
        assert!(NamingValidator::validate("_bad").is_ok());
    }

    #[test]
    fn rejects_leading_slash() {
        assert!(NamingValidator::validate("/bad").is_err());
    }

    #[test]
    fn accepts_trailing_hyphen() {
        assert!(NamingValidator::validate("bad-").is_ok());
    }

    #[test]
    fn accepts_trailing_underscore() {
        assert!(NamingValidator::validate("bad_").is_ok());
    }

    #[test]
    fn accepts_trailing_slash() {
        assert!(NamingValidator::validate("bad/").is_ok());
    }

    #[test]
    fn accepts_double_slash() {
        assert!(NamingValidator::validate("exp//bad").is_ok());
    }

    #[test]
    fn accepts_reserved_trunk() {
        assert!(NamingValidator::validate("trunk").is_ok());
    }

    #[test]
    fn accepts_reserved_main_case_insensitive() {
        assert!(NamingValidator::validate("MAIN").is_ok());
    }

    #[test]
    fn rejects_dot_dot_sequences() {
        assert!(NamingValidator::validate("feature/../escape").is_err());
    }

    #[test]
    fn rejects_invalid_symbols() {
        assert!(NamingValidator::validate("bad*").is_err());
    }

    #[test]
    fn slugifies_spaces_and_case() {
        assert_eq!(
            NamingValidator::slugify("Experiment Pricing V2"),
            "experiment-pricing-v2"
        );
    }

    #[test]
    fn slugifies_and_strips_symbols() {
        assert_eq!(NamingValidator::slugify("Feature!*@#Name"), "featurename");
    }

    #[test]
    fn slugify_preserves_namespace() {
        assert_eq!(
            NamingValidator::slugify("Experiment/Pricing V2"),
            "experiment/pricing-v2"
        );
    }

    #[test]
    fn slugify_fixes_reserved_name() {
        assert_eq!(NamingValidator::slugify("trunk"), "trunk-branch");
    }

    #[test]
    fn slugify_falls_back_for_invalid_only_symbols() {
        assert_eq!(NamingValidator::slugify("***"), "branch");
    }

    #[test]
    fn unique_slug_leaves_unique_values_unchanged() {
        let existing = vec!["alpha".to_string(), "beta".to_string()];
        assert_eq!(NamingValidator::make_unique("gamma", &existing), "gamma");
    }

    #[test]
    fn unique_slug_adds_incrementing_suffix() {
        let existing = vec!["alpha".to_string(), "alpha-2".to_string()];
        assert_eq!(NamingValidator::make_unique("alpha", &existing), "alpha-3");
    }

    #[test]
    fn namespace_returns_prefix() {
        assert_eq!(
            NamingValidator::namespace("experiment/pricing-v2"),
            Some("experiment")
        );
    }

    #[test]
    fn namespace_returns_none_without_slash() {
        assert_eq!(NamingValidator::namespace("pricing-v2"), None);
    }

    #[test]
    fn short_name_returns_last_segment() {
        assert_eq!(
            NamingValidator::short_name("experiment/pricing-v2"),
            "pricing-v2"
        );
    }

    #[test]
    fn short_name_returns_input_without_namespace() {
        assert_eq!(NamingValidator::short_name("pricing-v2"), "pricing-v2");
    }
}
