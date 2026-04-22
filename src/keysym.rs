use crate::error::{Result, WdoError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyChain {
    pub modifiers: Vec<&'static str>,
    pub key: String,
}

pub fn parse_chain(input: &str) -> Result<KeyChain> {
    if input.is_empty() {
        return Err(WdoError::Keysym {
            input: input.into(),
            reason: "empty input".into(),
        });
    }

    let parts: Vec<&str> = input
        .split('+')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    if parts.is_empty() {
        return Err(WdoError::Keysym {
            input: input.into(),
            reason: "no tokens after splitting on '+'".into(),
        });
    }

    let (key, mods) = parts.split_last().unwrap();

    let modifiers: Result<Vec<&'static str>> = mods
        .iter()
        .map(|m| {
            normalize_modifier(m).ok_or_else(|| WdoError::Keysym {
                input: input.into(),
                reason: format!("unknown modifier '{m}'"),
            })
        })
        .collect();

    Ok(KeyChain {
        modifiers: modifiers?,
        key: (*key).to_string(),
    })
}

/// Map common xdotool-style modifier aliases to canonical keysym names.
/// Returns None for unknown tokens so the parser can produce a targeted error.
fn normalize_modifier(m: &str) -> Option<&'static str> {
    match m.to_ascii_lowercase().as_str() {
        "ctrl" | "control" | "control_l" => Some("Control_L"),
        "control_r" => Some("Control_R"),
        "alt" | "meta" | "alt_l" => Some("Alt_L"),
        "alt_r" => Some("Alt_R"),
        "shift" | "shift_l" => Some("Shift_L"),
        "shift_r" => Some("Shift_R"),
        "super" | "win" | "windows" | "logo" | "mod4" | "super_l" => Some("Super_L"),
        "super_r" => Some("Super_R"),
        "altgr" | "mod5" | "iso_level3_shift" => Some("ISO_Level3_Shift"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_chain() {
        let c = parse_chain("ctrl+shift+a").unwrap();
        assert_eq!(c.modifiers, vec!["Control_L", "Shift_L"]);
        assert_eq!(c.key, "a");
    }

    #[test]
    fn parses_bare_key() {
        let c = parse_chain("Return").unwrap();
        assert!(c.modifiers.is_empty());
        assert_eq!(c.key, "Return");
    }

    #[test]
    fn super_aliases() {
        for alias in ["super", "Win", "logo", "mod4"] {
            let c = parse_chain(&format!("{alias}+l")).unwrap();
            assert_eq!(c.modifiers, vec!["Super_L"]);
            assert_eq!(c.key, "l");
        }
    }

    #[test]
    fn rejects_empty() {
        assert!(parse_chain("").is_err());
        assert!(parse_chain("++").is_err());
    }

    #[test]
    fn rejects_unknown_modifier() {
        let err = parse_chain("hyper+a").unwrap_err();
        assert!(matches!(err, WdoError::Keysym { .. }));
    }
}
