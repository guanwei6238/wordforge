//! 把教材切成適合塞進 prompt 的段落。
//!
//! ## 為什麼按字元而不是按詞
//!
//! 按詞切需要斷詞，而斷詞是語言特定的——中日文沒有空格，用 Unicode
//! 詞界切出來是一個字一塊。切塊如果依賴斷詞，「換一份字典就能學那個語言」
//! 就會在這裡斷掉。
//!
//! 按字元切對每種語言都成立，代價只是「一塊的資訊量」在不同語言之間
//! 不一樣（中文一個字的資訊量比英文一個字母大）。那個代價可以接受：
//! 塊的大小本來就只是「一次餵給模型多少」的粗略控制。

/// 一塊的目標大小（字元）。
///
/// 1200 個字元大約是英文 200 詞、中文 1200 字，都在「一次出題參考得完」
/// 的範圍內。再大會排擠掉 prompt 裡的其他東西（已知詞樣本、題目要求）。
pub const TARGET_CHARS: usize = 1_200;

/// 一塊最小要多大，太短的段落會併進下一塊。
const MIN_CHARS: usize = 200;

/// 單一段落超過這個長度就得從中間切開。
const HARD_LIMIT: usize = TARGET_CHARS * 2;

/// 把整份文字切成塊。
///
/// 優先在空行（段落邊界）切，因為那是作者自己標出來的語意邊界。
/// 段落太長才退而求其次在句末切。
pub fn split(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();

    for paragraph in paragraphs(text) {
        // 單一段落就超過硬上限：從句末切開，否則一塊會塞爆 prompt
        if paragraph.chars().count() > HARD_LIMIT {
            flush(&mut current, &mut chunks);
            for piece in split_long(&paragraph) {
                push_paragraph(&piece, &mut current, &mut chunks);
            }
            continue;
        }
        push_paragraph(&paragraph, &mut current, &mut chunks);
    }

    flush(&mut current, &mut chunks);
    chunks
}

fn push_paragraph(paragraph: &str, current: &mut String, chunks: &mut Vec<String>) {
    let would_be = current.chars().count() + paragraph.chars().count();
    if !current.is_empty() && would_be > TARGET_CHARS {
        flush(current, chunks);
    }
    if !current.is_empty() {
        current.push_str("\n\n");
    }
    current.push_str(paragraph);
}

fn flush(current: &mut String, chunks: &mut Vec<String>) {
    let text = current.trim();
    if text.is_empty() {
        current.clear();
        return;
    }
    // 太短的尾巴併回上一塊，不要留下只有一行的塊
    if text.chars().count() < MIN_CHARS
        && let Some(last) = chunks.last_mut()
    {
        last.push_str("\n\n");
        last.push_str(text);
        current.clear();
        return;
    }
    chunks.push(text.to_string());
    current.clear();
}

/// 以空行分段。
fn paragraphs(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            if !buf.trim().is_empty() {
                out.push(buf.trim().to_string());
            }
            buf.clear();
        } else {
            if !buf.is_empty() {
                buf.push('\n');
            }
            buf.push_str(line.trim());
        }
    }
    if !buf.trim().is_empty() {
        out.push(buf.trim().to_string());
    }
    out
}

/// 句末的標點。
///
/// 中日文的句號是全形的，跟英文的不同字元——只認 `.` 的話，
/// 一整段中文會被當成一個沒有句子邊界的長句，最後在字元數上硬切。
const SENTENCE_ENDS: &[char] = &[
    '.', '!', '?', '。', '！', '？', '…', '；', ';', '」', '』', '\n',
];

/// 把過長的段落從句末切開；找不到句末就在字元上限硬切。
fn split_long(paragraph: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();

    for sentence in sentences(paragraph) {
        let would_be = current.chars().count() + sentence.chars().count();
        if !current.is_empty() && would_be > TARGET_CHARS {
            out.push(std::mem::take(&mut current).trim().to_string());
        }
        // 單一句子就超過上限（沒有標點的長文），只能硬切
        if sentence.chars().count() > HARD_LIMIT {
            out.extend(hard_split(&sentence));
            continue;
        }
        current.push_str(&sentence);
    }

    let tail = current.trim();
    if !tail.is_empty() {
        out.push(tail.to_string());
    }
    out.retain(|s| !s.is_empty());
    out
}

/// 依句末標點切句，標點留在句尾。
fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    for ch in text.chars() {
        buf.push(ch);
        if SENTENCE_ENDS.contains(&ch) {
            out.push(std::mem::take(&mut buf));
        }
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

/// 最後手段：照字元數切。用 `chars` 而不是位元組，切在多位元組字元
/// 中間會產生無效的 UTF-8。
fn hard_split(text: &str) -> Vec<String> {
    text.chars()
        .collect::<Vec<_>>()
        .chunks(TARGET_CHARS)
        .map(|c| c.iter().collect::<String>().trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn para(n: usize, filler: &str) -> String {
        (0..n).map(|_| filler).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn short_text_stays_in_one_chunk() {
        let chunks = split("First paragraph.\n\nSecond paragraph.");
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].contains("First"));
        assert!(chunks[0].contains("Second"));
    }

    #[test]
    fn empty_text_yields_nothing() {
        assert!(split("").is_empty());
        assert!(split("   \n\n  \n").is_empty());
    }

    /// 段落邊界是作者標出來的語意邊界，能不切就不切。
    #[test]
    fn chunks_prefer_paragraph_boundaries() {
        let text = format!("{}\n\n{}", para(300, "alpha"), para(300, "beta"));
        let chunks = split(&text);
        assert!(chunks.len() >= 2);
        for chunk in &chunks {
            let mixed = chunk.contains("alpha") && chunk.contains("beta");
            assert!(!mixed, "在段落中間切開了");
        }
    }

    #[test]
    fn every_chunk_stays_near_the_target_size() {
        let text = (0..40)
            .map(|i| format!("Paragraph number {i}. {}", para(30, "word")))
            .collect::<Vec<_>>()
            .join("\n\n");
        let chunks = split(&text);
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(
                chunk.chars().count() <= HARD_LIMIT,
                "有一塊太大：{}",
                chunk.chars().count()
            );
        }
    }

    /// 中日文的句號是全形的。只認 `.` 的話一整段中文會被硬切在字元上限。
    #[test]
    fn japanese_splits_on_its_own_sentence_marks() {
        let sentence = "これはとても長い文章です。";
        let text = sentence.repeat(400);
        let chunks = split(&text);

        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(
                chunk.ends_with('。') || chunk == chunks.last().unwrap(),
                "沒有切在句末：{}",
                &chunk[chunk.len().saturating_sub(20)..]
            );
        }
    }

    /// 沒有任何標點的長文也不能讓程式卡住或產生無效 UTF-8。
    #[test]
    fn text_without_punctuation_is_hard_split_safely() {
        let text = "字".repeat(HARD_LIMIT * 3);
        let chunks = split(&text);
        assert!(chunks.len() > 1);
        let total: usize = chunks.iter().map(|c| c.chars().count()).sum();
        assert_eq!(total, HARD_LIMIT * 3, "硬切不能掉字");
    }

    /// 只有一行的尾巴要併回去，不然檢索時會撈到沒有上下文的碎片。
    #[test]
    fn a_tiny_tail_is_merged_into_the_previous_chunk() {
        let text = format!("{}\n\nend.", para(400, "word"));
        let chunks = split(&text);
        assert!(chunks.last().unwrap().contains("end."));
        assert!(
            chunks.last().unwrap().chars().count() > MIN_CHARS,
            "尾巴沒有被併回去"
        );
    }
}
