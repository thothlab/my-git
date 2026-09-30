/**
 * Pure rules of the "Signature" line in the commit card: how alarming an answer
 * is, and which explanation fits it. The words are `i18n`'s; checking the
 * signature is the backend's (`engine::signature`).
 *
 * Imports nothing, on purpose: `scripts/check-log-filters.mjs` transpiles it
 * rather than bundling it, and one import would fail module resolution there.
 */

/** Mirrors `api.SignatureStatus`, restated so this module stays import-free. */
export type SigStatus =
  | "unsigned"
  | "verified"
  | "unknown-key"
  | "missing-key"
  | "expired"
  | "expired-key"
  | "revoked"
  | "bad"
  | "unchecked";

export type SigFormat = "openpgp" | "ssh" | "x509" | "unknown";

/** `good` → success colour, `bad` → danger, `warn` → warn, `none` → muted. */
export type SigTone = "good" | "warn" | "bad" | "none";

export function signatureTone(status: SigStatus): SigTone {
  switch (status) {
    case "verified":
      return "good";
    case "bad":
    case "revoked":
      return "bad";
    case "unsigned":
      return "none";
    default:
      return "warn";
  }
}

/**
 * Which explanation goes under the verdict. SSH and OpenPGP fail for different
 * reasons with the same letter: an SSH key is "unknown" when the allowed signers
 * file does not list it, an OpenPGP key when the keyring does not trust it; an
 * SSH signature is left unchecked when no allowed signers file is configured, an
 * OpenPGP one when `gpg` is not there to run.
 */
export type SigHint =
  | "none"
  | "ssh-not-listed"
  | "untrusted"
  | "ssh-no-signers-file"
  | "no-gpg"
  | "no-gpgsm"
  | "unknown-format"
  | "missing-key";

export function signatureHint(status: SigStatus, format: SigFormat | null): SigHint {
  switch (status) {
    case "unknown-key":
      return format === "ssh" ? "ssh-not-listed" : "untrusted";
    case "missing-key":
      return "missing-key";
    case "unchecked":
      return format === "ssh"
        ? "ssh-no-signers-file"
        : format === "openpgp"
          ? "no-gpg"
          : format === "x509"
            ? "no-gpgsm"
            : "unknown-format";
    default:
      return "none";
  }
}
