//! Route identity: what makes a generated request "the same request" across
//! runs, machines and (later) synchronizations.
//!
//! # Definition
//!
//! A route is identified by **(controller, HTTP method, normalized path,
//! handler)** where the controller is its class name *and* its project-relative
//! source file. Nothing machine-specific (absolute paths), random or
//! time-based is involved, so the identity is identical on every machine.
//!
//! # Limits (documented, not solved here)
//!
//! - The handler name is part of the identity, so renaming a handler changes
//!   the fingerprint even though the route did not change; likewise a renamed
//!   path parameter (`:id` to `:userId`) changes the path. Matching such
//!   changes to the old request is the job of Phase 5, which can compare the
//!   individual fields of [`RouteIdentity`] (method + path is the strongest
//!   signal) instead of only the fingerprint.
//! - Moving a controller file changes its identity (the file is included to
//!   keep two same-named controllers in different modules apart).
//!
//! The fingerprint is FNV-1a (64 bit) over length-prefixed fields. It is used
//! to disambiguate names, never for security, and is implemented here (about
//! ten lines) so that its value is stable across Rust versions, unlike
//! `std`'s `DefaultHasher`.

use std::path::{Path, PathBuf};

use crate::route::{Controller, HttpMethod, Route, RoutePath};

/// The identity of one route.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RouteIdentity {
    pub controller: ControllerIdentity,
    pub method: HttpMethod,
    pub path: RoutePath,
    pub handler: String,
}

/// The identity of one controller class.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ControllerIdentity {
    pub name: String,
    /// Path relative to the project root.
    pub file: PathBuf,
}

impl ControllerIdentity {
    pub fn of(controller: &Controller) -> Self {
        Self {
            name: controller.name.clone(),
            file: controller.source.file.to_path_buf(),
        }
    }

    pub fn fingerprint(&self) -> u64 {
        let mut hash = Fnv::new();
        hash.field(self.name.as_bytes());
        hash.field(path_bytes(&self.file).as_bytes());
        hash.finish()
    }
}

impl RouteIdentity {
    pub fn of(controller: &Controller, route: &Route) -> Self {
        Self {
            controller: ControllerIdentity::of(controller),
            method: route.http_method,
            path: route.path.clone(),
            handler: route.method_name.clone(),
        }
    }

    /// Stable 64-bit fingerprint of all identity fields.
    pub fn fingerprint(&self) -> u64 {
        let mut hash = Fnv::new();
        hash.field(&self.controller.fingerprint().to_be_bytes());
        hash.field(self.method.as_str().as_bytes());
        match &self.path {
            RoutePath::Static(path) => {
                hash.field(b"s");
                hash.field(path.as_bytes());
            }
            RoutePath::Unresolved => hash.field(b"u"),
        }
        hash.field(self.handler.as_bytes());
        hash.finish()
    }
}

/// Path with `/` separators on every platform, so the fingerprint does not
/// depend on the operating system.
fn path_bytes(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// FNV-1a, 64 bit. Fields are length-prefixed so `("ab","c")` and `("a","bc")`
/// differ.
struct Fnv(u64);

impl Fnv {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    fn new() -> Self {
        Self(Self::OFFSET)
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }

    fn field(&mut self, bytes: &[u8]) {
        self.bytes(&(bytes.len() as u64).to_be_bytes());
        self.bytes(bytes);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(controller: &str, file: &str, path: &str, handler: &str) -> RouteIdentity {
        RouteIdentity {
            controller: ControllerIdentity {
                name: controller.to_owned(),
                file: PathBuf::from(file),
            },
            method: HttpMethod::Get,
            path: RoutePath::Static(path.to_owned()),
            handler: handler.to_owned(),
        }
    }

    #[test]
    fn fnv_matches_the_published_test_vectors() {
        // Reference values of FNV-1a 64 (the algorithm's own test suite).
        let raw = |text: &str| {
            let mut hash = Fnv::new();
            hash.bytes(text.as_bytes());
            hash.finish()
        };
        assert_eq!(raw(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(raw("a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(raw("foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn the_fingerprint_is_pinned() {
        // Cross-checked against an independent implementation of the documented
        // algorithm. A change here changes every generated disambiguation
        // suffix and must be a conscious decision.
        let id = identity(
            "UsersController",
            "src/users/users.controller.ts",
            "/users/:id",
            "findOne",
        );
        assert_eq!(id.controller.fingerprint(), 0x9c4f_8822_e47c_d02a);
        assert_eq!(id.fingerprint(), 0xd544_2d27_afb7_7b7a);
    }

    #[test]
    fn every_field_changes_the_fingerprint() {
        let base = identity("C", "a.ts", "/x", "h");
        let mut seen = vec![base.fingerprint()];
        let variants = [
            identity("D", "a.ts", "/x", "h"),
            identity("C", "b.ts", "/x", "h"),
            identity("C", "a.ts", "/y", "h"),
            identity("C", "a.ts", "/x", "i"),
            RouteIdentity {
                method: HttpMethod::Post,
                ..base.clone()
            },
            RouteIdentity {
                path: RoutePath::Unresolved,
                ..base.clone()
            },
        ];
        for variant in variants {
            let fingerprint = variant.fingerprint();
            assert!(!seen.contains(&fingerprint), "{variant:?}");
            seen.push(fingerprint);
        }
    }

    #[test]
    fn field_boundaries_matter() {
        assert_ne!(
            identity("ab", "c.ts", "/x", "h").fingerprint(),
            identity("a", "bc.ts", "/x", "h").fingerprint()
        );
    }

    #[test]
    fn param_names_distinguish_otherwise_equal_routes() {
        assert_ne!(
            identity("C", "a.ts", "/users/:id", "h").fingerprint(),
            identity("C", "a.ts", "/users/:userId", "h").fingerprint()
        );
    }

    #[test]
    fn path_separators_do_not_affect_the_fingerprint() {
        let unix = ControllerIdentity {
            name: "C".into(),
            file: PathBuf::from("src/a/b.ts"),
        };
        let windows = ControllerIdentity {
            name: "C".into(),
            file: PathBuf::from("src\\a\\b.ts"),
        };
        assert_eq!(unix.fingerprint(), windows.fingerprint());
    }
}
