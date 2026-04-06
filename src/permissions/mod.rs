use std::collections::HashSet;

/// Decision returned after asking the user whether to allow a bash command.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// Run this once; ask again next time.
    AllowOnce,
    /// Run and remember for the rest of the session.
    AllowSession,
    /// Reject; send an error back to the LLM.
    Deny,
}

/// In-memory permission store for the current session.
///
/// Commands granted with `AllowSession` are stored here; they are never written
/// to disk (workspace / always scopes are future work).
pub struct PermissionStore {
    session_grants: HashSet<String>,
}

impl PermissionStore {
    pub fn new() -> Self {
        Self { session_grants: HashSet::new() }
    }

    /// True if the exact command string was previously granted for the session.
    pub fn is_session_granted(&self, command: &str) -> bool {
        self.session_grants.contains(command)
    }

    /// Grant a command for the remainder of the session.
    pub fn grant_session(&mut self, command: &str) {
        self.session_grants.insert(command.to_string());
    }

    /// Parse a single character / word from user input into a `Decision`.
    ///
    /// Accepted values (case-insensitive):
    /// - `y`, `yes`, `1`       → AllowOnce
    /// - `s`, `session`        → AllowSession
    /// - anything else (incl. empty, `n`, `no`) → Deny
    pub fn parse_input(s: &str) -> Decision {
        match s.trim().to_lowercase().as_str() {
            "y" | "yes" | "1" => Decision::AllowOnce,
            "s" | "session" => Decision::AllowSession,
            _ => Decision::Deny,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_grant_and_lookup() {
        let mut store = PermissionStore::new();
        assert!(!store.is_session_granted("cargo test"));
        store.grant_session("cargo test");
        assert!(store.is_session_granted("cargo test"));
        // Different command is not granted
        assert!(!store.is_session_granted("cargo build"));
    }

    #[test]
    fn test_parse_input_allow_once() {
        assert_eq!(PermissionStore::parse_input("y"), Decision::AllowOnce);
        assert_eq!(PermissionStore::parse_input("Y"), Decision::AllowOnce);
        assert_eq!(PermissionStore::parse_input("yes"), Decision::AllowOnce);
        assert_eq!(PermissionStore::parse_input("1"), Decision::AllowOnce);
    }

    #[test]
    fn test_parse_input_session() {
        assert_eq!(PermissionStore::parse_input("s"), Decision::AllowSession);
        assert_eq!(PermissionStore::parse_input("S"), Decision::AllowSession);
        assert_eq!(PermissionStore::parse_input("session"), Decision::AllowSession);
    }

    #[test]
    fn test_parse_input_deny() {
        assert_eq!(PermissionStore::parse_input("n"), Decision::Deny);
        assert_eq!(PermissionStore::parse_input("no"), Decision::Deny);
        assert_eq!(PermissionStore::parse_input(""), Decision::Deny);
        assert_eq!(PermissionStore::parse_input("anything"), Decision::Deny);
    }
}
