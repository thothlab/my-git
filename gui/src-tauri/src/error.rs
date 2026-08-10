use serde::{Serialize, Serializer};

/// Application error.
///
/// The `Git` variant *requires* `stderr` — there is no way to construct a git
/// failure without carrying its underlying output. This is deliberate: Правка
/// `e83ccb7` in the TUI was exactly the bug of collapsing a git failure into a
/// generic literal (`Err(_) => "rebase failed"`), which hid the real reason from
/// the user. Here every failed shell-out surfaces its stderr all the way to the UI.
#[derive(Debug)]
pub enum Error {
    Git { command: String, stderr: String },
    Io(String),
    Parse(String),
    /// A domain rule was violated (duplicate list name, deleting Default, ...).
    Rule(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Git { command, stderr } => write!(f, "git {command} failed: {stderr}"),
            Error::Io(m) => write!(f, "io error: {m}"),
            Error::Parse(m) => write!(f, "parse error: {m}"),
            Error::Rule(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}

/// Serialize as `{ kind, message, stderr }` so the SolidJS layer can always show a
/// reason and, for git errors, the raw stderr.
impl Serialize for Error {
    // Fully-qualified Result: the crate's `Result<T>` alias (below) shadows the std one.
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let (kind, message, stderr) = match self {
            Error::Git { command, stderr } => {
                ("git", format!("git {command} failed"), Some(stderr.clone()))
            }
            Error::Io(m) => ("io", m.clone(), None),
            Error::Parse(m) => ("parse", m.clone(), None),
            Error::Rule(m) => ("rule", m.clone(), None),
        };
        let mut st = s.serialize_struct("Error", 3)?;
        st.serialize_field("kind", kind)?;
        st.serialize_field("message", &message)?;
        st.serialize_field("stderr", &stderr)?;
        st.end()
    }
}

pub type Result<T> = std::result::Result<T, Error>;
