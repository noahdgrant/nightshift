//! Redaction of secret-like values in a phase transcript before `ns log --phase-output` shows it.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

const REDACTED: &str = "[redacted]";

/// Keys whose value is a credential, wherever they sit in a transcript line: any key containing
/// a credential word, or ending in `token` or a `token_value`-style suffix, so `GH_TOKEN` and
/// `JWT_SECRET_KEY` but not `input_tokens`.
static SECRET_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:secret|password|passwd|passphrase|(?:private|api|access|signing|encryption)[_-]?key|credentials?|authorization|token(?:[_-]?(?:value|string|hash|data))?$)").unwrap()
});

static AUTH_SCHEME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(bearer|basic)\s+([A-Za-z0-9._~+/=-]{8,})").unwrap());

fn credential_like(word: &str) -> bool {
    word.len() >= 20 || word.contains(|c: char| c.is_ascii_digit() || "+/=_~".contains(c))
}

/// Secret-like text and what replaces it, applied in order.
static SECRET_TEXT: LazyLock<Vec<(Regex, &str)>> = LazyLock::new(|| {
    [
        (
            r"-----BEGIN [A-Z ]*PRIVATE KEY-----(?s:.*?)(?:-----END [A-Z ]*PRIVATE KEY-----|$)",
            "[redacted private key]",
        ),
        (r"(://)[^/\s:@]+:[^/\s@]+@", "${1}[redacted]@"),
        (r"(?i)\b(authorization\s*:\s*token\s+)\S+", "${1}[redacted]"),
        (
            r#"(?i)\b(authorization\s*:\s*(?:bearer|basic)\s+)[^\s'"]+"#,
            "${1}[redacted]",
        ),
        (
            r"(?i)(\s--[a-z-]*(?:password|passwd|passphrase|secret|token|api-?key|access-?key|private-?key|signing-?key|encryption-?key|credentials?)\s+)\S+",
            "${1}[redacted]",
        ),
        (
            r"(\s(?:-u|--user|--proxy-user)[\s=]+)[^\s:]+:\S+",
            "${1}[redacted]",
        ),
        (
            r"(\b(?:docker\s+login|sshpass)\b[^\n|;&]*?\s-p\s*)\S+",
            "${1}[redacted]",
        ),
        (
            r"(\b(?:mysql|mysqldump|mysqladmin|mariadb)\b[^\n|;&]*?\s-p)\S+",
            "${1}[redacted]",
        ),
        (
            r"\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|glpat-[A-Za-z0-9_-]{20,}|sk-[A-Za-z0-9_-]{20,}|xox[abprs]-[A-Za-z0-9-]{10,}|AKIA[0-9A-Z]{16}|ASIA[0-9A-Z]{16}|(?:sk|rk)_live_[A-Za-z0-9]{10,}|npm_[A-Za-z0-9]{20,}|hf_[A-Za-z0-9]{20,}|SG\.[A-Za-z0-9_-]{16,}\.[A-Za-z0-9_-]{16,}|ya29\.[A-Za-z0-9_-]{20,}|shp(?:at|ca|pa|ss)_[A-Za-z0-9]{20,}|pypi-[A-Za-z0-9_-]{20,}|AIza[0-9A-Za-z_-]{35}|eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,})",
            REDACTED,
        ),
        (
            r#"(?i)\b([A-Za-z0-9_.-]*(?:token|secret|password|passwd|passphrase|api[_-]?key|access[_-]?key|private[_-]?key|signing[_-]?key|encryption[_-]?key|credentials?)[A-Za-z0-9_]*"?[ \t]*[:=][ \t]*)("[^"\n]*"|'[^'\n]*'|[^\n]+)"#,
            "${1}[redacted]",
        ),
    ]
    .into_iter()
    .map(|(re, with)| (Regex::new(re).unwrap(), with))
    .collect()
});

pub(super) fn redact(v: &mut Value) {
    match v {
        Value::String(s) => {
            if AUTH_SCHEME.is_match(s) {
                *s = AUTH_SCHEME
                    .replace_all(s, |c: &regex::Captures| {
                        if credential_like(&c[2]) {
                            format!("{} {REDACTED}", &c[1])
                        } else {
                            c[0].to_string()
                        }
                    })
                    .into_owned();
            }
            for (re, with) in SECRET_TEXT.iter() {
                if re.is_match(s) {
                    *s = re.replace_all(s, *with).into_owned();
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact),
        Value::Object(fields) => {
            for (k, v) in fields.iter_mut() {
                if SECRET_KEY.is_match(k) && !v.is_null() {
                    *v = Value::String(REDACTED.into());
                } else {
                    redact(v);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unterminated_private_key_is_redacted_to_the_end() {
        let mut v = Value::String("key: -----BEGIN PRIVATE KEY-----\nMIIabc".into());
        redact(&mut v);
        assert_eq!(v, "key: [redacted private key]");
    }

    #[test]
    fn ordinary_text_and_token_counts_are_left_alone() {
        let mut v = serde_json::json!({"input_tokens": 12, "text": "ran cargo test, 3 passed",
            "auth": null, "token": null});
        let before = v.clone();
        redact(&mut v);
        assert_eq!(v, before);
    }

    #[test]
    fn every_credential_key_and_shape_is_redacted() {
        let keys = [
            "token",
            "secret",
            "password",
            "passwd",
            "passphrase",
            "api_key",
            "access-key",
            "private_key",
            "credential",
            "credentials",
            "authorization",
        ];
        for k in keys {
            let mut v = serde_json::json!({ k: "hunter2value" });
            redact(&mut v);
            assert_eq!(v[k], REDACTED, "{k}");
        }
        let texts = [
            ("Authorization: Basic dXNlcjpodW50ZXIy", "dXNlcjpodW50ZXIy"),
            ("Authorization: token hunter2value", "hunter2value"),
            (
                "key AIzaSyA1234567890abcdefghijklmnopqrstuv",
                "AIzaSyA1234567890abcdefghijklmnopqrstuv",
            ),
            (
                "gho_0123456789abcdefghijABCDEFGHIJ",
                "gho_0123456789abcdefghijABCDEFGHIJ",
            ),
            (
                "ghs_0123456789abcdefghijABCDEFGHIJ",
                "ghs_0123456789abcdefghijABCDEFGHIJ",
            ),
            (
                "ghu_0123456789abcdefghijABCDEFGHIJ",
                "ghu_0123456789abcdefghijABCDEFGHIJ",
            ),
            (
                "ghr_0123456789abcdefghijABCDEFGHIJ",
                "ghr_0123456789abcdefghijABCDEFGHIJ",
            ),
            ("mysql --password hunter2value db", "hunter2value"),
            ("tool --api-key hunter2value", "hunter2value"),
            ("mysql -phunter2value db", "hunter2value"),
            ("curl -u admin:hunter2value https://x", "hunter2value"),
            (
                "sk_live_0123456789abcdefghij",
                "sk_live_0123456789abcdefghij",
            ),
            (
                "rk_live_0123456789abcdefghij",
                "rk_live_0123456789abcdefghij",
            ),
            (
                "npm_0123456789abcdefghijABCDEFGHIJ",
                "npm_0123456789abcdefghijABCDEFGHIJ",
            ),
            ("ASIAABCDEFGHIJKLMNOP", "ASIAABCDEFGHIJKLMNOP"),
            (
                "hf_0123456789abcdefghijABCDEFGHIJ",
                "hf_0123456789abcdefghijABCDEFGHIJ",
            ),
            (
                "SG.abcdefghij0123456789.abcdefghij0123456789",
                "SG.abcdefghij0123456789.abcdefghij0123456789",
            ),
            (
                "ya29.a0AfH6SMBabcdefghij0123456789",
                "ya29.a0AfH6SMBabcdefghij0123456789",
            ),
            (
                concat!("shp", "at_0123456789abcdef0123456789abcdef"),
                concat!("shp", "at_0123456789abcdef0123456789abcdef"),
            ),
            (
                "pypi-AgEIcHlwaS5vcmcabcdefghij0123456789",
                "pypi-AgEIcHlwaS5vcmcabcdefghij0123456789",
            ),
            ("passphrase: hunter2value", "hunter2value"),
            (r#"{"token":"hunter2value"}"#, "hunter2value"),
            ("{\n\"token\": \"hunter2value\"\n}", "hunter2value"),
            ("password = correct horse battery staple", "staple"),
            ("PASSWORD=correct horse && run", "horse"),
            ("export SIGNING_KEY=abc123", "abc123"),
            ("ENCRYPTION_KEY=abc123", "abc123"),
            ("run --signing-key abc123 now", "abc123"),
        ];
        for (text, secret) in texts {
            let mut v = Value::String(text.into());
            redact(&mut v);
            let out = v.as_str().unwrap();
            assert!(!out.contains(secret), "{text} -> {out}");
        }
    }

    #[test]
    fn credential_words_anywhere_in_a_nested_key_redact_its_value() {
        for k in [
            "SECRET_KEY",
            "JWT_SECRET_KEY",
            "signing_key",
            "signingKey",
            "encryption-key",
            "passwordHash",
            "token_value",
        ] {
            let mut v = serde_json::json!({"env": {"inner": {k: "hunter2value"}}});
            redact(&mut v);
            assert_eq!(v["env"]["inner"][k], REDACTED, "{k}");
        }
        let mut v = serde_json::json!({"usage": {"input_tokens": 1, "max_tokens": 2,
            "total_tokens": 3, "monkey": "x"}});
        let before = v.clone();
        redact(&mut v);
        assert_eq!(v, before);
    }

    #[test]
    fn bare_bearer_and_basic_credentials_are_redacted_by_shape() {
        for (text, secret) in [
            ("Bearer abcdefghijklmnopqrst", "abcdefghijklmnopqrst"),
            ("Basic abc+defghij", "abc+defghij"),
            ("Basic abc/defghij", "abc/defghij"),
            ("Basic abcdefghij==", "abcdefghij=="),
            ("Bearer abc_defghij", "abc_defghij"),
            ("Bearer abc~defghij", "abc~defghij"),
            ("Bearer abcdefg1234", "abcdefg1234"),
        ] {
            let mut v = Value::String(text.into());
            redact(&mut v);
            assert!(!v.as_str().unwrap().contains(secret), "{text}");
        }
        for prose in [
            "Bearer abcdefg",
            "Bearer abc1234",
            "Bearer abcdefgh",
            "Basic abcdefghijklmnopqrs",
        ] {
            let mut v = Value::String(prose.into());
            redact(&mut v);
            assert_eq!(v, prose);
        }
    }

    #[test]
    fn a_flag_and_an_empty_assignment_keep_the_rest_of_the_command() {
        let mut v = Value::String("mkdir -p /tmp/x\nTOKEN=\nnext line".into());
        redact(&mut v);
        assert_eq!(v, "mkdir -p /tmp/x\nTOKEN=\nnext line");
    }

    #[test]
    fn bearer_and_basic_are_redacted_only_when_followed_by_a_credential() {
        for prose in [
            "a basic implementation of Bearer authentication",
            "the basic implementation.",
            "use basic configuration",
        ] {
            let mut v = Value::String(prose.into());
            redact(&mut v);
            assert_eq!(v, prose);
        }
        for (text, secret) in [
            (
                "Authorization: Bearer abcdefghijklmnopqrst",
                "abcdefghijklmnopqrst",
            ),
            ("Authorization: Basic dXNlcjpodW50ZXIy", "dXNlcjpodW50ZXIy"),
            ("Bearer opaquebearer42", "opaquebearer42"),
            ("Authorization: Basic dXNlcjpwYXNz", "dXNlcjpwYXNz"),
            ("Authorization: Bearer abcdefghij", "abcdefghij"),
            ("curl -H 'Authorization: Basic abc' x", "abc"),
        ] {
            let mut v = Value::String(text.into());
            redact(&mut v);
            assert!(!v.as_str().unwrap().contains(secret), "{text}");
        }
    }

    #[test]
    fn p_is_redacted_only_where_it_takes_a_password() {
        for cmd in [
            "find . -print0",
            "ls -pa dir",
            "docker run -p8080:80 img",
            "docker run -p 8080:80 img",
            "mkdir -p /tmp/x",
            "mysql -p db",
        ] {
            let mut v = Value::String(cmd.into());
            redact(&mut v);
            assert_eq!(v, cmd);
        }
        for (cmd, secret) in [
            ("docker login -u me -p hunter2value reg", "hunter2value"),
            ("sshpass -p hunter2value ssh host", "hunter2value"),
            ("mysql -u root -phunter2value db", "hunter2value"),
            ("curl --user admin:hunter2value https://x", "hunter2value"),
            (
                "curl --proxy-user admin:hunter2value https://x",
                "hunter2value",
            ),
            ("curl -u admin:hunter2value https://x", "hunter2value"),
        ] {
            let mut v = Value::String(cmd.into());
            redact(&mut v);
            assert!(!v.as_str().unwrap().contains(secret), "{cmd}");
        }
    }
}
