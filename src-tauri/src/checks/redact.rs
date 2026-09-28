//! Replaces the person's home folder and username with `~` in everything sent.

use serde_json::Value;

#[derive(Debug, Clone, Default)]
pub struct Redactor {
    homes: Vec<String>,
    username: Option<String>,
    case_insensitive: bool,
}

impl Redactor {
    pub fn new(home: Option<&str>, username: Option<&str>, case_insensitive: bool) -> Self {
        let mut homes = Vec::new();
        if let Some(home) = home
            .map(|h| h.trim_end_matches(['/', '\\']))
            .filter(|h| h.len() > 1)
        {
            homes.push(home.to_string());
            let flipped = home.replace('\\', "/");
            if flipped != home {
                homes.push(flipped);
            }
        }
        homes.sort_by_key(|h| std::cmp::Reverse(h.len()));
        // A very short username would match inside ordinary words.
        let username = username
            .map(str::trim)
            .filter(|u| u.chars().count() >= 3)
            .map(str::to_string);
        Redactor {
            homes,
            username,
            case_insensitive,
        }
    }

    pub fn for_this_computer() -> Self {
        #[allow(deprecated)]
        let home = std::env::home_dir().map(|p| p.to_string_lossy().into_owned());
        let username = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .ok();
        Redactor::new(home.as_deref(), username.as_deref(), cfg!(windows))
    }

    pub fn text(&self, input: &str) -> String {
        let mut out = input.to_string();
        for home in &self.homes {
            out = replace_all(&out, home, self.case_insensitive, false);
        }
        if let Some(user) = &self.username {
            out = replace_all(&out, user, true, true);
        }
        out
    }

    pub fn value(&self, value: &mut Value) {
        match value {
            Value::String(s) => *s = self.text(s),
            Value::Array(items) => items.iter_mut().for_each(|v| self.value(v)),
            Value::Object(map) => map.values_mut().for_each(|v| self.value(v)),
            _ => {}
        }
    }
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

/// Replaces `needle` with `~`. With `whole_word`, only where it is not part of a longer word.
fn replace_all(haystack: &str, needle: &str, case_insensitive: bool, whole_word: bool) -> String {
    if needle.is_empty() {
        return haystack.to_string();
    }
    let (hay_cmp, needle_cmp) = if case_insensitive {
        (haystack.to_ascii_lowercase(), needle.to_ascii_lowercase())
    } else {
        (haystack.to_string(), needle.to_string())
    };
    let bytes = hay_cmp.as_bytes();
    let mut out = String::with_capacity(haystack.len());
    let mut last = 0;
    let mut from = 0;
    while let Some(pos) = hay_cmp[from..].find(&needle_cmp) {
        let start = from + pos;
        let end = start + needle_cmp.len();
        let bounded = !whole_word
            || ((start == 0 || !is_word_byte(bytes[start - 1]))
                && (end == bytes.len() || !is_word_byte(bytes[end])));
        if bounded {
            out.push_str(&haystack[last..start]);
            out.push('~');
            last = end;
        }
        from = end;
    }
    out.push_str(&haystack[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn replaces_mac_home_and_username() {
        let r = Redactor::new(Some("/Users/pat"), Some("pat"), false);
        assert_eq!(
            r.text("/Users/pat/Library/LaunchAgents/x.plist"),
            "~/Library/LaunchAgents/x.plist"
        );
        assert_eq!(r.text("Owned by pat."), "Owned by ~.");
        assert_eq!(r.text("Pat's patch for spatula"), "~'s patch for spatula");
    }

    #[test]
    fn replaces_windows_home_in_either_slash_and_case() {
        let r = Redactor::new(Some("C:\\Users\\Morgan"), Some("Morgan"), true);
        assert_eq!(
            r.text("c:\\users\\morgan\\AppData\\Roaming\\x.exe"),
            "~\\AppData\\Roaming\\x.exe"
        );
        assert_eq!(r.text("C:/Users/Morgan/Desktop"), "~/Desktop");
        assert_eq!(r.text("HKEY_USERS\\morgan\\Run"), "HKEY_USERS\\~\\Run");
    }

    #[test]
    fn ignores_very_short_usernames() {
        let r = Redactor::new(Some("/Users/al"), Some("al"), false);
        assert_eq!(
            r.text("Alerts all around /Users/al/x"),
            "Alerts all around ~/x"
        );
    }

    #[test]
    fn walks_json_values() {
        let r = Redactor::new(Some("/Users/pat"), Some("pat"), false);
        let mut value = json!({ "items": [{ "label": "/Users/pat/Applications/Foo.app", "n": 3 }], "who": "pat" });
        r.value(&mut value);
        assert_eq!(
            value,
            json!({ "items": [{ "label": "~/Applications/Foo.app", "n": 3 }], "who": "~" })
        );
    }
}
