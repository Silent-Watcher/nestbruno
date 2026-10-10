//! The ownership stamp: one machine-readable line in a generated request's
//! `docs` block.
//!
//! ```text
//! docs {
//!   NestJS: UsersController.findOne
//!   Source: src/users/users.controller.ts
//!   NestBruno: v1 id=<16 hex> key=<16 hex> anchor=<16 hex> hash=<16 hex>
//! }
//! ```
//!
//! - `id`: [`RouteIdentity::fingerprint`](super::identity::RouteIdentity::fingerprint).
//! - `key`: [`RouteIdentity::key`](super::identity::RouteIdentity::key)
//!   (controller + method + path), to recognize a route after a handler rename
//!   or controller move.
//! - `anchor`: [`RouteIdentity::anchor`](super::identity::RouteIdentity::anchor)
//!   (controller + handler), to recognize a route whose path or method changed.
//! - `hash`: hash of the file **as generated**, with the stamp line and the
//!   `seq` line removed (see [`normalized`]). A file whose current normalized
//!   content hashes differently has been edited by the user.
//!
//! # Why the `docs` block
//!
//! Bruno re-saves requests from its own model and drops unknown keys in `meta`,
//! but it keeps `docs`. If a user deletes the line the file is simply no longer
//! recognized as generated and is preserved untouched, which is the safe
//! failure mode. The line is therefore a *claim to be verified*, never an
//! authority: classification always re-checks the hash.
//!
//! # Why `seq` is excluded
//!
//! Reordering requests in Bruno rewrites `seq`. That is not an edit worth a
//! conflict, so it is ignored when judging modification, and synchronization
//! keeps the user's `seq` when it updates a file.

use std::ops::Range;

use super::identity::{RouteIdentity, content_hash};

const PREFIX: &str = "NestBruno:";
const VERSION: &str = "v1";

/// The parsed stamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    pub id: u64,
    pub key: u64,
    pub anchor: u64,
    /// Hash of the normalized generated content.
    pub hash: u64,
}

/// What a file says about its origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// No stamp line.
    Unstamped,
    /// Exactly one well-formed stamp.
    Stamped(Stamp),
    /// A stamp line that cannot be trusted (reason).
    Malformed(String),
}

/// Insert the stamp for `identity` into freshly serialized request text.
///
/// `unstamped` must contain a `docs` block, which generated requests always do.
pub fn stamp(unstamped: &str, identity: &RouteIdentity) -> Result<String, &'static str> {
    let hash = content_hash(&normalized(unstamped));
    let line = format!(
        "  {PREFIX} {VERSION} id={:016x} key={:016x} anchor={:016x} hash={hash:016x}\n",
        identity.fingerprint(),
        identity.key(),
        identity.anchor()
    );
    let open = unstamped
        .find("\ndocs {\n")
        .ok_or("the request has no docs block")?;
    let close = unstamped[open + 1..]
        .find("\n}\n")
        .ok_or("the docs block is not terminated")?;
    let at = open + 1 + close + 1;
    let mut out = String::with_capacity(unstamped.len() + line.len());
    out.push_str(&unstamped[..at]);
    out.push_str(&line);
    out.push_str(&unstamped[at..]);
    Ok(out)
}

/// Read the stamp of `text`, if any.
pub fn parse(text: &str) -> Parsed {
    match find_stamp_line(text) {
        Err(reason) => Parsed::Malformed(reason),
        Ok(None) => Parsed::Unstamped,
        Ok(Some(range)) => match parse_line(&text[range]) {
            Ok(stamp) => Parsed::Stamped(stamp),
            Err(reason) => Parsed::Malformed(reason),
        },
    }
}

/// The text a stamp's `hash` refers to: `text` without its stamp line and
/// without the `seq` line of the `meta` block.
pub fn normalized(text: &str) -> String {
    let mut remove: Vec<Range<usize>> = Vec::new();
    if let Ok(Some(range)) = find_stamp_line(text) {
        remove.push(range);
    }
    if let Some(range) = seq_line(text) {
        remove.push(range);
    }
    remove.sort_by_key(|r| r.start);
    let mut out = String::with_capacity(text.len());
    let mut position = 0;
    for range in remove {
        out.push_str(&text[position..range.start]);
        position = range.end;
    }
    out.push_str(&text[position..]);
    out
}

/// Hash of [`normalized`] text.
pub fn normalized_hash(text: &str) -> u64 {
    content_hash(&normalized(text))
}

/// The `seq` value of a request, if present.
pub fn read_seq(text: &str) -> Option<u32> {
    let range = seq_line(text)?;
    text[range].trim().strip_prefix("seq:")?.trim().parse().ok()
}

/// `text` with its `seq` replaced by `seq` (unchanged if there is none).
pub fn with_seq(text: &str, seq: u32) -> String {
    match seq_line(text) {
        Some(range) => format!(
            "{}  seq: {seq}\n{}",
            &text[..range.start],
            &text[range.end..]
        ),
        None => text.to_owned(),
    }
}

/// Byte range (including the newline) of the first-level `seq` line of the
/// `meta` block.
fn seq_line(text: &str) -> Option<Range<usize>> {
    let mut in_meta = false;
    for (start, line) in lines(text) {
        let content = line.trim_end_matches(['\n', '\r']);
        if !in_meta {
            if content == "meta {" {
                in_meta = true;
            }
            continue;
        }
        if content == "}" {
            return None;
        }
        if content.starts_with("  seq:") && !content.starts_with("   ") {
            return Some(start..start + line.len());
        }
    }
    None
}

/// Byte range (including the newline) of the single stamp line inside a
/// `docs` block. `Ok(None)` if there is none.
fn find_stamp_line(text: &str) -> Result<Option<Range<usize>>, String> {
    let mut in_docs = false;
    let mut found: Vec<Range<usize>> = Vec::new();
    for (start, line) in lines(text) {
        let content = line.trim_end_matches(['\n', '\r']);
        if in_docs {
            if content == "}" {
                in_docs = false;
            } else if content.trim_start().starts_with(PREFIX) {
                found.push(start..start + line.len());
            }
        } else if content == "docs {" {
            in_docs = true;
        }
    }
    if in_docs && !found.is_empty() {
        return Err("the docs block containing the NestBruno line is not terminated".to_owned());
    }
    match found.len() {
        0 => Ok(None),
        1 => Ok(found.pop()),
        n => Err(format!("{n} NestBruno lines in the docs block")),
    }
}

fn lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut offset = 0;
    text.split_inclusive('\n').map(move |line| {
        let start = offset;
        offset += line.len();
        (start, line)
    })
}

fn parse_line(line: &str) -> Result<Stamp, String> {
    let body = line
        .trim()
        .strip_prefix(PREFIX)
        .ok_or_else(|| "not a NestBruno line".to_owned())?;
    let mut fields = body.split_whitespace();
    match fields.next() {
        Some(VERSION) => {}
        Some(other) => return Err(format!("unsupported metadata version {other:?}")),
        None => return Err("empty NestBruno line".to_owned()),
    }
    let mut take = |name: &str| -> Result<u64, String> {
        let field = fields.next().ok_or_else(|| format!("missing {name}"))?;
        let value = field
            .strip_prefix(name)
            .and_then(|rest| rest.strip_prefix('='))
            .ok_or_else(|| format!("expected {name}=..., found {field:?}"))?;
        if value.len() != 16
            || !value
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(format!("{name} must be 16 lowercase hex digits"));
        }
        u64::from_str_radix(value, 16).map_err(|_| format!("{name} is not a number"))
    };
    let stamp = Stamp {
        id: take("id")?,
        key: take("key")?,
        anchor: take("anchor")?,
        hash: take("hash")?,
    };
    if fields.next().is_some() {
        return Err("unexpected extra fields".to_owned());
    }
    Ok(stamp)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use proptest::prelude::*;

    use super::*;
    use crate::bruno::identity::ControllerIdentity;
    use crate::route::{HttpMethod, RoutePath};

    fn identity(handler: &str) -> RouteIdentity {
        RouteIdentity {
            controller: ControllerIdentity {
                name: "UsersController".into(),
                file: PathBuf::from("src/users.controller.ts"),
            },
            method: HttpMethod::Get,
            path: RoutePath::Static("/users".into()),
            handler: handler.into(),
        }
    }

    const BASE: &str = "meta {\n  name: GET /users\n  type: http\n  seq: 3\n}\n\nget {\n  url: {{baseUrl}}/users\n  body: none\n  auth: inherit\n}\n\ndocs {\n  NestJS: UsersController.findAll\n}\n";

    fn stamped() -> String {
        stamp(BASE, &identity("findAll")).unwrap()
    }

    #[test]
    fn stamp_is_the_last_docs_line_and_the_rest_is_untouched() {
        let text = stamped();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(
            lines[lines.len() - 2].trim_start().split(' ').next(),
            Some("NestBruno:")
        );
        assert_eq!(lines.last(), Some(&"}"));
        assert!(text.starts_with("meta {\n  name: GET /users"));
        // Removing the stamp line gives back exactly the input.
        let range = find_stamp_line(&text).unwrap().unwrap();
        assert_eq!(
            format!("{}{}", &text[..range.start], &text[range.end..]),
            BASE
        );
    }

    #[test]
    fn a_stamp_round_trips() {
        let id = identity("findAll");
        let Parsed::Stamped(parsed) = parse(&stamped()) else {
            panic!("not stamped")
        };
        assert_eq!(parsed.id, id.fingerprint());
        assert_eq!(parsed.key, id.key());
        assert_eq!(parsed.anchor, id.anchor());
        assert_eq!(parsed.hash, normalized_hash(BASE));
    }

    #[test]
    fn the_anchor_ignores_method_path_and_file() {
        let a = identity("findAll");
        let mut b = identity("findAll");
        b.method = HttpMethod::Post;
        b.path = RoutePath::Static("/elsewhere".into());
        b.controller.file = PathBuf::from("src/moved.ts");
        assert_eq!(a.anchor(), b.anchor());
        assert_ne!(a.anchor(), identity("other").anchor());
        let mut c = identity("findAll");
        c.controller.name = "OtherController".into();
        assert_ne!(a.anchor(), c.anchor());
    }

    #[test]
    fn the_key_ignores_handler_and_file_but_the_id_does_not() {
        let a = identity("findAll");
        let mut b = identity("renamed");
        b.controller.file = PathBuf::from("src/moved.controller.ts");
        assert_ne!(a.fingerprint(), b.fingerprint());
        assert_eq!(a.key(), b.key());
        let mut c = identity("findAll");
        c.path = RoutePath::Static("/other".into());
        assert_ne!(a.key(), c.key());
    }

    #[test]
    fn stamping_is_deterministic() {
        assert_eq!(stamped(), stamped());
    }

    #[test]
    fn requests_without_docs_cannot_be_stamped() {
        assert!(stamp("meta {\n  name: x\n}\n", &identity("h")).is_err());
    }

    #[test]
    fn unstamped_files_are_reported_as_such() {
        assert_eq!(parse(BASE), Parsed::Unstamped);
        assert_eq!(parse(""), Parsed::Unstamped);
        assert_eq!(parse("garbage \u{0} \u{ffff}"), Parsed::Unstamped);
    }

    #[test]
    fn a_stamp_outside_a_docs_block_is_ignored() {
        let text = format!(
            "{BASE}\nscript:pre-request {{\n  // NestBruno: v1 id=0000000000000000 key=0000000000000000 anchor=0000000000000000 hash=0000000000000000\n}}\n"
        );
        assert_eq!(parse(&text), Parsed::Unstamped);
        let text = "meta {\n  name: NestBruno: v1 id=0000000000000000 key=0000000000000000 anchor=0000000000000000 hash=0000000000000000\n}\n";
        assert_eq!(parse(text), Parsed::Unstamped);
    }

    #[test]
    fn malformed_stamps_are_detected() {
        let good = "NestBruno: v1 id=0123456789abcdef key=0123456789abcdef anchor=0123456789abcdef hash=0123456789abcdef";
        let wrap = |line: &str| format!("docs {{\n  {line}\n}}\n");
        assert!(matches!(parse(&wrap(good)), Parsed::Stamped(_)));
        for bad in [
            "NestBruno:",
            "NestBruno: v2 id=0123456789abcdef key=0123456789abcdef anchor=0123456789abcdef hash=0123456789abcdef",
            "NestBruno: v1 id=0123456789abcde key=0123456789abcdef anchor=0123456789abcdef hash=0123456789abcdef",
            "NestBruno: v1 id=0123456789ABCDEF key=0123456789abcdef anchor=0123456789abcdef hash=0123456789abcdef",
            "NestBruno: v1 id=0123456789abcdeg key=0123456789abcdef anchor=0123456789abcdef hash=0123456789abcdef",
            "NestBruno: v1 key=0123456789abcdef id=0123456789abcdef hash=0123456789abcdef",
            "NestBruno: v1 id=0123456789abcdef key=0123456789abcdef anchor=0123456789abcdef",
            "NestBruno: v1 id=0123456789abcdef key=0123456789abcdef anchor=0123456789abcdef hash=0123456789abcdef extra=1",
            "NestBruno: v1 id=0123456789abcdef key=0123456789abcdef hash=0123456789abcdef",
            "NestBruno: v1 id=+123456789abcdef key=0123456789abcdef anchor=0123456789abcdef hash=0123456789abcdef",
        ] {
            assert!(matches!(parse(&wrap(bad)), Parsed::Malformed(_)), "{bad}");
        }
    }

    #[test]
    fn two_stamps_are_ambiguous() {
        let text = stamped();
        let range = find_stamp_line(&text).unwrap().unwrap();
        let line = text[range.clone()].to_owned();
        let doubled = format!("{}{line}{}", &text[..range.end], &text[range.end..]);
        assert!(matches!(parse(&doubled), Parsed::Malformed(m) if m.contains("2 NestBruno lines")));
    }

    #[test]
    fn an_unterminated_docs_block_with_a_stamp_is_malformed() {
        let text = stamped();
        let cut = &text[..text.len() - 2]; // drop the closing "}\n"
        assert!(matches!(parse(cut), Parsed::Malformed(_)));
    }

    #[test]
    fn seq_changes_do_not_count_as_modification_but_other_changes_do() {
        let Parsed::Stamped(s) = parse(&stamped()) else {
            panic!()
        };
        let reordered = with_seq(&stamped(), 9);
        assert_eq!(read_seq(&reordered), Some(9));
        assert_eq!(normalized_hash(&reordered), s.hash);
        let edited = stamped().replace("body: none", "body: json");
        assert_ne!(normalized_hash(&edited), s.hash);
        let extra_docs = stamped().replace("  NestJS:", "  My note\n  NestJS:");
        assert_ne!(normalized_hash(&extra_docs), s.hash);
    }

    #[test]
    fn crlf_conversion_counts_as_modification() {
        let Parsed::Stamped(s) = parse(&stamped()) else {
            panic!()
        };
        assert_ne!(normalized_hash(&stamped().replace('\n', "\r\n")), s.hash);
    }

    #[test]
    fn seq_helpers_handle_missing_and_odd_values() {
        assert_eq!(read_seq("meta {\n  name: x\n}\n"), None);
        assert_eq!(read_seq("meta {\n  seq: abc\n}\n"), None);
        assert_eq!(
            with_seq("meta {\n  name: x\n}\n", 4),
            "meta {\n  name: x\n}\n"
        );
        // A seq outside `meta` is not the request's seq.
        assert_eq!(read_seq("get {\n  seq: 1\n}\n"), None);
        assert_eq!(with_seq(BASE, 12).matches("seq: 12").count(), 1);
    }

    proptest! {
        #[test]
        fn parsing_arbitrary_text_never_panics(text in "\\PC{0,400}") {
            let _ = parse(&text);
            let _ = normalized(&text);
            let _ = read_seq(&text);
            let _ = with_seq(&text, 1);
        }

        #[test]
        fn garbage_after_the_prefix_is_never_accepted_as_a_stamp(rest in "[ -~]{0,60}") {
            let text = format!("docs {{\n  NestBruno:{rest}\n}}\n");
            if let Parsed::Stamped(_) = parse(&text) {
                // Only a fully well-formed line may be accepted.
                prop_assert!(rest.trim().starts_with("v1 id="));
            }
        }

        #[test]
        fn stamp_then_parse_always_recovers_the_identity(handler in "[a-zA-Z]{1,12}", seq in 1u32..1000) {
            let base = BASE.replace("seq: 3", &format!("seq: {seq}"));
            let id = identity(&handler);
            let text = stamp(&base, &id).unwrap();
            let Parsed::Stamped(s) = parse(&text) else { panic!("not stamped") };
            prop_assert_eq!((s.id, s.key), (id.fingerprint(), id.key()));
            prop_assert_eq!(s.hash, normalized_hash(&text));
            prop_assert_eq!(read_seq(&text), Some(seq));
        }
    }
}
