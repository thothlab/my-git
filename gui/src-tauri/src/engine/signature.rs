//! Whether a commit is signed, and what checking the signature gave.
//!
//! Two reads, because git's own answer is not enough on its own. `%G?` says `N`
//! ("no signature") both for an unsigned commit **and** for a signed one git could
//! not check: an SSH signature with no `gpg.ssh.allowedSignersFile` configured, an
//! OpenPGP one whose `gpg` is missing (`cannot exec`, exit 0 all the same). So
//! whether a signature is *there* is read from the commit object's own `gpgsig`
//! header, and `%G?` only says what verifying it gave.
//!
//! The kind (OpenPGP / SSH / X.509) is read from the signature itself — its armour
//! line — not from `gpg.format`: that setting chooses how *this* user signs, while
//! git picks the verifier for a commit by the signature's own prefix.
//!
//! ## Cost and prompts
//!
//! Verifying runs `gpg`, `ssh-keygen -Y verify` or `gpgsm` through git, one process
//! per commit, so it is asked for the one commit open in the details pane, never
//! per log row. Verification needs only public keys: no secret key is touched, so no
//! pinentry is asked for, and stdin is closed ([`exec::git`]) in any case. What the
//! user's own `gpg.conf` does (`auto-key-retrieve` reaching a keyserver) is theirs.
//! An unsigned commit costs one `cat-file` and no verifier at all.
//!
//! A verifier that is missing or fails is not an error: the answer is
//! [`SignatureStatus::Unchecked`], "could not check".

use std::path::Path;

use crate::engine::exec;
use crate::error::{Error, Result};
use crate::model::{CommitSignature, SignatureFormat, SignatureStatus};

/// `%G?`, signer, key, fingerprint — NUL-separated, one record.
const VERIFY_FORMAT: &str = "--format=%G?%x00%GS%x00%GK%x00%GF";

/// The signature kind named by a raw commit object's headers, or `None` when it
/// carries none. Headers end at the first empty line: a message that quotes a
/// signature block is not signed by it.
pub fn signature_format(raw: &str) -> Option<SignatureFormat> {
    let headers = raw.split("\n\n").next().unwrap_or("");
    let first = headers.lines().find_map(|l| {
        l.strip_prefix("gpgsig ")
            .or_else(|| l.strip_prefix("gpgsig-sha256 "))
    })?;
    Some(match first.trim() {
        "-----BEGIN PGP SIGNATURE-----" | "-----BEGIN PGP MESSAGE-----" => SignatureFormat::Openpgp,
        "-----BEGIN SSH SIGNATURE-----" => SignatureFormat::Ssh,
        "-----BEGIN SIGNED MESSAGE-----" => SignatureFormat::X509,
        _ => SignatureFormat::Unknown,
    })
}

/// Parse the `VERIFY_FORMAT` record of a **signed** commit. A letter git does not
/// document is `Error::Parse`, not a guess.
pub fn parse_verification(out: &str, format: SignatureFormat) -> Result<CommitSignature> {
    let out = out.strip_suffix('\n').unwrap_or(out);
    let f: Vec<&str> = out.split('\0').collect();
    if f.len() != 4 {
        return Err(Error::Parse(format!(
            "signature: expected 4 fields, got {}",
            f.len()
        )));
    }
    let status = match f[0] {
        "G" => SignatureStatus::Verified,
        "U" => SignatureStatus::UnknownKey,
        "E" => SignatureStatus::MissingKey,
        "X" => SignatureStatus::Expired,
        "Y" => SignatureStatus::ExpiredKey,
        "R" => SignatureStatus::Revoked,
        "B" => SignatureStatus::Bad,
        // There is a signature (the header says so) and git checked nothing.
        "N" => SignatureStatus::Unchecked,
        other => {
            return Err(Error::Parse(format!(
                "signature: unknown %G? answer {other:?}"
            )))
        }
    };
    let field = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
    Ok(CommitSignature {
        status,
        format: Some(format),
        signer: field(f[1]),
        key: field(f[2]),
        fingerprint: field(f[3]),
    })
}

/// The signature of commit `rev`.
pub fn commit_signature(repo: &Path, rev: &str) -> Result<CommitSignature> {
    let raw = exec::git(repo, &["cat-file", "commit", "--end-of-options", rev])
        .run()?
        .checked()?;
    let Some(format) = signature_format(&String::from_utf8_lossy(&raw)) else {
        return Ok(CommitSignature {
            status: SignatureStatus::Unsigned,
            format: None,
            signer: None,
            key: None,
            fingerprint: None,
        });
    };
    // `--no-show-signature`: `log.showSignature` would print the verifier's own
    // report into stdout ahead of the record.
    let out = exec::git(
        repo,
        &[
            "log",
            "-1",
            "--no-show-signature",
            VERIFY_FORMAT,
            "--end-of-options",
            rev,
            "--",
        ],
    )
    .run()?;
    if !out.success() {
        return Ok(CommitSignature {
            status: SignatureStatus::Unchecked,
            format: Some(format),
            signer: None,
            key: None,
            fingerprint: None,
        });
    }
    parse_verification(&out.stdout_text(), format)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::cli::tests::{run_git, scratch_repo};
    use std::process::Command;

    fn git_out(p: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(p)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn head(p: &Path) -> String {
        git_out(p, &["rev-parse", "HEAD"])
    }

    // ---- parsing ----

    #[test]
    fn the_signature_kind_comes_from_the_header_not_the_message() {
        let ssh = "tree t\nauthor a\ngpgsig -----BEGIN SSH SIGNATURE-----\n U1NI\n -----END SSH SIGNATURE-----\n\nmsg\n";
        assert_eq!(signature_format(ssh), Some(SignatureFormat::Ssh));
        let pgp = "tree t\ngpgsig -----BEGIN PGP SIGNATURE-----\n x\n\nmsg\n";
        assert_eq!(signature_format(pgp), Some(SignatureFormat::Openpgp));
        let x509 = "tree t\ngpgsig-sha256 -----BEGIN SIGNED MESSAGE-----\n x\n\nmsg\n";
        assert_eq!(signature_format(x509), Some(SignatureFormat::X509));
        let odd = "tree t\ngpgsig something else\n\nmsg\n";
        assert_eq!(signature_format(odd), Some(SignatureFormat::Unknown));
        let quoted = "tree t\nauthor a\n\nmsg\ngpgsig -----BEGIN SSH SIGNATURE-----\n";
        assert_eq!(signature_format(quoted), None);
    }

    #[test]
    fn every_documented_letter_parses_and_an_unknown_one_is_an_error() {
        let pairs = [
            ("G", SignatureStatus::Verified),
            ("U", SignatureStatus::UnknownKey),
            ("E", SignatureStatus::MissingKey),
            ("X", SignatureStatus::Expired),
            ("Y", SignatureStatus::ExpiredKey),
            ("R", SignatureStatus::Revoked),
            ("B", SignatureStatus::Bad),
            ("N", SignatureStatus::Unchecked),
        ];
        for (letter, want) in pairs {
            let s = parse_verification(&format!("{letter}\0\0\0\n"), SignatureFormat::Ssh).unwrap();
            assert_eq!(s.status, want, "{letter}");
        }
        let s = parse_verification("G\0Jo <j@x>\0ABCD\0SHA256:xyz\n", SignatureFormat::Openpgp)
            .unwrap();
        assert_eq!(s.signer.as_deref(), Some("Jo <j@x>"));
        assert_eq!(s.key.as_deref(), Some("ABCD"));
        assert_eq!(s.fingerprint.as_deref(), Some("SHA256:xyz"));
        assert_eq!(s.format, Some(SignatureFormat::Openpgp));
        assert!(matches!(
            parse_verification("Q\0\0\0\n", SignatureFormat::Ssh),
            Err(Error::Parse(_))
        ));
        assert!(matches!(
            parse_verification("G\0only\n", SignatureFormat::Ssh),
            Err(Error::Parse(_))
        ));
    }

    // ---- real commits ----

    #[test]
    fn an_unsigned_commit_is_unsigned() {
        let dir = scratch_repo();
        let s = commit_signature(dir.path(), "HEAD").unwrap();
        assert_eq!(s.status, SignatureStatus::Unsigned);
        assert_eq!(s.format, None);
    }

    /// A repository whose HEAD is signed with a fresh SSH key; `None` when this
    /// machine has no `ssh-keygen` that signs (OpenSSH < 8.2).
    fn ssh_signed() -> Option<(tempfile::TempDir, tempfile::TempDir)> {
        let keys = tempfile::tempdir().unwrap();
        let k = keys.path();
        for name in ["k", "other"] {
            let ok = Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-C", name, "-f"])
                .arg(k.join(name))
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !ok {
                return None;
            }
        }
        let dir = scratch_repo();
        let p = dir.path();
        run_git(p, &["config", "gpg.format", "ssh"]);
        run_git(
            p,
            &["config", "user.signingkey", k.join("k").to_str().unwrap()],
        );
        let signed = Command::new("git")
            .arg("-C")
            .arg(p)
            .args(["commit", "-q", "--allow-empty", "-S", "-m", "signed"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        signed.then_some((dir, keys))
    }

    fn allow(p: &Path, keys: &Path, file: &str, key: &str) {
        let public = std::fs::read_to_string(keys.join(format!("{key}.pub"))).unwrap();
        let path = keys.join(file);
        std::fs::write(&path, format!("t@example.com namespaces=\"git\" {public}")).unwrap();
        run_git(
            p,
            &[
                "config",
                "gpg.ssh.allowedSignersFile",
                path.to_str().unwrap(),
            ],
        );
    }

    #[test]
    fn an_ssh_signature_is_verified_against_the_allowed_signers_file() {
        let Some((dir, keys)) = ssh_signed() else {
            eprintln!("skipped: ssh-keygen cannot sign here");
            return;
        };
        let p = dir.path();

        // No allowed signers file: git answers `N`, as for no signature at all —
        // the header is what tells "could not check" from "not signed".
        let s = commit_signature(p, "HEAD").unwrap();
        assert_eq!(s.status, SignatureStatus::Unchecked);
        assert_eq!(s.format, Some(SignatureFormat::Ssh));

        allow(p, keys.path(), "allowed", "k");
        let s = commit_signature(p, "HEAD").unwrap();
        assert_eq!(s.status, SignatureStatus::Verified);
        assert_eq!(s.signer.as_deref(), Some("t@example.com"));
        assert!(
            s.key.as_deref().is_some_and(|k| k.starts_with("SHA256:")),
            "{s:?}"
        );

        // The file lists another key for this signer: valid signature, unknown key.
        allow(p, keys.path(), "allowed-other", "other");
        let s = commit_signature(p, "HEAD").unwrap();
        assert_eq!(s.status, SignatureStatus::UnknownKey);
        assert_eq!(s.signer, None);
        assert!(s.key.is_some());
    }

    #[test]
    fn a_commit_changed_after_signing_has_a_bad_signature() {
        let Some((dir, keys)) = ssh_signed() else {
            eprintln!("skipped: ssh-keygen cannot sign here");
            return;
        };
        let p = dir.path();
        allow(p, keys.path(), "allowed", "k");
        let raw = git_out(p, &["cat-file", "commit", "HEAD"]);
        let forged = format!("{}\n", raw.replace("\n\nsigned", "\n\nforged"));
        let out = Command::new("git")
            .arg("-C")
            .arg(p)
            .args(["hash-object", "-t", "commit", "-w", "--stdin"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut c| {
                use std::io::Write;
                c.stdin.take().unwrap().write_all(forged.as_bytes())?;
                c.wait_with_output()
            })
            .unwrap();
        let oid = String::from_utf8_lossy(&out.stdout).trim().to_string();
        assert_ne!(oid, head(p));
        let s = commit_signature(p, &oid).unwrap();
        assert_eq!(s.status, SignatureStatus::Bad);
    }

    #[test]
    fn a_missing_verifier_is_could_not_check_not_an_error() {
        let dir = scratch_repo();
        let p = dir.path();
        // An OpenPGP-signed commit object (the signature need not be valid: the
        // verifier is never reached).
        let tree = git_out(p, &["rev-parse", "HEAD^{tree}"]);
        let object = format!(
            "tree {tree}\nauthor T <t@example.com> 1 +0000\ncommitter T <t@example.com> 1 +0000\n\
             gpgsig -----BEGIN PGP SIGNATURE-----\n \n iQEz\n -----END PGP SIGNATURE-----\n\npgp\n"
        );
        let out = Command::new("git")
            .arg("-C")
            .arg(p)
            .args(["hash-object", "-t", "commit", "-w", "--stdin"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut c| {
                use std::io::Write;
                c.stdin.take().unwrap().write_all(object.as_bytes())?;
                c.wait_with_output()
            })
            .unwrap();
        let oid = String::from_utf8_lossy(&out.stdout).trim().to_string();
        run_git(p, &["config", "gpg.program", "/nonexistent/gpg"]);
        let s = commit_signature(p, &oid).unwrap();
        assert_eq!(s.status, SignatureStatus::Unchecked);
        assert_eq!(s.format, Some(SignatureFormat::Openpgp));
    }

    #[test]
    fn an_unknown_revision_is_a_git_error() {
        let dir = scratch_repo();
        assert!(matches!(
            commit_signature(dir.path(), "no-such-rev"),
            Err(Error::Git { .. })
        ));
    }
}
