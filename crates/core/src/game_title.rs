//! Parser do título da janela do cliente MU Online (inglês e português)
//! e parser de linhas de chat (`[PK]` e `[PickItem]`).

use crate::sanitize_text;

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TitleInfo {
    pub name: Option<String>,
    pub level: Option<u32>,
    pub resets: Option<u32>,
    pub wc: Option<u64>,
    pub wp: Option<u64>,
    /// Goblin Points em décimos (`15,840.0` -> `158400`).
    pub gp_tenths: Option<u64>,
}

impl TitleInfo {
    pub fn is_logged_in(&self) -> bool {
        self.name.is_some() && self.level.is_some()
    }

    pub fn parse(title: &str) -> Self {
        let mut info = TitleInfo::default();
        let bounded: String = title.chars().take(512).collect();
        for part in bounded.split("||") {
            let Some((k, v)) = part.split_once(':') else {
                continue;
            };
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim();
            match key.as_str() {
                "name" | "nome" => {
                    let n = sanitize_text(val, 16);
                    if !n.is_empty() && !n.contains(' ') {
                        info.name = Some(n);
                    }
                }
                "level" | "nível" | "nivel" => {
                    info.level = parse_int(val).and_then(|n| u32::try_from(n).ok());
                }
                "resets" => {
                    info.resets = parse_int(val).and_then(|n| u32::try_from(n).ok());
                }
                "wc" => info.wc = parse_int(val),
                "wp" => info.wp = parse_int(val),
                "gp" => info.gp_tenths = parse_tenths(val),
                _ => {}
            }
        }
        info
    }
}

pub fn parse_int(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() || s.len() > 24 {
        return None;
    }
    let mut n: u64 = 0;
    let mut digits = 0;
    for c in s.chars() {
        match c {
            '0'..='9' => {
                n = n.checked_mul(10)?.checked_add(c as u64 - '0' as u64)?;
                digits += 1;
            }
            ',' => {}
            _ => return None,
        }
    }
    (digits > 0).then_some(n)
}

pub fn parse_tenths(s: &str) -> Option<u64> {
    let s = s.trim();
    let (int_part, dec_part) = s.split_once('.').unwrap_or((s, ""));
    let i = parse_int(int_part)?;
    let first_dec = match dec_part.chars().next() {
        None => 0,
        Some(c) if c.is_ascii_digit() => c as u64 - '0' as u64,
        Some(_) => return None,
    };
    if !dec_part.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    i.checked_mul(10)?.checked_add(first_dec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_english_and_portuguese_client_titles() {
        let en = TitleInfo::parse(
            "TheClassic MU || Name: ReDMooN || Level: 400 || Resets: 911 || WC: 0 || WP: 21,519 || GP: 15,840.0",
        );
        assert_eq!(en.name.as_deref(), Some("ReDMooN"));
        assert_eq!(en.level, Some(400));
        assert_eq!(en.resets, Some(911));
        assert_eq!(en.gp_tenths, Some(158_400));
        assert!(en.is_logged_in());

        let pt = TitleInfo::parse("TheClassic MU || Nome: xWizard || Level: 400 || Resets: 900");
        assert_eq!(pt.name.as_deref(), Some("xWizard"));
        assert!(pt.is_logged_in());

        let login = TitleInfo::parse("TheClassic MU");
        assert!(!login.is_logged_in());
    }
}
