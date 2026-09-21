use icu_segmenter::LineSegmenter;
use icu_segmenter::options::LineBreakOptions;

#[must_use]
pub fn line_break_opportunities(text: &str, boundaries: &[usize]) -> Vec<usize> {
    let mut starts: Vec<usize> = boundaries.to_vec();
    starts.sort_unstable();
    starts.dedup();
    LineSegmenter::new_auto(LineBreakOptions::default())
        .segment_str(text)
        .filter(|offset| *offset > 0 && *offset < text.len())
        .filter(|offset| starts.binary_search(offset).is_ok())
        .filter(|offset| !between_hangul(text, *offset))
        .collect()
}

fn between_hangul(text: &str, offset: usize) -> bool {
    let before = text.get(..offset).and_then(|head| head.chars().next_back());
    let after = text.get(offset..).and_then(|tail| tail.chars().next());
    matches!((before, after), (Some(one), Some(other)) if is_hangul(one) && is_hangul(other))
}

const fn is_hangul(character: char) -> bool {
    matches!(
        character as u32,
        0x1100..=0x11FF | 0x3130..=0x318F | 0xA960..=0xA97F | 0xAC00..=0xD7AF | 0xD7B0..=0xD7FF
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use icu_segmenter::GraphemeClusterSegmenter;

    fn graphemes(text: &str) -> Vec<usize> {
        GraphemeClusterSegmenter::new()
            .segment_str(text)
            .filter(|offset| *offset < text.len())
            .collect()
    }

    fn pieces(text: &str) -> Vec<&str> {
        let mut out = Vec::new();
        let mut last = 0;
        for offset in line_break_opportunities(text, &graphemes(text)) {
            out.push(&text[last..offset]);
            last = offset;
        }
        out.push(&text[last..]);
        out
    }

    const SAMPLES: &[&str] = &[
        "ข้อความภาษาไทยที่พิมพ์ลงในเอกสารพีดีเอฟต้องตัดบรรทัดที่ขอบคำ",
        "สวัสดีครับวันนี้อากาศดีมากนักเรียนกำลังอ่านหนังสือ",
        "ພາສາລາວເປັນພາສາທີ່ສວຍງາມຫຼາຍແລະມີປະຫວັດສາດຍາວນານ",
        "ភាសាខ្មែរគឺជាភាសាផ្លូវការរបស់ប្រទេសកម្ពុជា",
        "မြန်မာဘာသာစကားသည်မြန်မာနိုင်ငံ၏ရုံးသုံးဘာသာဖြစ်သည်",
        "中文排版需要在汉字之间换行，但是标点符号不能出现在行首。",
        "日本語の文章は、ひらがなとカタカナと漢字で書かれています。",
        "한국어는 띄어쓰기를 사용하는 언어입니다 줄바꿈은 공백에서",
        "اللغة العربية تكتب من اليمين إلى اليسار وتتصل حروفها",
        "עברית נכתבת מימין לשמאל ויש בה ניקוד",
        "हिन्दी भाषा में संयुक्ताक्षर जैसे क्ष त्र ज्ञ होते हैं",
        "বাংলা ভাষায় যুক্তাক্ষর অনেক আছে যেমন ক্ষ ত্র",
        "தமிழ் மொழி மிகவும் பழமையான மொழிகளில் ஒன்றாகும்",
        "བོད་ཡིག་ནི་བོད་པའི་ཡི་གེ་ཡིན།",
        "Русский язык использует кириллицу и пробелы между словами",
        "Η ελληνική γλώσσα έχει τόνους και διαλυτικά ϊ ΰ",
        "Tiếng Việt dùng nhiều dấu thanh như ệ ở ữ",
        "Tie\u{302}\u{301}ng Vie\u{323}\u{302}t",
        "A well-known URL https://example.com/path?x=1 breaks, doesn't it?",
        "family 👨\u{200d}👩\u{200d}👧\u{200d}👦 flag 🇹🇭 skin 👍🏽 done",
        "ภาษาไทยEnglish中文ພາສາລາວ123ข้อความ",
    ];

    #[test]
    fn every_script_breaks_somewhere_and_never_inside_a_cluster() {
        for text in SAMPLES {
            let boundaries = graphemes(text);
            let breaks = line_break_opportunities(text, &boundaries);
            assert!(!breaks.is_empty(), "no opportunity at all in {text}");
            for offset in &breaks {
                assert!(
                    boundaries.contains(offset),
                    "{text}: break at {offset} is inside a cluster"
                );
            }
            assert!(breaks.windows(2).all(|pair| pair[0] < pair[1]));
            assert_eq!(pieces(text).concat(), *text);
        }
    }

    #[test]
    fn a_break_that_is_not_a_cluster_boundary_is_discarded() {
        let text = "ab cd";
        assert_eq!(line_break_opportunities(text, &graphemes(text)), vec![3]);
        assert_eq!(
            line_break_opportunities(text, &[0, 1, 2, 4]),
            Vec::<usize>::new()
        );
    }

    #[test]
    fn thai_and_lao_break_at_words_not_inside_them() {
        assert_eq!(pieces("กิ่งไม้ใหญ่"), ["กิ่ง", "ไม้", "ใหญ่"]);
        assert_eq!(pieces("ພາສາລາວງາມຫຼາຍ"), ["ພາສາ", "ລາວ", "ງາມ", "ຫຼາຍ"]);
    }

    #[test]
    fn korean_breaks_at_spaces_and_chinese_still_breaks_between_characters() {
        assert_eq!(
            pieces("한국어는 띄어쓰기를 사용"),
            ["한국어는 ", "띄어쓰기를 ", "사용"]
        );
        let text = "한국어";
        let raw: Vec<usize> = LineSegmenter::new_auto(LineBreakOptions::default())
            .segment_str(text)
            .filter(|offset| *offset > 0 && *offset < text.len())
            .collect();
        assert!(!raw.is_empty());
        assert_eq!(pieces("中文排版"), ["中", "文", "排", "版"]);
        assert_eq!(pieces("日本語"), ["日", "本", "語"]);
    }

    #[test]
    fn punctuation_stays_with_what_it_closes() {
        let text = "换行，但是";
        let got = pieces(text);
        assert!(got.contains(&"行，"), "{got:?}");
    }
}
