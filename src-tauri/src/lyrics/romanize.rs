//! Script detection and transliteration for lyric lines.
//!
//! Every transform here is deterministic and offline. Japanese kana, Hangul, Hanzi, Cyrillic and
//! Greek all convert without a network round trip.

use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::sync::OnceLock;

/// Unicode blocks that select a transliteration strategy.
pub const HIRAGANA: std::ops::RangeInclusive<u32> = 0x3040..=0x309F;
pub const KATAKANA: std::ops::RangeInclusive<u32> = 0x30A0..=0x30FF;
pub const CJK: std::ops::RangeInclusive<u32> = 0x4E00..=0x9FFF;
pub const HANGUL: std::ops::RangeInclusive<u32> = 0xAC00..=0xD7AF;
pub const HANGUL_JAMO: std::ops::RangeInclusive<u32> = 0x1100..=0x11FF;
pub const CYRILLIC: std::ops::RangeInclusive<u32> = 0x0400..=0x04FF;
pub const GREEK: std::ops::RangeInclusive<u32> = 0x0370..=0x03FF;

/// Fraction of characters that must belong to a script before it is treated as that language.
/// Lyrics are frequently bilingual, so a single borrowed word should not flip the whole line.
const SCRIPT_DOMINANCE: f32 = 0.3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Script {
    Japanese,
    Korean,
    Chinese,
    Cyrillic,
    Greek,
    Latin,
    Other,
}

fn ratio(text: &str, matches: impl Fn(char) -> bool) -> f32 {
    let total = text.chars().filter(|c| !c.is_whitespace()).count();
    if total == 0 {
        return 0.0;
    }
    text.chars()
        .filter(|c| !c.is_whitespace() && matches(*c))
        .count() as f32
        / total as f32
}

/// Picks the dominant script in a line.
pub fn detect_script(text: &str) -> Script {
    let kana = ratio(text, |c| {
        HIRAGANA.contains(&(c as u32)) || KATAKANA.contains(&(c as u32))
    });
    // Kana are unique to Japanese and outrank any Hanzi on the same line.
    if kana > 0.05 {
        return Script::Japanese;
    }
    if ratio(text, |c| {
        HANGUL.contains(&(c as u32)) || HANGUL_JAMO.contains(&(c as u32))
    }) > SCRIPT_DOMINANCE
    {
        return Script::Korean;
    }
    if ratio(text, |c| CJK.contains(&(c as u32))) > SCRIPT_DOMINANCE {
        return Script::Chinese;
    }
    if ratio(text, |c| CYRILLIC.contains(&(c as u32))) > SCRIPT_DOMINANCE {
        return Script::Cyrillic;
    }
    if ratio(text, |c| GREEK.contains(&(c as u32))) > SCRIPT_DOMINANCE {
        return Script::Greek;
    }
    if ratio(text, |c| c.is_ascii_alphabetic()) > 0.0 {
        return Script::Latin;
    }
    Script::Other
}

// ---------------------------------------------------------------------------------------------
// Japanese
// ---------------------------------------------------------------------------------------------

/// Katakana live one contiguous block above hiragana, so a single offset covers the whole
/// syllabary and only one lookup table is needed.
fn to_hiragana(c: char) -> char {
    let code = c as u32;
    if (0x30A1..=0x30F6).contains(&code) {
        char::from_u32(code - 0x60).unwrap_or(c)
    } else {
        c
    }
}

/// Hepburn readings indexed by `kana - 0x3041`, laid out as consecutive gojūon rows. Entries in
/// ALL CAPS mark small kana, which are only valid as the second half of a contraction.
const HEPBURN: [&str; 83] = [
    // ぁ あ ぃ ぃ... (0x3041..)
    "a", "a", "i", "i", "u", "u", "e", "e", "o", "o",
    // か が き ぎ く ぐ け げ こ ご
    "ka", "ga", "ki", "gi", "ku", "gu", "ke", "ge", "ko", "go",
    // さ ざ し じ す ず せ ぜ そ ぞ
    "sa", "za", "shi", "ji", "su", "zu", "se", "ze", "so", "zo",
    // た だ ち ぢ っ つ づ て で と ど
    "ta", "da", "chi", "ji", "TSU", "tsu", "zu", "te", "de", "to", "do",
    // な に ぬ ね の
    "na", "ni", "nu", "ne", "no",
    // は ば ぱ ひ び ぴ ふ ぶ ぷ へ べ ぺ ほ ぼ ぽ
    "ha", "ba", "pa", "hi", "bi", "pi", "fu", "bu", "pu", "he", "be", "pe", "ho", "bo", "po",
    // ま み む め も
    "ma", "mi", "mu", "me", "mo", // ゃ や ゅ ゆ ょ よ
    "YA", "ya", "YU", "yu", "YO", "yo", // ら り る れ ろ
    "ra", "ri", "ru", "re", "ro", // ゎ わ ゐ ゑ を ん
    "WA", "wa", "wi", "we", "wo", "n",
];

/// Contracts a -i kana with a small ゃ/ゅ/ょ into one Hepburn syllable: き + ゃ → kya, し + ゃ → sha.
/// Derived from the base kana's own reading so the two tables can never disagree.
fn yoon_reading(base: char, small: char) -> Option<String> {
    let vowel = match small {
        'ゃ' => "a",
        'ゅ' => "u",
        'ょ' => "o",
        _ => return None,
    };
    // Only -i kana contract; a bare あ + ゃ is just two syllables.
    let stem = lookup(base)?.strip_suffix('i')?;
    // sh/ch/j already end in the palatal sound Hepburn writes, so they take the vowel directly.
    if matches!(stem, "sh" | "ch" | "j") {
        Some(format!("{stem}{vowel}"))
    } else {
        Some(format!("{stem}y{vowel}"))
    }
}

fn lookup(c: char) -> Option<&'static str> {
    let index = (c as u32).checked_sub(0x3041)? as usize;
    HEPBURN.get(index).copied()
}

/// Hepburn romanization for kana. Kanji are preserved here so the dictionary path can use it as a
/// safe fallback for unknown words and punctuation.
fn romanize_kana(text: &str) -> String {
    let chars: Vec<char> = text.chars().map(to_hiragana).collect();
    let mut out = String::with_capacity(text.len() * 2);
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];

        // Sokuon: the small tsu doubles the following consonant.
        if c == 'っ' {
            if let Some(next) = chars.get(i + 1).and_then(|n| lookup(*n)) {
                if let Some(first) = next.chars().next() {
                    out.push(first);
                }
            }
            i += 1;
            continue;
        }

        // Long vowel mark repeats the previous vowel sound.
        if c == 'ー' {
            if let Some(last) = out.chars().last().filter(|l| "aeiou".contains(*l)) {
                out.push(last);
            }
            i += 1;
            continue;
        }

        if let Some(small) = chars.get(i + 1).copied() {
            if let Some(combined) = yoon_reading(c, small) {
                out.push_str(&combined);
                i += 2;
                continue;
            }
        }

        match lookup(c) {
            Some(reading) => {
                out.push_str(&reading.to_lowercase());
                // Hepburn separates a syllable-final ん from a following vowel or y-glide with an
                // apostrophe, so しんいち reads shin'ichi rather than shinichi.
                if reading == "n"
                    && chars.get(i + 1).is_some_and(|next| {
                        matches!(next, 'あ' | 'い' | 'う' | 'え' | 'お' | 'や' | 'ゆ' | 'よ')
                    })
                {
                    out.push('\'');
                }
                i += 1;
            }
            None => {
                out.push(c);
                i += 1;
            }
        }
    }

    out
}

fn japanese_segmenter() -> Option<&'static lindera::segmenter::Segmenter> {
    static SEGMENTER: OnceLock<Option<lindera::segmenter::Segmenter>> = OnceLock::new();
    SEGMENTER
        .get_or_init(|| {
            lindera::dictionary::load_dictionary("embedded://ipadic")
                .ok()
                .map(|dictionary| {
                    lindera::segmenter::Segmenter::new(
                        lindera::mode::Mode::Normal,
                        dictionary,
                        None,
                    )
                })
        })
        .as_ref()
}

/// Hepburn romanization of a Japanese line, including dictionary-backed kanji readings.
pub fn romanize_japanese(text: &str) -> String {
    if !text.chars().any(|c| CJK.contains(&(c as u32))) {
        return romanize_kana(text);
    }
    let Some(segmenter) = japanese_segmenter() else {
        return romanize_kana(text);
    };
    let Ok(mut tokens) = segmenter.segment(Cow::Borrowed(text)) else {
        return romanize_kana(text);
    };

    let mut out = String::with_capacity(text.len() * 2);
    let mut cursor = 0;
    for token in &mut tokens {
        if token.byte_start > cursor {
            out.push_str(&romanize_kana(&text[cursor..token.byte_start]));
        }
        let surface = &text[token.byte_start..token.byte_end];
        let reading = token
            .details()
            .get(7)
            .copied()
            .filter(|value| !value.is_empty() && *value != "*");
        out.push_str(&romanize_kana(reading.unwrap_or(surface)));
        cursor = token.byte_end;
    }
    if cursor < text.len() {
        out.push_str(&romanize_kana(&text[cursor..]));
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Korean
// ---------------------------------------------------------------------------------------------

const HANGUL_INITIAL: [&str; 19] = [
    "g", "kk", "n", "d", "tt", "r", "m", "b", "pp", "s", "ss", "", "j", "jj", "ch", "k", "t", "p",
    "h",
];
const HANGUL_VOWEL: [&str; 21] = [
    "a", "ae", "ya", "yae", "eo", "e", "yeo", "ye", "o", "wa", "wae", "oe", "yo", "u", "wo", "we",
    "wi", "yu", "eu", "ui", "i",
];
const HANGUL_FINAL: [&str; 28] = [
    "", "k", "kk", "ks", "n", "nj", "nh", "t", "l", "lk", "lm", "lb", "ls", "lt", "lp", "lh", "m",
    "p", "ps", "t", "t", "ng", "t", "t", "k", "t", "p", "h",
];

/// Revised Romanization of Korean, syllable by syllable.
pub fn romanize_korean(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    for c in text.chars() {
        let code = c as u32;
        if !HANGUL.contains(&code) {
            out.push(c);
            continue;
        }
        let syllable = code - 0xAC00;
        let initial = (syllable / 588) as usize;
        let vowel = ((syllable % 588) / 28) as usize;
        let final_ = (syllable % 28) as usize;
        out.push_str(HANGUL_INITIAL[initial]);
        out.push_str(HANGUL_VOWEL[vowel]);
        out.push_str(HANGUL_FINAL[final_]);
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Chinese
// ---------------------------------------------------------------------------------------------

/// Hanyu Pinyin with tone marks, leaving non-Han characters untouched. `to_pinyin` yields exactly
/// one item per character of the input, so zipping it with `chars()` restores punctuation and
/// spacing in place.
pub fn romanize_chinese(text: &str) -> String {
    use pinyin::ToPinyin;

    let mut out = String::with_capacity(text.len() * 2);
    for (c, reading) in text.chars().zip(text.to_pinyin()) {
        match reading {
            Some(pinyin) => out.push_str(pinyin.with_tone()),
            None => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Cyrillic & Greek
// ---------------------------------------------------------------------------------------------

/// ISO 9 / ALA-LC style transliteration tables.
fn map_chars(text: &str, table: &[(char, &str)]) -> String {
    text.chars()
        .map(|c| {
            table
                .iter()
                .find(|(from, _)| *from == c)
                .map(|(_, to)| (*to).to_string())
                .unwrap_or_else(|| c.to_string())
        })
        .collect()
}

#[rustfmt::skip]
const CYRILLIC_MAP: &[(char, &str)] = &[
    ('а', "a"), ('б', "b"), ('в', "v"), ('г', "g"), ('д', "d"), ('е', "e"), ('ё', "yo"),
    ('ж', "zh"), ('з', "z"), ('и', "i"), ('й', "y"), ('к', "k"), ('л', "l"), ('м', "m"),
    ('н', "n"), ('о', "o"), ('п', "p"), ('р', "r"), ('с', "s"), ('т', "t"), ('у', "u"),
    ('ф', "f"), ('х', "kh"), ('ц', "ts"), ('ч', "ch"), ('ш', "sh"), ('щ', "shch"), ('ъ', ""),
    ('ы', "y"), ('ь', ""), ('э', "e"), ('ю', "yu"), ('я', "ya"),
    ('А', "A"), ('Б', "B"), ('В', "V"), ('Г', "G"), ('Д', "D"), ('Е', "E"), ('Ё', "Yo"),
    ('Ж', "Zh"), ('З', "Z"), ('И', "I"), ('Й', "Y"), ('К', "K"), ('Л', "L"), ('М', "M"),
    ('Н', "N"), ('О', "O"), ('П', "P"), ('Р', "R"), ('С', "S"), ('Т', "T"), ('У', "U"),
    ('Ф', "F"), ('Х', "Kh"), ('Ц', "Ts"), ('Ч', "Ch"), ('Ш', "Sh"), ('Щ', "Shch"), ('Ъ', ""),
    ('Ы', "Y"), ('Ь', ""), ('Э', "E"), ('Ю', "Yu"), ('Я', "Ya"),
    // Ukrainian / Serbian letters that appear in Cyrillic lyrics.
    ('і', "i"), ('ї', "yi"), ('є', "ye"), ('ґ', "g"), ('І', "I"), ('Ї', "Yi"), ('Є', "Ye"),
    ('Ґ', "G"), ('ђ', "dj"), ('ј', "j"), ('љ', "lj"), ('њ', "nj"), ('ћ', "c"), ('џ', "dz"),
    ('Ђ', "Dj"), ('Ј', "J"), ('Љ', "Lj"), ('Њ', "Nj"), ('Ћ', "C"), ('Џ', "Dz"),
];

#[rustfmt::skip]
const GREEK_MAP: &[(char, &str)] = &[
    ('α', "a"), ('β', "v"), ('γ', "g"), ('δ', "d"), ('ε', "e"), ('ζ', "z"), ('η', "i"),
    ('θ', "th"), ('ι', "i"), ('κ', "k"), ('λ', "l"), ('μ', "m"), ('ν', "n"), ('ξ', "x"),
    ('ο', "o"), ('π', "p"), ('ρ', "r"), ('σ', "s"), ('ς', "s"), ('τ', "t"), ('υ', "y"),
    ('φ', "f"), ('χ', "ch"), ('ψ', "ps"), ('ω', "o"),
    ('Α', "A"), ('Β', "V"), ('Γ', "G"), ('Δ', "D"), ('Ε', "E"), ('Ζ', "Z"), ('Η', "I"),
    ('Θ', "Th"), ('Ι', "I"), ('Κ', "K"), ('Λ', "L"), ('Μ', "M"), ('Ν', "N"), ('Ξ', "X"),
    ('Ο', "O"), ('Π', "P"), ('Ρ', "R"), ('Σ', "S"), ('Τ', "T"), ('Υ', "Y"), ('Φ', "F"),
    ('Χ', "Ch"), ('Ψ', "Ps"), ('Ω', "O"),
    // Tonos and diaeresis forms, which every accented Greek lyric uses.
    ('ά', "a"), ('έ', "e"), ('ή', "i"), ('ί', "i"), ('ό', "o"), ('ύ', "y"), ('ώ', "o"),
    ('Ά', "A"), ('Έ', "E"), ('Ή', "I"), ('Ί', "I"), ('Ό', "O"), ('Ύ', "Y"), ('Ώ', "O"),
    ('ϊ', "i"), ('ϋ', "y"), ('ΐ', "i"), ('ΰ', "y"), ('Ϊ', "I"), ('Ϋ', "Y"),
];

pub fn romanize_cyrillic(text: &str) -> String {
    map_chars(text, CYRILLIC_MAP)
}

pub fn romanize_greek(text: &str) -> String {
    map_chars(text, GREEK_MAP)
}

/// Transliterates a lyric line using whichever script it is written in. Latin and unrecognised
/// lines come back unchanged.
pub fn romanize(text: &str) -> String {
    match detect_script(text) {
        Script::Japanese => romanize_japanese(text),
        Script::Korean => romanize_korean(text),
        Script::Chinese => romanize_chinese(text),
        Script::Cyrillic => romanize_cyrillic(text),
        Script::Greek => romanize_greek(text),
        Script::Latin | Script::Other => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_each_script() {
        assert_eq!(detect_script("夜に駆ける"), Script::Japanese);
        assert_eq!(detect_script("ヨルニカケル"), Script::Japanese);
        assert_eq!(detect_script("봄날"), Script::Korean);
        assert_eq!(detect_script("你好世界"), Script::Chinese);
        assert_eq!(detect_script("Привет мир"), Script::Cyrillic);
        assert_eq!(detect_script("Καλημέρα"), Script::Greek);
        assert_eq!(detect_script("Just a song"), Script::Latin);
        assert_eq!(detect_script("123 !!!"), Script::Other);
    }

    #[test]
    fn kana_romanizes_with_hepburn_contractions() {
        assert_eq!(romanize_japanese("さくら"), "sakura");
        assert_eq!(romanize_japanese("しんぶん"), "shinbun");
        assert_eq!(romanize_japanese("きょ"), "kyo");
        // きょう is kyo + the long-vowel う, so the ASCII form keeps both.
        assert_eq!(romanize_japanese("きょう"), "kyou");
        assert_eq!(romanize_japanese("しゃしん"), "shashin");
        assert_eq!(romanize_japanese("ちょっと"), "chotto");
        assert_eq!(romanize_japanese("がっこう"), "gakkou");
        assert_eq!(romanize_japanese("コーヒー"), "koohii");
        assert_eq!(romanize_japanese("ふじさん"), "fujisan");
        // ん takes an apostrophe before a vowel to keep the syllables apart.
        assert_eq!(romanize_japanese("しんいち"), "shin'ichi");
        assert_eq!(romanize_japanese("しんぶん"), "shinbun");
    }

    #[test]
    fn japanese_dictionary_converts_kanji_and_kana() {
        assert_eq!(romanize_japanese("駆ける"), "kakeru");
        assert_eq!(romanize_japanese("夜に駆ける"), "yorunikakeru");
        assert_eq!(romanize_japanese("ヨル"), "yoru");
    }

    #[test]
    fn hangul_uses_revised_romanization() {
        assert_eq!(romanize_korean("안녕"), "annyeong");
        assert_eq!(romanize_korean("한국"), "hanguk");
        assert_eq!(romanize_korean("사랑"), "sarang");
        // Final consonant then a null onset keeps both syllables intact.
        assert_eq!(romanize_korean("음악"), "eumak");
    }

    #[test]
    fn chinese_produces_pinyin_with_tone_marks() {
        let out = romanize_chinese("你好");
        assert_eq!(out, "nǐhǎo");
        // Non-Han characters survive the round trip.
        let mixed = romanize_chinese("你好, world!");
        assert!(mixed.starts_with("nǐhǎo"), "got {mixed}");
        assert!(mixed.ends_with(", world!"), "got {mixed}");
    }

    #[test]
    fn cyrillic_and_greek_transliterate() {
        assert_eq!(romanize_cyrillic("Привет"), "Privet");
        assert_eq!(romanize_cyrillic("Москва"), "Moskva");
        assert_eq!(romanize_greek("Καλημέρα"), "Kalimera");
    }

    #[test]
    fn latin_lines_are_returned_untouched() {
        assert_eq!(romanize("Just a song"), "Just a song");
        assert_eq!(romanize(""), "");
    }
}
