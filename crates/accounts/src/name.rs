//! Account names: short, ASCII letters, digits and hyphens, so they work as credential names,
//! file names and command-line arguments.

/// The longest allowed name.
pub const MAX_NAME_LENGTH: usize = 40;

/// Whether `name` can be used for a saved account: 1–40 characters from `A–Z a–z 0–9 -`, starting
/// with a letter or digit.
#[must_use]
pub fn is_valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else { return false };
    name.len() <= MAX_NAME_LENGTH
        && first.is_ascii_alphanumeric()
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-')
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn accepts_short_ascii_names() {
        for name in ["work", "Work-2", "a", "9lives", "x-", &"a".repeat(40)] {
            assert!(is_valid_name(name), "{name}");
        }
    }

    #[test]
    fn rejects_everything_else() {
        for name in [
            "",
            " ",
            "-work",
            "my account",
            "work_2",
            "wörk",
            "کار",
            "a.b",
            " work",
            "work ",
            "a\"b",
            "a\\b",
            &"a".repeat(41),
        ] {
            assert!(!is_valid_name(name), "{name:?}");
        }
    }

    proptest! {
        #[test]
        fn valid_names_are_safe_in_paths_and_arguments(name in "\\PC{0,50}") {
            if is_valid_name(&name) {
                prop_assert!(!name.is_empty() && name.len() <= MAX_NAME_LENGTH);
                prop_assert!(name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
                prop_assert!(!name.starts_with('-'));
            }
        }

        #[test]
        fn generated_valid_names_pass(name in "[A-Za-z0-9][A-Za-z0-9-]{0,39}") {
            prop_assert!(is_valid_name(&name));
        }
    }
}
