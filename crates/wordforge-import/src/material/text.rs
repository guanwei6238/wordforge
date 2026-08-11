//! 從各種檔案格式抽出純文字。
//!
//! 這一層刻意不知道任何語言的事：進來是檔案，出去是純文字，
//! 中間沒有斷詞、沒有詞形還原、沒有語言判斷。載入日文小說跟載入
//! 英文課本走的是同一條路。

use std::io::Read;
use std::path::Path;

use crate::ImportError;

type Result<T> = std::result::Result<T, ImportError>;

/// 支援的教材格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterialFormat {
    /// 純文字
    Text,
    /// EPUB 電子書
    Epub,
    /// PDF（只讀得到文字層；掃描檔沒有文字層，讀不到）
    Pdf,
    /// SubRip / WebVTT 字幕
    Subtitle,
    /// 單一 HTML 檔
    Html,
}

impl MaterialFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            MaterialFormat::Text => "text",
            MaterialFormat::Epub => "epub",
            MaterialFormat::Pdf => "pdf",
            MaterialFormat::Subtitle => "subtitle",
            MaterialFormat::Html => "html",
        }
    }

    /// 依副檔名猜格式。猜不到就當純文字——最壞的情況是抽出一堆亂碼，
    /// 使用者看得到，總比直接拒絕匯入好。
    pub fn from_path(path: &Path) -> Self {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_lowercase();
        match ext.as_str() {
            "epub" => MaterialFormat::Epub,
            "pdf" => MaterialFormat::Pdf,
            "srt" | "vtt" => MaterialFormat::Subtitle,
            "html" | "htm" | "xhtml" => MaterialFormat::Html,
            _ => MaterialFormat::Text,
        }
    }
}

/// 從檔案抽出純文字。
pub fn extract(path: &Path, format: MaterialFormat) -> Result<String> {
    match format {
        MaterialFormat::Text => read_utf8_lossy(path),
        MaterialFormat::Subtitle => Ok(strip_subtitle(&read_utf8_lossy(path)?)),
        MaterialFormat::Html => Ok(strip_markup(&read_utf8_lossy(path)?)),
        MaterialFormat::Epub => extract_epub(path),
        MaterialFormat::Pdf => extract_pdf(path),
    }
}

/// 讀檔並以 UTF-8 解讀，壞掉的位元組用替代字元帶過。
///
/// 不用 `read_to_string`：使用者的教材可能有幾個壞位元組，
/// 整份拒絕匯入太嚴苛了。
fn read_utf8_lossy(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn extract_pdf(path: &Path) -> Result<String> {
    // pdf-extract 在遇到怪 PDF 時會 panic 而不是回錯，
    // 這裡把 panic 攔下來變成正常錯誤，不然整個 App 會被帶走
    let path = path.to_path_buf();
    let text = std::panic::catch_unwind(move || pdf_extract::extract_text(&path))
        .map_err(|_| ImportError::Parse("這個 PDF 解析不了（可能是加密或結構異常）".into()))?
        .map_err(|e| ImportError::Parse(format!("PDF 解析失敗：{e}")))?;

    if text.trim().is_empty() {
        return Err(ImportError::Parse(
            "這個 PDF 沒有文字層，抽不出東西。掃描的書需要先做 OCR。".into(),
        ));
    }
    Ok(text)
}

/// EPUB 就是一個 zip，裡面是 XHTML。
///
/// 這裡刻意不去讀 `content.opf` 的閱讀順序，而是照檔名排序：
/// 出題只需要內容，章節順序錯了不影響。少一層解析就少一種壞法。
fn extract_epub(path: &Path) -> Result<String> {
    let file = std::fs::File::open(path)?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| ImportError::Parse(format!("EPUB 打不開：{e}")))?;

    let mut names: Vec<String> = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().map(|f| f.name().to_string()))
        .filter(|n| {
            let lower = n.to_lowercase();
            lower.ends_with(".xhtml") || lower.ends_with(".html") || lower.ends_with(".htm")
        })
        .collect();
    names.sort();

    if names.is_empty() {
        return Err(ImportError::Parse("EPUB 裡找不到任何內容檔".into()));
    }

    let mut out = String::new();
    for name in names {
        let Ok(mut entry) = zip.by_name(&name) else {
            continue;
        };
        let mut raw = Vec::new();
        if entry.read_to_end(&mut raw).is_err() {
            continue;
        }
        let chapter = strip_markup(&String::from_utf8_lossy(&raw));
        if !chapter.trim().is_empty() {
            out.push_str(&chapter);
            out.push_str("\n\n");
        }
    }

    if out.trim().is_empty() {
        return Err(ImportError::Parse("EPUB 抽不出文字".into()));
    }
    Ok(out)
}

/// 去掉 HTML/XHTML 標記，只留文字。
///
/// 用 pull parser 而不是正規表示式：`<p title="a > b">` 這種東西
/// 正規表示式會切錯，而教材裡什麼都有。
fn strip_markup(source: &str) -> String {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_str(source);
    let config = reader.config_mut();
    config.check_end_names = false;
    config.trim_text(false);

    let mut out = String::new();
    let mut skip_depth = 0usize;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = e.name();
                let tag = String::from_utf8_lossy(name.as_ref()).to_lowercase();
                // script 與 style 的內容不是給人讀的
                if tag == "script" || tag == "style" {
                    skip_depth += 1;
                } else if is_block(&tag) {
                    push_break(&mut out);
                }
            }
            Ok(Event::End(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_lowercase();
                if (tag == "script" || tag == "style") && skip_depth > 0 {
                    skip_depth -= 1;
                } else if is_block(&tag) {
                    push_break(&mut out);
                }
            }
            Ok(Event::Text(e)) if skip_depth == 0 => {
                // unescape 會處理 &amp; 與 &#233;，但 HTML 專屬的具名實體
                // （&nbsp; 之類）不是 XML 的一部分，會回錯；那時退回原文，
                // 少解一個實體總比整段文字消失好
                if let Ok(text) = e.decode() {
                    out.push_str(text.as_ref());
                }
            }
            // quick-xml 把 `&amp;` 這種實體參照當成獨立事件，不含在文字裡。
            // 不處理的話 café 會變成 caf、A&B 會變成 AB——教材的字直接少掉。
            Ok(Event::GeneralRef(e)) if skip_depth == 0 => {
                if let Ok(name) = e.decode() {
                    out.push_str(&resolve_entity(name.as_ref()));
                }
            }
            Ok(Event::CData(e)) if skip_depth == 0 => {
                out.push_str(&String::from_utf8_lossy(&e));
            }
            Ok(Event::Eof) => break,
            // 教材的 HTML 常常是壞的；壞在哪就跳過哪，不要整份放棄
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    normalize_blank_lines(&out)
}

/// 把 `&` 與 `;` 中間的東西還原成字元。
///
/// 認不出來的原樣寫回 `&name;`：教材裡什麼都有，猜錯不如不猜，
/// 至少使用者看得出來原本是什麼。
fn resolve_entity(name: &str) -> String {
    if let Some(digits) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
        return u32::from_str_radix(digits, 16)
            .ok()
            .and_then(char::from_u32)
            .map(String::from)
            .unwrap_or_else(|| format!("&{name};"));
    }
    if let Some(digits) = name.strip_prefix('#') {
        return digits
            .parse::<u32>()
            .ok()
            .and_then(char::from_u32)
            .map(String::from)
            .unwrap_or_else(|| format!("&{name};"));
    }
    quick_xml::escape::resolve_predefined_entity(name)
        .or_else(|| resolve_common_html_entity(name))
        .map(String::from)
        .unwrap_or_else(|| format!("&{name};"))
}

/// EPUB 裡常見、但不屬於 XML 預定義的具名實體。
///
/// quick-xml 有完整的 HTML5 實體表，但那個 `match` 長到會讓編譯時間
/// 多十秒以上（上游自己標註了這件事）。教材裡真正會出現的就這些，
/// 手寫一張表換回編譯速度。認不出來的會原樣保留，不會靜靜掉字。
fn resolve_common_html_entity(name: &str) -> Option<&'static str> {
    Some(match name {
        "nbsp" => "\u{a0}",
        "ensp" => "\u{2002}",
        "emsp" => "\u{2003}",
        "thinsp" => "\u{2009}",
        "ndash" => "–",
        "mdash" => "—",
        "hellip" => "…",
        "lsquo" => "\u{2018}",
        "rsquo" => "\u{2019}",
        "ldquo" => "\u{201c}",
        "rdquo" => "\u{201d}",
        "laquo" => "«",
        "raquo" => "»",
        "bull" => "•",
        "middot" => "·",
        "copy" => "©",
        "reg" => "®",
        "trade" => "™",
        "deg" => "°",
        "times" => "×",
        "shy" => "\u{ad}",
        _ => return None,
    })
}

/// 這個標籤會不會造成換行。段落邊界要留著，切塊時要靠它。
fn is_block(tag: &str) -> bool {
    matches!(
        tag,
        "p" | "div"
            | "br"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "li"
            | "tr"
            | "blockquote"
            | "section"
            | "article"
            | "pre"
    )
}

fn push_break(out: &mut String) {
    if !out.ends_with('\n') {
        out.push('\n');
    }
}

/// 去掉字幕的序號與時間軸，只留台詞。
///
/// SubRip 與 WebVTT 的差別只在有沒有 `WEBVTT` 檔頭與時間格式，
/// 兩種都靠「這一行長得像時間軸嗎」來判斷，不必分開處理。
fn strip_subtitle(source: &str) -> String {
    let mut out = String::new();
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            push_break(&mut out);
            push_break_hard(&mut out);
            continue;
        }
        if trimmed == "WEBVTT" || trimmed.starts_with("NOTE ") {
            continue;
        }
        // 時間軸：`00:00:01,000 --> 00:00:04,000`
        if trimmed.contains("-->") {
            continue;
        }
        // 純數字的序號行
        if trimmed.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        // 字幕常有 <i> 之類的標記
        out.push_str(&strip_markup(trimmed));
        out.push('\n');
    }
    normalize_blank_lines(&out)
}

fn push_break_hard(out: &mut String) {
    if !out.ends_with("\n\n") {
        out.push('\n');
    }
}

/// 把三行以上的空行壓成兩行，並去掉行尾空白。
///
/// 切塊靠空行判斷段落，所以這一步不是美觀問題：
/// PDF 抽出來常常每行後面都有空行，不壓的話每個「段落」只有一句話。
fn normalize_blank_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0usize;

    for line in text.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim().is_empty() {
            blank_run += 1;
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
            if blank_run > 0 {
                out.push('\n');
            }
        }
        blank_run = 0;
        out.push_str(trimmed);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_is_guessed_from_the_extension() {
        assert_eq!(
            MaterialFormat::from_path(Path::new("book.epub")),
            MaterialFormat::Epub
        );
        assert_eq!(
            MaterialFormat::from_path(Path::new("SHOW.S01E01.SRT")),
            MaterialFormat::Subtitle
        );
        assert_eq!(
            MaterialFormat::from_path(Path::new("課本.pdf")),
            MaterialFormat::Pdf
        );
        assert_eq!(
            MaterialFormat::from_path(Path::new("notes")),
            MaterialFormat::Text,
            "猜不到就當純文字，不要拒絕匯入"
        );
    }

    #[test]
    fn markup_is_stripped_but_paragraphs_survive() {
        let html = "<html><body><h1>Title</h1><p>First line.</p><p>Second line.</p></body></html>";
        let text = strip_markup(html);
        assert!(text.contains("Title"));
        assert!(text.contains("First line."));
        assert!(
            text.contains("First line.\n\nSecond line.") || text.contains("First line.\nSecond"),
            "段落邊界要留著，切塊靠它：{text:?}"
        );
        assert!(!text.contains('<'));
    }

    #[test]
    fn script_and_style_content_is_dropped() {
        let html = "<p>Keep</p><script>var x = 1;</script><style>p{color:red}</style>";
        let text = strip_markup(html);
        assert!(text.contains("Keep"));
        assert!(!text.contains("var x"));
        assert!(!text.contains("color:red"));
    }

    /// 教材的 HTML 常常是壞的，不能因為一個沒關的標籤就整份放棄。
    #[test]
    fn broken_markup_still_yields_text() {
        let text = strip_markup("<p>before<p>after");
        assert!(text.contains("before"));
        assert!(text.contains("after"));
    }

    /// 實體不處理的話 café 會變成 caf——教材的字直接少掉。
    #[test]
    fn entities_are_decoded() {
        let text = strip_markup("<p>caf&#233; &amp; caf&#xE9; &nbsp;bar</p>");
        assert!(text.contains("café"), "{text:?}");
        assert_eq!(text.matches("café").count(), 2, "十進位與十六進位都要認");
        assert!(text.contains('&'), "&amp; 要還原成 &：{text:?}");
    }

    #[test]
    fn an_unknown_entity_is_kept_as_written() {
        let text = strip_markup("<p>a &notarealentity; b</p>");
        assert!(
            text.contains("&notarealentity;"),
            "猜不出來就別猜：{text:?}"
        );
    }

    #[test]
    fn subtitle_timecodes_and_indices_are_dropped() {
        let srt = "1\n00:00:01,000 --> 00:00:04,000\nHello there.\n\n\
                   2\n00:00:05,000 --> 00:00:07,000\n<i>How are you?</i>\n";
        let text = strip_subtitle(srt);
        assert!(text.contains("Hello there."));
        assert!(text.contains("How are you?"));
        assert!(!text.contains("-->"));
        assert!(!text.contains("00:00"));
        assert!(!text.contains("<i>"));
    }

    #[test]
    fn webvtt_header_is_dropped() {
        let vtt = "WEBVTT\n\n00:01.000 --> 00:04.000\nこんにちは。\n";
        let text = strip_subtitle(vtt);
        assert_eq!(text.trim(), "こんにちは。", "字幕處理不能只對英文成立");
    }

    /// PDF 抽出來常常每行後面都有空行，不壓的話段落全碎掉。
    #[test]
    fn runs_of_blank_lines_collapse() {
        let text = normalize_blank_lines("a\n\n\n\n\nb\n   \nc");
        assert_eq!(text, "a\n\nb\n\nc");
    }
}
