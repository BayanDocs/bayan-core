//! The probe set of CORE-003: thirty paragraphs, each isolating something the text pipeline must get identical everywhere. The sentences were written for this spike.
//!
//! Every probe names its fonts, first choice first; Liberation Sans comes last wherever the text has characters the script's font lacks (the Noto fonts for Hebrew and Thai have no ASCII digits or punctuation, and Noto Naskh Arabic no ASCII parentheses or question mark). The three probes marked [`Probe::committed`] use only the two fonts committed in `fonts/`, so the verification gate can check them without a download.

use bayan_units::Blu;

use crate::shape::Features;

/// Liberation Sans 2.1.5, metric-compatible with Arial.
pub const SANS: &str = "LiberationSans-Regular.ttf";
/// Liberation Serif 2.1.5, metric-compatible with Times New Roman.
pub const SERIF: &str = "LiberationSerif-Regular.ttf";
/// Carlito 1.104, metric-compatible with Calibri.
pub const CARLITO: &str = "Carlito-Regular.ttf";
/// Noto Naskh Arabic 2.021.
pub const NASKH: &str = "NotoNaskhArabic-Regular.ttf";
/// Noto Sans Hebrew 3.001 (committed).
pub const HEBREW: &str = "NotoSansHebrew-Regular.ttf";
/// Noto Sans Devanagari 2.007.
pub const DEVANAGARI: &str = "NotoSansDevanagari-Regular.ttf";
/// Noto Sans Thai 2.002 (committed).
pub const THAI: &str = "NotoSansThai-Regular.ttf";
/// Noto Sans SC 2.004, the Simplified Chinese subset of Noto Sans CJK.
pub const CHINESE: &str = "NotoSansSC-Regular.otf";
/// Noto Sans JP 2.004, the Japanese subset of Noto Sans CJK.
pub const JAPANESE: &str = "NotoSansJP-Regular.otf";

/// Kerning and ligatures, as most probes use them.
const ON: Features = Features {
    kerning: true,
    ligatures: true,
};
/// Neither kerning nor ligatures.
const OFF: Features = Features {
    kerning: false,
    ligatures: false,
};
/// Ligatures without kerning.
const NO_KERNING: Features = Features {
    kerning: false,
    ligatures: true,
};

/// One paragraph of the probe set.
#[derive(Clone, Copy, Debug)]
pub struct Probe {
    /// A stable name, used in file names and results.
    pub id: &'static str,
    /// What the probe checks.
    pub checks: &'static str,
    /// The text.
    pub text: &'static str,
    /// Whether the paragraph is right to left.
    pub rtl: bool,
    /// Its fonts, by file name, first choice first.
    pub fonts: &'static [&'static str],
    /// Its font size in half-points, as WordprocessingML writes it (`w:sz`).
    pub half_points: i32,
    /// The width of its lines in twips.
    pub measure_twips: i32,
    /// Its BCP 47 language.
    pub language: &'static str,
    /// The OpenType features it switches.
    pub features: Features,
}

impl Probe {
    /// The font size.
    #[must_use]
    pub fn size(&self) -> Blu {
        Blu::from_half_points(self.half_points)
    }

    /// The width of the lines.
    #[must_use]
    pub fn measure(&self) -> Blu {
        Blu::from_twips(self.measure_twips)
    }

    /// Whether every font the probe uses is committed in `fonts/`.
    #[must_use]
    pub fn committed(&self) -> bool {
        self.fonts
            .iter()
            .all(|&font| font == HEBREW || font == THAI)
    }
}

/// 3.5 inches, the measure of most probes.
const MEASURE: i32 = 5_040;

/// The probe set.
pub const PROBES: [Probe; 30] = [
    Probe {
        id: "latin-sans-kerning",
        checks: "Latin with pair kerning (GPOS kern) in Liberation Sans",
        text: "AVATAR, WAVE and TOYOTA: Tyre, Vase, Yacht and Wolf. Typography tightens pairs such as AV, AW, LT, LY, PA, TA, Te, To, VA, Vo, Wa and Yo when kerning is on.",
        rtl: false,
        fonts: &[SANS],
        half_points: 22,
        measure_twips: MEASURE,
        language: "en",
        features: ON,
    },
    Probe {
        id: "latin-sans-no-kerning",
        checks: "the same text with kerning off",
        text: "AVATAR, WAVE and TOYOTA: Tyre, Vase, Yacht and Wolf. Typography tightens pairs such as AV, AW, LT, LY, PA, TA, Te, To, VA, Vo, Wa and Yo when kerning is on.",
        rtl: false,
        fonts: &[SANS],
        half_points: 22,
        measure_twips: MEASURE,
        language: "en",
        features: NO_KERNING,
    },
    Probe {
        id: "latin-serif-kerning",
        checks: "Latin with kerning in Liberation Serif",
        text: "Waltz, bad nymph, for quick jigs vex. AWAY FROM YOUR TOWN, VALLEY AND LAKE, the river flows quietly toward the sea.",
        rtl: false,
        fonts: &[SERIF],
        half_points: 24,
        measure_twips: MEASURE,
        language: "en",
        features: ON,
    },
    Probe {
        id: "latin-serif-no-kerning",
        checks: "the same text with kerning off",
        text: "Waltz, bad nymph, for quick jigs vex. AWAY FROM YOUR TOWN, VALLEY AND LAKE, the river flows quietly toward the sea.",
        rtl: false,
        fonts: &[SERIF],
        half_points: 24,
        measure_twips: MEASURE,
        language: "en",
        features: NO_KERNING,
    },
    Probe {
        id: "latin-carlito",
        checks: "Carlito, the Calibri substitute, with kerning and standard ligatures (Liberation Sans and Serif have no ligatures, as Arial and Times New Roman have none)",
        text: "Carlito sets this sentence with the widths of Calibri: quick brown foxes jump over lazy dogs, and the office's fifty fluffy affiliates agree.",
        rtl: false,
        fonts: &[CARLITO],
        half_points: 22,
        measure_twips: MEASURE,
        language: "en",
        features: ON,
    },
    Probe {
        id: "latin-carlito-ligatures",
        checks: "the ligatures fi, fl, ff, ffi and ffl alone, without kerning",
        text: "The office staff found five fluffy waffles; fifty affluent officials reflected on the final flight and the field's flowers.",
        rtl: false,
        fonts: &[CARLITO],
        half_points: 22,
        measure_twips: MEASURE,
        language: "en",
        features: NO_KERNING,
    },
    Probe {
        id: "latin-carlito-plain",
        checks: "the same text with neither ligatures nor kerning, as Word sets it by default",
        text: "The office staff found five fluffy waffles; fifty affluent officials reflected on the final flight and the field's flowers.",
        rtl: false,
        fonts: &[CARLITO],
        half_points: 22,
        measure_twips: MEASURE,
        language: "en",
        features: OFF,
    },
    Probe {
        id: "latin-punctuation-numbers",
        checks: "digits, currency, typographic quotes, dashes and fractions",
        text: "Prices rose 12.5% between 2024 and 2026 (from €1,234.56 to €1,388.88); see §4.2 — “double” and ‘single’ quotes… 1/2, 3½ and 10:30 a.m.",
        rtl: false,
        fonts: &[SANS],
        half_points: 22,
        measure_twips: MEASURE,
        language: "en",
        features: ON,
    },
    Probe {
        id: "latin-diacritics",
        checks: "precomposed letters with diacritics, and combining marks placed by GPOS",
        text: "Crème brûlée, naïve façade, Ångström, Øresund, Dvořák, Łódź and Ştefan; with combining marks: e\u{301}cole, n\u{303}andu\u{301} and a\u{30a}.",
        rtl: false,
        fonts: &[SERIF],
        half_points: 24,
        measure_twips: MEASURE,
        language: "en",
        features: ON,
    },
    Probe {
        id: "latin-long-word",
        checks: "a word wider than the measure, which overflows its own line (no emergency breaking yet)",
        text: "Pneumonoultramicroscopicsilicovolcanoconiosis is a long word that cannot be broken.",
        rtl: false,
        fonts: &[SANS],
        half_points: 22,
        measure_twips: 1_440,
        language: "en",
        features: ON,
    },
    Probe {
        id: "latin-break-classes",
        checks: "UAX #14 classes: no break at a no-break space or a non-breaking hyphen, a break after a hyphen and around an em dash, and a soft hyphen, which the shaper hides",
        text: "A well-known co-author—who needs 10\u{a0}km of road and the 3\u{2011}4 seats—wrote extra\u{ad}ordinary, never-ending sentences.",
        rtl: false,
        fonts: &[SANS],
        half_points: 22,
        measure_twips: 2_880,
        language: "en",
        features: ON,
    },
    Probe {
        id: "arabic-marks",
        checks: "Arabic with full vowel marks (harakat, shadda, sukun) positioned by GPOS mark attachment",
        text: "كَتَبَ الطَّالِبُ الدَّرْسَ بِخَطٍّ جَمِيلٍ، ثُمَّ قَرَأَهُ عَلَى زُمَلَائِهِ فِي الْمَدْرَسَةِ.",
        rtl: true,
        fonts: &[NASKH, SANS],
        half_points: 28,
        measure_twips: MEASURE,
        language: "ar",
        features: ON,
    },
    Probe {
        id: "arabic-joining",
        checks: "Arabic joining forms (isolated, initial, medial, final) across a paragraph",
        text: "تكتب اللغة العربية من اليمين إلى اليسار، وتتصل معظم حروفها ببعضها داخل الكلمة الواحدة فتتغير أشكالها.",
        rtl: true,
        fonts: &[NASKH, SANS],
        half_points: 28,
        measure_twips: MEASURE,
        language: "ar",
        features: ON,
    },
    Probe {
        id: "arabic-numbers-latin",
        checks: "European numbers and Latin words inside Arabic text (bidi levels 1 and 2)",
        text: "بلغ عدد سكان المدينة 1,250,000 نسمة في عام 2024، ويستخدم كثير منهم نظام Linux وبرنامج BayanDocs.",
        rtl: true,
        fonts: &[NASKH, SANS],
        half_points: 28,
        measure_twips: MEASURE,
        language: "ar",
        features: ON,
    },
    Probe {
        id: "arabic-lam-alef",
        checks: "the obligatory lam-alef ligatures and the Arabic semicolon",
        text: "لا، لأن الأمل لا يأتي إلا بالعمل؛ والآلات الإلكترونية لآلئ هذا العصر وإلى آخره.",
        rtl: true,
        fonts: &[NASKH, SANS],
        half_points: 28,
        measure_twips: MEASURE,
        language: "ar",
        features: ON,
    },
    Probe {
        id: "hebrew-numbers-latin",
        checks: "numbers, a Latin word and a maqaf inside Hebrew text",
        text: "בשנת 2026 פרסמה חברת BayanDocs דוח שנתי, ובו 3.5 מיליון שורות קוד ו־42 תרגומים.",
        rtl: true,
        fonts: &[HEBREW, SANS],
        half_points: 24,
        measure_twips: MEASURE,
        language: "he",
        features: ON,
    },
    Probe {
        id: "hebrew-points",
        checks: "Hebrew with vowel points and dagesh, a maqaf and gershayim, in the committed font only",
        text: "הַיֶּלֶד קָרָא סֵפֶר בְּבֵית־הַסֵּפֶר וְהַמּוֹרָה שָׂמְחָה מְאֹד כִּי הָיָה זֶה יוֹם יָפֶה בְּצה״ל",
        rtl: true,
        fonts: &[HEBREW],
        half_points: 24,
        measure_twips: MEASURE,
        language: "he",
        features: ON,
    },
    Probe {
        id: "hebrew-brackets",
        checks: "mirrored brackets and a currency sign in right-to-left text",
        text: "המחיר (כולל מע״מ) הוא 45.90 ₪ ליחידה [ראו נספח א׳], והמשלוח (באזור המרכז) חינם.",
        rtl: true,
        fonts: &[HEBREW, SANS],
        half_points: 24,
        measure_twips: MEASURE,
        language: "he",
        features: ON,
    },
    Probe {
        id: "devanagari-conjuncts",
        checks: "Devanagari conjuncts, half forms and vowel signs around their consonants",
        text: "हिन्दी में संयुक्ताक्षर जैसे क्ष, त्र, ज्ञ, श्र और द्ध बहुत सामान्य हैं, और मात्राएँ अक्षरों के चारों ओर लगती हैं।",
        rtl: false,
        fonts: &[DEVANAGARI, SANS],
        half_points: 24,
        measure_twips: MEASURE,
        language: "hi",
        features: ON,
    },
    Probe {
        id: "devanagari-reph",
        checks: "reph (र above) and the rakar forms (र below)",
        text: "अर्थ, धर्म, कर्म, सूर्य और पूर्व में रेफ़ ऊपर दिखाई देता है; प्रकाश, क्रम और ट्रेन में र नीचे जुड़ता है।",
        rtl: false,
        fonts: &[DEVANAGARI, SANS],
        half_points: 24,
        measure_twips: MEASURE,
        language: "hi",
        features: ON,
    },
    Probe {
        id: "devanagari-numbers",
        checks: "European and Devanagari digits in Hindi text",
        text: "भारत की राजधानी नई दिल्ली है, जहाँ 2026 में लगभग ३ करोड़ लोग रहते हैं और ४५ से अधिक विश्वविद्यालय हैं।",
        rtl: false,
        fonts: &[DEVANAGARI, SANS],
        half_points: 24,
        measure_twips: MEASURE,
        language: "hi",
        features: ON,
    },
    Probe {
        id: "thai-dictionary",
        checks: "Thai without spaces between words, broken into lines with the dictionary model, in the committed font only",
        text: "ภาษาไทยไม่มีการเว้นวรรคระหว่างคำ จึงต้องใช้พจนานุกรมในการตัดคำเพื่อขึ้นบรรทัดใหม่อย่างถูกต้อง",
        rtl: false,
        fonts: &[THAI],
        half_points: 28,
        measure_twips: 4_320,
        language: "th",
        features: ON,
    },
    Probe {
        id: "thai-marks",
        checks: "Thai tone marks stacked over vowels, in the committed font only",
        text: "น้ำใจไมตรีของคนไทยเป็นที่รู้จักไปทั่วโลก ผู้คนยิ้มแย้มแจ่มใสและพร้อมช่วยเหลือผู้อื่นเสมอ",
        rtl: false,
        fonts: &[THAI],
        half_points: 28,
        measure_twips: 4_320,
        language: "th",
        features: ON,
    },
    Probe {
        id: "thai-mixed",
        checks: "Thai with European digits and a Latin word",
        text: "ในปี 2026 บริษัท BayanDocs เปิดตัวโปรแกรมประมวลผลคำรุ่นใหม่ที่รองรับภาษาไทยอย่างสมบูรณ์",
        rtl: false,
        fonts: &[THAI, SANS],
        half_points: 28,
        measure_twips: 4_320,
        language: "th",
        features: ON,
    },
    Probe {
        id: "chinese-punctuation",
        checks: "Simplified Chinese, a break opportunity between any two ideographs, and full-width punctuation (UAX #14, no kinsoku tailoring yet)",
        text: "汉字排版通常允许在任意两个汉字之间换行，但逗号、句号、问号和引号等标点符号不应出现在行首。",
        rtl: false,
        fonts: &[CHINESE, SANS],
        half_points: 21,
        measure_twips: 3_600,
        language: "zh-Hans",
        features: ON,
    },
    Probe {
        id: "chinese-mixed",
        checks: "Chinese with English words and digits, in a CFF font with Latin fallback",
        text: "这份文档包含中文、English words 和数字 2026，用于测试中英文混合排版的行宽与断行。",
        rtl: false,
        fonts: &[CHINESE, SANS],
        half_points: 21,
        measure_twips: 3_600,
        language: "zh-Hans",
        features: ON,
    },
    Probe {
        id: "japanese-kana-kanji",
        checks: "Japanese mixing hiragana, katakana and kanji, which itemize as three scripts",
        text: "日本語の文章では、ひらがな、カタカナ、漢字が混ざり合い、句読点の位置にも細かな規則があります。",
        rtl: false,
        fonts: &[JAPANESE, SANS],
        half_points: 21,
        measure_twips: 3_600,
        language: "ja",
        features: ON,
    },
    Probe {
        id: "japanese-katakana",
        checks: "katakana with the prolonged sound mark, a Common character with script extensions",
        text: "コンピューターのソフトウェア開発には、テストとレビューとドキュメントが欠かせません。",
        rtl: false,
        fonts: &[JAPANESE, SANS],
        half_points: 21,
        measure_twips: 3_600,
        language: "ja",
        features: ON,
    },
    Probe {
        id: "mixed-ltr-with-rtl",
        checks: "Hebrew with numbers, and an Arabic word, embedded in an English paragraph",
        text: "The Hebrew title “שלום עולם 2026” sits beside the Arabic word «مرحبا» in this English sentence.",
        rtl: false,
        fonts: &[SANS, HEBREW, NASKH],
        half_points: 22,
        measure_twips: MEASURE,
        language: "en",
        features: ON,
    },
    Probe {
        id: "mixed-rtl-with-ltr",
        checks: "an English quotation with parentheses and a time inside an Arabic paragraph",
        text: "قال المهندس: «The build passed (all 5 checks)» ثم غادر المكتب في الساعة 6:30 مساءً.",
        rtl: true,
        fonts: &[NASKH, SANS],
        half_points: 28,
        measure_twips: MEASURE,
        language: "ar",
        features: ON,
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::pinned_fonts;

    #[test]
    fn every_probe_has_a_unique_file_name_and_pinned_fonts() {
        let pins = pinned_fonts().unwrap();
        let mut ids: Vec<&str> = PROBES.iter().map(|probe| probe.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), PROBES.len());
        for probe in &PROBES {
            assert!(
                probe
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
                "{}",
                probe.id
            );
            assert!(!probe.fonts.is_empty(), "{}", probe.id);
            for font in probe.fonts {
                assert!(
                    pins.iter().any(|pin| pin.file == *font),
                    "{}: {font}",
                    probe.id
                );
            }
            assert!(
                !probe.text.contains(['\n', '\r', '\u{2029}']),
                "{}",
                probe.id
            );
        }
    }

    #[test]
    fn three_probes_need_only_the_committed_fonts() {
        let committed: Vec<&str> = PROBES
            .iter()
            .filter(|probe| probe.committed())
            .map(|probe| probe.id)
            .collect();
        assert_eq!(
            committed,
            ["hebrew-points", "thai-dictionary", "thai-marks"]
        );
    }
}
