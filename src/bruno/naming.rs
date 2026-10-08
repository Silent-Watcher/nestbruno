//! Deterministic, filesystem-safe names for Bruno folders and request files.
//!
//! Source-derived text (class and method names) is untrusted, so every name is
//! reduced to a conservative ASCII alphabet **before** it can become a path
//! component. No route path text is ever used in a file name.
//!
//! # Rules
//!
//! - **Request file** (`find-one.bru`): the handler name in kebab-case.
//!   `findOne` gives `find-one`, `getUserByID` gives `get-user-by-id`. Only
//!   ASCII letters and digits survive; everything else is a separator. Names
//!   are lower case, so case-insensitive filesystems cannot conflate two
//!   different names. An empty result becomes `route`.
//! - **Controller folder** (`Users`): the class name without a trailing
//!   `Controller`, reduced to ASCII letters, digits, `_` and `-`. Empty gives
//!   `Controller`.
//! - **Reserved names**: Windows device names (`CON`, `NUL`, `COM1`, ...) get a
//!   trailing `_`, because such names are invalid on Windows even with an
//!   extension.
//! - **Length**: a stem is at most [`MAX_STEM`] characters.
//! - **Collisions** ([`resolve_collisions`]): names are compared
//!   case-insensitively. If several items share a name, **all** of them get
//!   `-<8 hex digits of their fingerprint>`, so the result does not depend on
//!   the order of the items; any remaining duplicate gets `-2`, `-3`, ...
//!   Adding a route that collides therefore renames the previously unsuffixed
//!   file; this is rare and accepted for order independence.

use std::collections::{HashMap, HashSet};

/// Longest name stem (before a collision suffix and the extension).
pub const MAX_STEM: usize = 60;

/// Kebab-case ASCII slug of `input`; may be empty.
pub fn slugify(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut slug = String::new();
    let separator = |slug: &mut String| {
        if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    };
    for (index, &c) in chars.iter().enumerate() {
        if !c.is_ascii_alphanumeric() {
            separator(&mut slug);
            continue;
        }
        if let Some(&previous) = index.checked_sub(1).and_then(|i| chars.get(i)) {
            let lower_to_upper = (previous.is_ascii_lowercase() || previous.is_ascii_digit())
                && c.is_ascii_uppercase();
            // `HTTPServer`: the last capital of an acronym starts a new word.
            let acronym_end = previous.is_ascii_uppercase()
                && c.is_ascii_uppercase()
                && chars.get(index + 1).is_some_and(char::is_ascii_lowercase);
            if lower_to_upper || acronym_end {
                separator(&mut slug);
            }
        }
        slug.push(c.to_ascii_lowercase());
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    truncate(slug)
}

/// Base file stem for a request handler.
pub fn file_stem(handler: &str) -> String {
    let slug = slugify(handler);
    avoid_reserved(if slug.is_empty() {
        "route".to_owned()
    } else {
        slug
    })
}

/// Base folder name for a controller class.
pub fn folder_name(controller: &str) -> String {
    let stem = controller
        .strip_suffix("Controller")
        .filter(|rest| !rest.is_empty())
        .unwrap_or(controller);
    let cleaned: String = stem
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    let cleaned = truncate(cleaned.trim_start_matches('-').to_owned());
    avoid_reserved(if cleaned.is_empty() {
        "Controller".to_owned()
    } else {
        cleaned
    })
}

fn truncate(mut name: String) -> String {
    // The alphabet is ASCII at this point, so byte and character counts agree.
    name.truncate(MAX_STEM);
    while name.ends_with('-') {
        name.pop();
    }
    name
}

fn avoid_reserved(name: String) -> String {
    let lower = name.to_ascii_lowercase();
    let reserved = matches!(lower.as_str(), "con" | "prn" | "aux" | "nul")
        || ["com", "lpt"].iter().any(|prefix| {
            lower
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.len() == 1 && rest.as_bytes()[0].is_ascii_digit())
        });
    if reserved { format!("{name}_") } else { name }
}

/// Make `items` (base name, fingerprint) pairwise distinct, case-insensitively.
/// The result has the same length and order as `items`.
pub fn resolve_collisions(items: &[(String, u64)]) -> Vec<String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (name, _) in items {
        *counts.entry(name.to_ascii_lowercase()).or_default() += 1;
    }

    let mut used: HashSet<String> = HashSet::new();
    items
        .iter()
        .map(|(name, fingerprint)| {
            let mut candidate = if counts[&name.to_ascii_lowercase()] > 1 {
                format!("{name}-{:08x}", fingerprint & 0xffff_ffff)
            } else {
                name.clone()
            };
            // Identical fingerprints (or unlucky coincidences): count up.
            let base = candidate.clone();
            let mut n = 2;
            while !used.insert(candidate.to_ascii_lowercase()) {
                candidate = format!("{base}-{n}");
                n += 1;
            }
            candidate
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn slugify_handles_common_method_names() {
        for (input, expected) in [
            ("findOne", "find-one"),
            ("findAll", "find-all"),
            ("getUserByID", "get-user-by-id"),
            ("HTTPServer", "http-server"),
            ("create", "create"),
            ("update_user", "update-user"),
            ("get-user", "get-user"),
            ("v2Users", "v2-users"),
            ("A", "a"),
            ("  spaced  out  ", "spaced-out"),
            ("a__b--c", "a-b-c"),
        ] {
            assert_eq!(slugify(input), expected, "{input}");
        }
    }

    #[test]
    fn slugify_drops_everything_outside_ascii_alphanumerics() {
        assert_eq!(slugify("../../etc/passwd"), "etc-passwd");
        assert_eq!(slugify("a/b\\c:d"), "a-b-c-d");
        assert_eq!(slugify("کاربران"), "");
        assert_eq!(slugify("naïve"), "na-ve");
        assert_eq!(slugify("a\nb\0c"), "a-b-c");
        assert_eq!(slugify("..."), "");
    }

    #[test]
    fn file_stem_falls_back_and_avoids_reserved_names() {
        assert_eq!(file_stem("findOne"), "find-one");
        assert_eq!(file_stem("کاربران"), "route");
        assert_eq!(file_stem("con"), "con_");
        assert_eq!(file_stem("NUL"), "nul_");
        assert_eq!(file_stem("com1"), "com1_");
        assert_eq!(file_stem("lpt9"), "lpt9_");
        assert_eq!(file_stem("com10"), "com10");
        assert_eq!(file_stem("console"), "console");
    }

    #[test]
    fn folder_names_come_from_the_class_name() {
        for (input, expected) in [
            ("UsersController", "Users"),
            ("AuthController", "Auth"),
            ("AdminUsersController", "AdminUsers"),
            ("Controller", "Controller"),
            ("Users", "Users"),
            ("Weird$Name", "WeirdName"),
            ("../evil", "evil"),
            ("-flag", "flag"),
            ("Ünï", "n"),
            ("", "Controller"),
            ("Nul", "Nul_"),
            ("AUXController", "AUX_"),
        ] {
            assert_eq!(folder_name(input), expected, "{input:?}");
        }
    }

    #[test]
    fn long_names_are_truncated() {
        let long = "a".repeat(500);
        assert_eq!(slugify(&long).len(), MAX_STEM);
        assert_eq!(folder_name(&long).len(), MAX_STEM);
    }

    #[test]
    fn unique_names_are_left_alone() {
        let items = vec![("a".to_owned(), 1), ("b".to_owned(), 2)];
        assert_eq!(resolve_collisions(&items), ["a", "b"]);
    }

    #[test]
    fn all_colliding_names_get_a_fingerprint_suffix() {
        let items = vec![
            ("users".to_owned(), 0x1111_2222_3333_4444),
            ("other".to_owned(), 5),
            ("Users".to_owned(), 0x5555_6666_7777_8888),
        ];
        assert_eq!(
            resolve_collisions(&items),
            ["users-33334444", "other", "Users-77778888"]
        );
    }

    #[test]
    fn suffixing_does_not_depend_on_item_order() {
        let a = ("get".to_owned(), 0xaaaa_aaaa_u64);
        let b = ("get".to_owned(), 0xbbbb_bbbb_u64);
        let forward = resolve_collisions(&[a.clone(), b.clone()]);
        let backward = resolve_collisions(&[b, a]);
        assert_eq!(forward, ["get-aaaaaaaa", "get-bbbbbbbb"]);
        assert_eq!(backward, ["get-bbbbbbbb", "get-aaaaaaaa"]);
    }

    #[test]
    fn identical_fingerprints_are_counted_apart() {
        let items = vec![
            ("x".to_owned(), 7),
            ("x".to_owned(), 7),
            ("x".to_owned(), 7),
        ];
        assert_eq!(
            resolve_collisions(&items),
            ["x-00000007", "x-00000007-2", "x-00000007-3"]
        );
    }

    proptest! {
        #[test]
        fn names_are_always_safe_path_components(input in "\\PC{0,100}") {
            for name in [file_stem(&input), folder_name(&input)] {
                prop_assert!(!name.is_empty());
                prop_assert!(name.len() <= MAX_STEM + 1);
                prop_assert!(name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
                prop_assert!(!name.starts_with('-'));
            }
        }

        #[test]
        fn resolved_names_are_distinct_case_insensitively(
            items in proptest::collection::vec(("[a-cA-C]{1,2}", any::<u64>()), 0..30)
        ) {
            let resolved = resolve_collisions(&items);
            prop_assert_eq!(resolved.len(), items.len());
            let distinct: HashSet<_> = resolved.iter().map(|n| n.to_ascii_lowercase()).collect();
            prop_assert_eq!(distinct.len(), resolved.len());
        }
    }
}
