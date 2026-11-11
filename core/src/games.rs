//! Per-game profiles: "when this game is in front, use that profile". The rules live in config.ini as one line
//! (`[app] games=cs2=Pro|valorant=Main`); the overlay's platform layer reports what is in front (see
//! overlay/src/games.rs) and `Model::game_focus` switches. No platform code here.
//!
//! A game is named by its program file without folder and without ".exe", in small letters ("cs2", "valorant"),
//! so one rule matches the Windows build (`cs2.exe`) and the Linux build (`cs2`) of the same game.

/// The name a rule uses for a program: "C:\Games\CS2\cs2.exe" -> "cs2", "/usr/bin/Foo" -> "foo".
pub fn exe_key(s: &str) -> String {
    let s = s.trim().trim_matches('"');
    let base = s.rsplit(['/', '\\']).next().unwrap_or(s).trim();
    let low = base.to_lowercase();
    let low = low.strip_suffix(".exe").unwrap_or(&low);
    low.chars().filter(|c| !c.is_control() && !matches!(c, '|' | '=' | '[' | ']')).collect::<String>().trim().to_string()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    /// `exe_key` of the game.
    pub game: String,
    /// The profile's name.
    pub profile: String,
}

pub fn parse(s: &str) -> Vec<Rule> {
    let mut v: Vec<Rule> = Vec::new();
    for part in s.split('|') {
        let Some((g, p)) = part.split_once('=') else { continue };
        let (g, p) = (exe_key(g), p.trim().to_string());
        if g.is_empty() || p.is_empty() || v.iter().any(|r| r.game == g) || v.len() >= 200 {
            continue;
        }
        v.push(Rule { game: g, profile: p });
    }
    v
}

pub fn format(rules: &[Rule]) -> String {
    rules.iter().map(|r| format!("{}={}", r.game, r.profile)).collect::<Vec<_>>().join("|")
}

/// The first rule for any of `seen` (the names the platform found for the window in front).
pub fn rule_for<'a>(rules: &'a [Rule], seen: &[String]) -> Option<&'a Rule> {
    let keys: Vec<String> = seen.iter().map(|s| exe_key(s)).filter(|k| !k.is_empty()).collect();
    rules.iter().find(|r| keys.contains(&r.game))
}

/// What the overlay did when a game came to the front, so it can go back when the game leaves.
#[derive(Clone, Debug)]
pub struct Auto {
    /// The profile name + crosshair from before the first switch.
    pub before_name: String,
    pub before: crate::config::Profile,
    /// What was switched to (if the crosshair was changed by hand since, it is kept).
    pub loaded: crate::config::Profile,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_ignore_folders_case_and_exe() {
        assert_eq!(exe_key(r"C:\Program Files\Steam\steamapps\common\Counter-Strike Global Offensive\game\bin\win64\cs2.exe"), "cs2");
        assert_eq!(exe_key("/games/.steam/steam/steamapps/common/Counter-Strike Global Offensive/game/bin/linuxsteamrt64/cs2"), "cs2");
        assert_eq!(exe_key("VALORANT-Win64-Shipping.EXE"), "valorant-win64-shipping");
        assert_eq!(exe_key("  \"Z:\\x\\Game|=.exe\" "), "game");
        assert_eq!(exe_key(""), "");
    }

    #[test]
    fn rules_parse_format_and_match() {
        let r = parse("cs2.exe=Pro| Apex = Main |broken|=x|cs2=Dup|r5apex=Main");
        assert_eq!(r, vec![
            Rule { game: "cs2".into(), profile: "Pro".into() },
            Rule { game: "apex".into(), profile: "Main".into() },
            Rule { game: "r5apex".into(), profile: "Main".into() },
        ]);
        assert_eq!(parse(&format(&r)), r);
        assert_eq!(rule_for(&r, &["steam".into(), "/x/cs2".into()]).unwrap().profile, "Pro");
        assert!(rule_for(&r, &["firefox".into()]).is_none());
        assert!(parse("").is_empty());
    }

    fn model(tag: &str) -> (crate::model::Model, std::path::PathBuf) {
        let d = std::env::temp_dir().join(format!("aimdot-games-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        (crate::model::Model::new(d.clone(), crate::load_png), d)
    }

    #[test]
    fn switches_in_and_back_out() {
        use crate::config::Profile;
        let (mut m, dir) = model("focus");
        let a = Profile { dot: true, ..Profile::default() };
        let b = Profile { circle: true, ..Profile::default() };
        m.profiles = vec![("A".into(), a.clone()), ("B".into(), b.clone())];
        m.load_profile(0);
        assert!(!m.games_needed());
        m.set_game_rule(r"C:\x\cs2.exe", Some("B"));
        m.set_game_rule("apex", Some("A"));
        assert!(m.games_needed());
        // cs2 in front -> B; desktop -> back to A
        assert!(m.game_focus(&["cs2".into()]));
        assert_eq!((m.set.active_profile.as_str(), &m.cur), ("B", &b));
        assert!(!m.game_focus(&["cs2".into()]), "same game again: nothing to do");
        assert!(m.game_focus(&[]));
        assert_eq!((m.set.active_profile.as_str(), &m.cur), ("A", &a));
        assert!(!m.game_focus(&["firefox".into()]));
        // cs2 -> apex (A) -> desktop: back to the crosshair from before cs2, even with no profile active
        m.cur = Profile { outer: true, ..Profile::default() };
        m.set.active_profile.clear();
        let mine = m.cur.clone();
        assert!(m.game_focus(&["cs2".into()]));
        assert!(m.game_focus(&["apex".into()]));
        assert_eq!(m.cur, a);
        assert!(m.game_focus(&[]));
        assert_eq!((m.set.active_profile.as_str(), &m.cur), ("", &mine));
        // changed by hand while in the game: kept when the game leaves
        assert!(m.game_focus(&["cs2".into()]));
        m.cur.dot_size = 9.0;
        assert!(!m.game_focus(&[]));
        assert_eq!(m.cur.dot_size, 9.0);
        // switched off: no switching, no detection needed
        m.set.games_on = false;
        assert!(!m.games_needed());
        assert!(!m.game_focus(&["cs2".into()]));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rename_and_delete_keep_rules_in_step() {
        let (mut m, dir) = model("names");
        let p = crate::config::Profile::default();
        m.profiles = vec![("Main".into(), p.clone()), ("Sniper".into(), p.clone())];
        m.set.active_profile = "Main".into();
        m.set_game_rule("cs2", Some("Main"));
        m.set_game_rule("apex", Some("Sniper"));
        assert!(m.rename_profile(0, "sniper").is_err(), "names are unique, any case");
        assert!(m.rename_profile(0, "  ").is_err());
        assert_eq!(m.rename_profile(0, "Pro [1]").unwrap(), "Pro 1");
        assert_eq!(m.set.active_profile, "Pro 1");
        assert_eq!(m.set.games, "cs2=Pro 1|apex=Sniper");
        assert_eq!(m.free_profile_name("pro 1"), "pro 1 (2)");
        assert_eq!(m.add_profile("Sniper", p.clone()), Some(2));
        assert_eq!(m.profiles[2].0, "Sniper (2)");
        m.move_profile(2, -1);
        assert_eq!(m.profiles[1].0, "Sniper (2)");
        m.move_profile(0, -1); // first stays first
        assert_eq!(m.profiles[0].0, "Pro 1");
        m.delete_profile(0);
        assert_eq!(m.set.active_profile, "");
        assert_eq!(m.set.games, "apex=Sniper");
        m.set_game_rule("apex.exe", None);
        assert_eq!(m.set.games, "");
        // survives the config text (what goes to the overlay and to config.ini)
        m.set_game_rule("cs2", Some("Sniper"));
        let text = m.to_text();
        let (set, _, profs) = crate::config::parse(&text);
        assert_eq!(set.games, "cs2=Sniper");
        assert_eq!(profs.len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }
}
