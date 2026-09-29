use serde::{Serialize, Serializer};

use crate::engine::exec::mask_credentials;

/// Application error.
///
/// The `Git` variant *requires* `stderr` — there is no way to construct a git
/// failure without carrying its underlying output. This is deliberate: Правка
/// `e83ccb7` in the TUI was exactly the bug of collapsing a git failure into a
/// generic literal (`Err(_) => "rebase failed"`), which hid the real reason from
/// the user. Here every failed shell-out surfaces its stderr all the way to the UI.
///
/// What reaches the screen is masked: a remote URL with credentials in it
/// (`https://user:token@host/…`) is shown as `https://***@host/…` in both `Display`
/// and the serialized form — see `engine::exec::mask_credentials`. The fields
/// themselves stay verbatim for code that inspects them.
#[derive(Debug)]
pub enum Error {
    /// `journal` is the id of the journal entry of the failed run, so the UI can
    /// open its whole output; `None` for a failure that was never journaled.
    Git {
        command: String,
        stderr: String,
        journal: Option<u64>,
    },
    Io(String),
    Parse(String),
    /// A domain rule was violated (duplicate list name, deleting Default, ...).
    Rule(String),
    /// The file on disk no longer matches what the application last read or wrote
    /// there, so a write was refused rather than overwriting someone else's change.
    ///
    /// Its own variant rather than a `Rule` with a recognisable opening: the client
    /// **branches** here — it offers a choice between rereading and overwriting — and
    /// matching that branch against prose breaks on the first rewording, translation
    /// included. The discriminator is the serialized `kind`, see below.
    Stale(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Git { command, stderr, .. } => write!(
                f,
                "git {} failed: {}",
                mask_credentials(command),
                mask_credentials(stderr)
            ),
            Error::Io(m) => write!(f, "io error: {m}"),
            Error::Parse(m) => write!(f, "parse error: {m}"),
            Error::Rule(m) => write!(f, "{m}"),
            Error::Stale(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}

/// Serialize as `{ kind, message, stderr, journalId }` so the SolidJS layer can always
/// show a reason and, for git errors, git's own output (credentials masked) and a link
/// to the journal entry.
impl Serialize for Error {
    // Fully-qualified Result: the crate's `Result<T>` alias (below) shadows the std one.
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let (kind, message, stderr, journal) = match self {
            Error::Git {
                command,
                stderr,
                journal,
            } => (
                "git",
                format!("git {} failed", mask_credentials(command)),
                Some(mask_credentials(stderr).into_owned()),
                *journal,
            ),
            Error::Io(m) => ("io", m.clone(), None, None),
            Error::Parse(m) => ("parse", m.clone(), None, None),
            Error::Rule(m) => ("rule", m.clone(), None, None),
            Error::Stale(m) => ("stale", m.clone(), None, None),
        };
        let mut st = s.serialize_struct("Error", 4)?;
        st.serialize_field("kind", kind)?;
        st.serialize_field("message", &message)?;
        st.serialize_field("stderr", &stderr)?;
        st.serialize_field("journalId", &journal)?;
        st.end()
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_error_masks_credentials_on_the_way_out() {
        let e = Error::Git {
            command: "push https://u:tok@host/r.git".into(),
            stderr: "fatal: unable to access 'https://u:tok@host/r.git/'".into(),
            journal: Some(7),
        };
        let shown = e.to_string();
        assert!(!shown.contains("tok"), "{shown}");
        let json = serde_json::to_value(&e).unwrap();
        let text = json.to_string();
        assert!(!text.contains("tok"), "{text}");
        assert!(text.contains("https://***@host/r.git"), "{text}");
        assert_eq!(json["journalId"], 7);
        assert_eq!(json["kind"], "git");
    }
}
