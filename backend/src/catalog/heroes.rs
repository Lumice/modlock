// Adapted 2026-10-28 from Deadlock Mod Manager at 976637e2d63329ffef6f87eca40e2aea681acc94.
// GPL-3.0; see UPSTREAM-LICENSE. Registry data retained; resolver is adapted.
// Generated from packages/shared/src/heroes/.
// Run apps/desktop/scripts/generate-gamebanana-hero-registry.ts after edits.

pub const HERO_ALIASES: &[(&str, &[&str])] = &[
    (
        "Abrams",
        &["Abrams", "atlas", "atlas detective", "atlas detective v2"],
    ),
    ("Apollo", &["Apollo", "fencer"]),
    ("Baba", &["Baba"]),
    ("Bebop", &["Bebop"]),
    ("Billy", &["Billy", "punkgoat"]),
    ("Calico", &["Calico", "cadence", "nano"]),
    ("Celeste", &["Celeste", "unicorn"]),
    (
        "Deadman Danny",
        &["Deadman Danny", "DeadmanDanny", "deadmandanny", "deadpack"],
    ),
    ("Doorman", &["Doorman", "doorman v2"]),
    ("Drifter", &["Drifter"]),
    ("Dynamo", &["Dynamo", "prof dynamo"]),
    ("Graves", &["Graves", "necro"]),
    (
        "Grey Talon",
        &[
            "Grey Talon",
            "GreyTalon",
            "archer",
            "archer v2",
            "greytalon",
        ],
    ),
    ("Haze", &["Haze", "haze v2"]),
    ("Holliday", &["Holliday", "astro"]),
    ("Infernus", &["Infernus", "inferno", "inferno v4"]),
    ("Ivy", &["Ivy", "tengu"]),
    ("Kelvin", &["Kelvin", "kelvin explorer", "kelvin v2"]),
    (
        "Lady Geist",
        &["Lady Geist", "LadyGeist", "geist", "ghost", "ladygeist"],
    ),
    ("Lash", &["Lash", "lash v2"]),
    ("McGinnis", &["McGinnis", "engineer"]),
    ("Mina", &["Mina", "vampirebat"]),
    ("Mirage", &["Mirage", "mirage v2"]),
    (
        "Mo & Krill",
        &["Mo & Krill", "MoKrill", "digger", "mo krill", "mokrill"],
    ),
    (
        "Nurse Harrow",
        &["Nurse Harrow", "NurseHarrow", "nurseharrow"],
    ),
    ("Paige", &["Paige", "bookworm"]),
    ("Paradox", &["Paradox", "chrono"]),
    ("Pocket", &["Pocket", "synth"]),
    ("Rat King", &["Rat King", "RatKing", "ratking"]),
    ("Rem", &["Rem", "familiar"]),
    ("Seven", &["Seven", "7", "gigawatt", "gigawatt prisoner"]),
    ("Shiv", &["Shiv", "shiv ult"]),
    ("Silver", &["Silver", "werewolf"]),
    ("Sinclair", &["Sinclair", "magician", "magician v2"]),
    ("Solomon", &["Solomon", "chessmaster"]),
    ("Venator", &["Venator", "priest"]),
    ("Victor", &["Victor", "frank", "frank v2", "viktor"]),
    ("Vindicta", &["Vindicta", "hornet", "hornet v3"]),
    ("Violet", &["Violet"]),
    ("Viscous", &["Viscous"]),
    ("Vyper", &["Vyper", "viper"]),
    ("Warden", &["Warden"]),
    (
        "Wraith",
        &[
            "Wraith",
            "wraith gen man",
            "wraith magician",
            "wraith puppeteer",
        ],
    ),
    ("Wrecker", &["Wrecker", "butcher"]),
    ("Yamato", &["Yamato", "yamato v2"]),
];

pub const HERO_FUZZY_TOKENS: &[(&str, &[&str])] = &[
    ("Abrams", &["brams"]),
    ("Grey Talon", &["talon"]),
    ("Infernus", &["fernus"]),
    ("Lady Geist", &["geist", "gaist"]),
    ("Mo & Krill", &["krill"]),
    ("Paradox", &["dox"]),
];

pub const HERO_PHRASES: &[(&str, &[&str])] = &[
    ("Abrams", &["atlas detective", "atlas detective v2"]),
    ("Deadman Danny", &["deadman danny"]),
    ("Doorman", &["doorman v2"]),
    ("Dynamo", &["prof dynamo"]),
    ("Grey Talon", &["grey talon", "archer v2"]),
    ("Haze", &["haze v2"]),
    ("Infernus", &["inferno v4"]),
    ("Kelvin", &["kelvin explorer", "kelvin v2"]),
    ("Lady Geist", &["lady geist", "lady geist"]),
    ("Lash", &["lash v2"]),
    ("Mirage", &["mirage v2"]),
    (
        "Mo & Krill",
        &["mo krill", "mo and krill", "mo krill", "mo krill"],
    ),
    ("Nurse Harrow", &["nurse harrow"]),
    ("Rat King", &["rat king"]),
    ("Seven", &["gigawatt prisoner"]),
    ("Shiv", &["shiv ult"]),
    ("Sinclair", &["magician v2"]),
    ("Victor", &["frank v2"]),
    ("Vindicta", &["hornet v3"]),
    (
        "Wraith",
        &["wraith gen man", "wraith magician", "wraith puppeteer"],
    ),
    ("Yamato", &["yamato v2"]),
];

fn key(value: &str) -> String {
    let mut out = String::new();
    let mut space = false;
    for c in value.trim().to_lowercase().chars() {
        match c {
            'a'..='z' | '0'..='9' => {
                if space && !out.is_empty() {
                    out.push(' ');
                }
                out.push(c);
                space = false;
            }
            '&' => {
                if !out.is_empty() && !out.ends_with(' ') {
                    out.push(' ');
                }
                out.push_str("and");
                space = true;
            }
            '\'' | '`' => {}
            _ => space = true,
        }
    }
    out
}
pub fn resolve(root: &str, category: &str, name: &str) -> Option<String> {
    fn normalized(value: &str) -> Option<String> {
        if let Some(display) = crate::hero::display_for_internal(value.trim()) { return Some(display); }
        let k = key(value);
        HERO_ALIASES
            .iter()
            .find(|(_, a)| a.iter().any(|s| key(s) == k))
            .map(|(h, _)| (*h).into())
    }
    if root == "Skins" && !matches!(category, "Skins" | "Other") {
        if let Some(h) = normalized(category) {
            return Some(h);
        }
    }
    if let Some(h) = normalized(name) {
        return Some(h);
    }
    let padded = format!(" {} ", key(name));
    HERO_ALIASES
        .iter()
        .find(|(h, a)| {
            a.iter()
                .filter(|a| !key(a).contains(' '))
                .chain(
                    HERO_FUZZY_TOKENS
                        .iter()
                        .chain(HERO_PHRASES)
                        .filter(|(x, _)| x == h)
                        .flat_map(|(_, p)| p.iter()),
                )
                .any(|p| padded.contains(&format!(" {} ", key(p))))
        })
        .map(|(h, _)| (*h).into())
}

#[cfg(test)]
mod hero_mapping_tests {
    #[test]
    fn hero_parser_aliases_match_catalog_filters() {
        let mapping = json::parse(include_str!("../../data/hero-mapping.json")).unwrap();
        for row in mapping.members() {
            assert_eq!(super::resolve("Skins", row["internal"].as_str().unwrap(), ""),
                       Some(row["display"].as_str().unwrap().to_string()));
        }
    }
}
