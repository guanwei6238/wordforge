//! 句型的本地偵測：這一句用到了哪些句型、有沒有超過學習者的程度。
//!
//! ## 為什麼需要這個模組
//!
//! 出題的難度一直只有**詞彙**這一半有依據：閱讀靠 `coverage` 實算生詞比例，
//! 翻譯靠 `text::mentions_any` 確認指派的字真的用上了。**句法那一半完全沒有**
//! ——prompt 只寫「句子要自然、日常」，沒有上限、沒有下限，也沒有任何
//! 本地量得到的東西。結果是初學者拿到分詞構句與假設語氣，每題都錯，
//! 而系統對此一無所知。
//!
//! 這個模組補的就是那一半：把「句型」變成**本地跑得動的偵測器**，
//! 於是「這句超出他的程度」跟「這句有沒有真的練到那個句型」都變成
//! 可以在本地驗收的事實，而不是 prompt 裡的一句請求。
//!
//! ## 兩層檢查，缺一不可
//!
//! | 層 | 抓什麼 | 覆蓋範圍 |
//! | --- | --- | --- |
//! | [`PatternSet::over_level`] | 命中「超過上限」的句型 | 只抓**收錄過的**句型 |
//! | [`complexity`] | 句子太長、子句太多 | 對**每一句**都成立 |
//!
//! 只做第一層是假的保護：課綱沒收錄的難句照樣過關。只做第二層又太鈍：
//! 一個短句照樣可以是假設語氣。**兩層一起才說得出「這句在範圍內」**，
//! 而且即使如此，第一層的 recall 仍然有限——這件事要對使用者講清楚，
//! 不要假裝「通過檢查」等於「一定不難」。
//!
//! ## 偵測器自己也要被驗證
//!
//! 每個偵測器都要附 `positive`（一定要命中的例句）。這不是選配：
//! 一條打錯字的 regex 什麼都比對不到，於是「超綱檢查」**永遠通過**，
//! 而畫面上完全正常——跟拿 `strings` 找中文字串一樣，方法自己壞了，
//! 結論卻很有信心。所以 [`compile_detector`] 會實跑控制組，過不了就拒絕。
//!
//! ## 為什麼不用真的 parser
//!
//! 句法分析器（UD parser 之類）判斷子句結構準得多，但要外部執行期、
//! 每個語言各一份模型，而這一層的硬性條件是「不碰 I/O」。regex 加上
//! 一個粗的複雜度指標是本地、確定性、測得起來的，代價是 recall 有限。
//! 那個代價寫在上面那張表裡，也寫在 UI 上。

use std::ops::Range;

use regex::{Regex, RegexBuilder};

/// 使用者／模型寫的 regex 編譯後最多佔多少位元組。
///
/// 這些字串不是我們寫的（匯入的檔案、模型草擬的），所以要有上限：
/// 一條巢狀量詞的 regex 可以編譯成幾百 MB 的程式。`regex` crate 本身
/// 保證線性時間、不會災難性回溯，所以要防的只有記憶體。
const REGEX_SIZE_LIMIT: usize = 1 << 20;

/// 一個偵測器：怎麼在句子裡認出這個句型。
///
/// ## 為什麼 `kind` 不是 `Option<String>` 之類的東西
///
/// 這個專案踩過那個坑：推理強度在 claude 與 codex 上長得不一樣，
/// 硬塞進一個 `effort_flag: Option<String>` 的結果是 codex 那條路永遠壞的。
/// 偵測方式將來一定會不只 regex（詞性序列、依存關係），所以從第一天
/// 就留成有名字的種類——**而且認不得的種類要出聲**，不是默默當成沒有。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Detector {
    /// 目前只認得 `"regex"`。認不得的會在編譯時被回報，不會靜默忽略。
    #[serde(rename = "type", default = "default_kind")]
    pub kind: String,
    /// regex 本身。比對時**不分大小寫**——句首的 `If` 跟句中的 `if`
    /// 是同一件事，要求寫的人自己處理只會製造安靜失效的偵測器。
    pub value: String,
    /// 一定要命中的例句。**至少要有一個**，見模組說明。
    #[serde(default)]
    pub positive: Vec<String>,
    /// 一定不能命中的例句。可以是空的，但強烈建議寫——
    /// 一條「什麼都命中」的 regex 會讓每一句都被判成超綱。
    #[serde(default)]
    pub negative: Vec<String>,
}

fn default_kind() -> String {
    "regex".into()
}

/// 一個句型的定義（這一層只要偵測需要的部分）。
///
/// 完整的定義住在資料庫的 `grammar_def`；這裡刻意只收偵測用得到的欄位，
/// 讓 `wordforge-core` 不必認得資料庫的形狀。
#[derive(Debug, Clone, PartialEq)]
pub struct PatternDef {
    /// 受控識別碼，跟 `grammar_point.point` 對應
    pub point: String,
    /// 給使用者看的名稱
    pub name: String,
    /// 分級刻度上的位置。`None` 表示這個句型沒有分級——
    /// 那時它**不參與上限檢查**（我們不知道它算難還是簡單，
    /// 猜一個等於憑空造出一條規則）。
    pub level_ordinal: Option<i64>,
    /// 顯示用的等級名稱（`"國中七年級"`、`"B1"`）。程式不解讀它。
    pub level: Option<String>,
    pub detectors: Vec<Detector>,
}

/// 一條偵測器沒通過自我檢查。
///
/// `point` 與 `index` 要一起帶：匯入一份 80 條的清單，只說「有一條 regex
/// 壞了」等於沒說。
#[derive(Debug, Clone, PartialEq)]
pub struct DetectorProblem {
    pub point: String,
    pub index: usize,
    pub detail: String,
}

impl std::fmt::Display for DetectorProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}（第 {} 條偵測器）：{}",
            self.point,
            self.index + 1,
            self.detail
        )
    }
}

/// 編譯一條偵測器，並**實際跑一次控制組**。
///
/// 這個函式是整個方案能不能成立的地方。沒有它的話，一條打錯字的 regex
/// 會安靜地讓所有檢查通過——那是這個專案最熟悉的一種壞法：功能看起來
/// 完全正常，只是它從來沒有作用過。
///
/// 拒絕的條件：
///
/// 1. 認不得的 `type`（將來多一種偵測方式時，舊版不會默默當它不存在）
/// 2. regex 編譯不過，或大得離譜
/// 3. **一個 positive 都沒有**——沒有控制組就沒辦法確認它真的會命中
/// 4. positive 沒命中（regex 寫錯了）
/// 5. negative 命中了（regex 寫得太寬）
pub fn compile_detector(detector: &Detector) -> Result<Regex, String> {
    if detector.kind != "regex" {
        return Err(format!(
            "認不得的偵測方式「{}」。目前只支援 \"regex\"；\
             這條先跳過，不然它會被當成「檢查過了」。",
            detector.kind
        ));
    }

    let re = RegexBuilder::new(&detector.value)
        .case_insensitive(true)
        .size_limit(REGEX_SIZE_LIMIT)
        .build()
        .map_err(|e| format!("regex 編譯不過：{e}"))?;

    if detector.positive.is_empty() {
        return Err("沒有 positive 例句。至少要有一句「一定要命中」的例子——\
             沒有控制組的話，一條什麼都比對不到的 regex 會讓檢查永遠通過，\
             而且畫面上看不出任何異狀。"
            .into());
    }

    for example in &detector.positive {
        if !re.is_match(example) {
            return Err(format!(
                "positive 例句沒有命中：「{example}」。這條 regex 抓不到它自己的例子，\
                 表示它實際上什麼都抓不到。"
            ));
        }
    }

    for example in &detector.negative {
        if re.is_match(example) {
            return Err(format!(
                "negative 例句被誤判命中：「{example}」。這條 regex 太寬，\
                 會把不相干的句子都判成這個句型。"
            ));
        }
    }

    Ok(re)
}

/// 編譯好的句型。
#[derive(Debug)]
pub struct CompiledPattern {
    pub point: String,
    pub name: String,
    pub level_ordinal: Option<i64>,
    pub level: Option<String>,
    regexes: Vec<Regex>,
}

impl CompiledPattern {
    /// 這一段文字有沒有用到這個句型，命中的話落在哪個範圍。
    ///
    /// 回傳範圍而不只是布林值：批改時要拿它跟「模型指出的錯誤片段」比對，
    /// 才分得出「句型用錯了」與「句型是對的，錯在別的地方」。
    pub fn find(&self, text: &str) -> Option<Range<usize>> {
        self.regexes
            .iter()
            .filter_map(|re| re.find(text).map(|m| m.start()..m.end()))
            .min_by_key(|r| r.start)
    }

    pub fn matches(&self, text: &str) -> bool {
        self.regexes.iter().any(|re| re.is_match(text))
    }
}

/// 一整組編譯好、自我檢查過的句型。
#[derive(Debug, Default)]
pub struct PatternSet {
    patterns: Vec<CompiledPattern>,
}

impl PatternSet {
    /// 編譯一批定義。**壞掉的偵測器被丟掉並回報，不會讓整批失敗**——
    /// 一份 80 條的清單裡有一條 regex 打錯字，不該讓另外 79 條也用不了。
    ///
    /// 一個句型的偵測器全部壞掉時，那個句型不會進到集合裡：它偵測不到
    /// 任何東西，留著只會讓人以為它在運作。
    pub fn compile(defs: &[PatternDef]) -> (Self, Vec<DetectorProblem>) {
        let mut patterns = Vec::new();
        let mut problems = Vec::new();

        for def in defs {
            let mut regexes = Vec::new();
            for (i, detector) in def.detectors.iter().enumerate() {
                match compile_detector(detector) {
                    Ok(re) => regexes.push(re),
                    Err(detail) => problems.push(DetectorProblem {
                        point: def.point.clone(),
                        index: i,
                        detail,
                    }),
                }
            }
            if regexes.is_empty() {
                continue;
            }
            patterns.push(CompiledPattern {
                point: def.point.clone(),
                name: def.name.clone(),
                level_ordinal: def.level_ordinal,
                level: def.level.clone(),
                regexes,
            });
        }

        (Self { patterns }, problems)
    }

    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    pub fn len(&self) -> usize {
        self.patterns.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &CompiledPattern> {
        self.patterns.iter()
    }

    pub fn get(&self, point: &str) -> Option<&CompiledPattern> {
        self.patterns.iter().find(|p| p.point == point)
    }

    /// 這一段文字用到了哪些句型，各自命中在哪。
    pub fn detect<'a>(&'a self, text: &str) -> Vec<(&'a CompiledPattern, Range<usize>)> {
        self.patterns
            .iter()
            .filter_map(|p| p.find(text).map(|span| (p, span)))
            .collect()
    }

    /// 這一段文字裡**超過上限**的句型。
    ///
    /// 沒有分級的句型（`level_ordinal` 是 `None`）一律不算超綱：
    /// 我們不知道它難不難，猜一個等於憑空造出一條規則，而使用者
    /// 會看到題目被莫名其妙退回去重寫。
    pub fn over_level<'a>(&'a self, text: &str, ceiling: i64) -> Vec<&'a CompiledPattern> {
        self.patterns
            .iter()
            .filter(|p| p.level_ordinal.is_some_and(|o| o > ceiling))
            .filter(|p| p.matches(text))
            .collect()
    }

    /// 上限之內的句型。出題時列給模型看的就是這些。
    pub fn at_or_below(&self, ceiling: i64) -> impl Iterator<Item = &CompiledPattern> {
        self.patterns
            .iter()
            .filter(move |p| p.level_ordinal.is_none_or(|o| o <= ceiling))
    }

    /// 剛好超過上限的那些，由低到高。
    ///
    /// prompt 裡列「不要用這些」比列「只能用這些」有效得多：允許的清單
    /// 可能有上百條，列完就把 prompt 撐爆了，而**最可能不小心用到的**
    /// 永遠是剛好高一級的那幾個。
    pub fn just_above(&self, ceiling: i64, limit: usize) -> Vec<&CompiledPattern> {
        let mut above: Vec<&CompiledPattern> = self
            .patterns
            .iter()
            .filter(|p| p.level_ordinal.is_some_and(|o| o > ceiling))
            .collect();
        above.sort_by_key(|p| (p.level_ordinal, p.point.clone()));
        above.truncate(limit);
        above
    }
}

// ------------------------------------------------------------ 複雜度

/// 一句話的粗略句法複雜度。
///
/// 「粗略」是誠實的形容：這不是句法分析，只是兩個數得出來的量。
/// 它的價值在於**對每一句都成立**——句型偵測只抓收錄過的東西，
/// 而沒收錄的難句正是最需要被擋下來的那些。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Complexity {
    /// 詞數。有空格的語言算詞，沒有空格的（中日文）算字。
    pub words: usize,
    /// 子句標記的個數：從屬連接詞、關係詞、分號。
    ///
    /// **這是「多少個子句」的下界，不是準確值**。沒收錄的語言一律回 0，
    /// 那時只有 `words` 這一層還在作用——少一層過濾，不會誤擋。
    pub clause_markers: usize,
}

/// 子句標記：出現一個就多一個子句的詞。
///
/// 只收**幾乎只用來接子句**的詞。`that` 不在裡面（`that book` 太常見了），
/// `for` 也不在（介系詞用法佔壓倒多數）——誤判會讓好句子被退回去重寫，
/// 而使用者只會看到出題變慢。寧可漏抓。
const ENGLISH_CLAUSE_MARKERS: &[&str] = &[
    // 從屬連接詞
    "because", "although", "though", "unless", "whereas", "while", "since", "whether", "if",
    "until", "before", "after", "once", "as", "than", "so", // 關係詞
    "who", "whom", "whose", "which", "where", "when", "why",
];

/// 這個語言的子句標記。
///
/// 沒收錄的語言回空陣列——`wordlist::is_function_word` 對未知語言回
/// `false` 是同一個約定：**降級成少一層過濾，不是降級成誤判**。
fn clause_markers(lang: &str) -> &'static [&'static str] {
    let l = lang.trim().to_lowercase();
    if l.starts_with("en") {
        return ENGLISH_CLAUSE_MARKERS;
    }
    &[]
}

/// 量一句話的複雜度。
pub fn complexity(text: &str, lang: &str) -> Complexity {
    let tokens = crate::text::tokenize(text);
    let words = if crate::text::joins_with_space(lang) {
        tokens.len()
    } else {
        // 中日文 `tokenize` 會切成單字，詞數沒有意義；用字數當長度。
        // 兩個刻度不一樣，所以上限的預設值也必須由使用者自己調——
        // 這正是它做成設定而不是常數的原因。
        text.chars().filter(|c| !c.is_whitespace()).count()
    };

    let markers = clause_markers(lang);
    let mut clause_markers = tokens
        .iter()
        .filter(|t| markers.contains(&t.as_str()))
        .count();
    // 分號幾乎一定是接一個獨立子句，而且它不是詞元，`tokenize` 看不到
    clause_markers += text.matches(';').count() + text.matches('；').count();

    Complexity {
        words,
        clause_markers,
    }
}

/// 句子難度的上限。`None` 表示這一項不限制。
///
/// 三個欄位都是 `Option`：這整套機制在使用者匯入句型、自己設定之前
/// 應該是**完全不作用**的。預設一個數字等於替所有人決定「什麼叫太難」，
/// 而那件事因人而異——跟 `reading_coverage` 是同一個道理。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DifficultyLimit {
    /// 句型分級的上限（`level_ordinal`）
    pub level_ceiling: Option<i64>,
    /// 一句話最多幾個詞
    pub max_words: Option<usize>,
    /// 一句話最多幾個子句標記
    pub max_clause_markers: Option<usize>,
}

impl DifficultyLimit {
    /// 三項都沒設就代表這套機制沒有開，呼叫端可以整段跳過。
    pub fn is_off(&self) -> bool {
        self.level_ceiling.is_none()
            && self.max_words.is_none()
            && self.max_clause_markers.is_none()
    }
}

/// 一句話超標的地方。
#[derive(Debug, Clone, PartialEq)]
pub enum Exceeds {
    /// 用到了超過上限的句型
    Pattern {
        point: String,
        name: String,
        level: Option<String>,
    },
    /// 太長了
    Words { got: usize, max: usize },
    /// 子句太多
    Clauses { got: usize, max: usize },
}

impl std::fmt::Display for Exceeds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Exceeds::Pattern { name, level, .. } => match level {
                Some(level) => write!(f, "用到了「{name}」（{level}），超過他現在的程度"),
                None => write!(f, "用到了「{name}」，超過他現在的程度"),
            },
            Exceeds::Words { got, max } => {
                write!(f, "這一句有 {got} 個詞，上限是 {max} 個")
            }
            Exceeds::Clauses { got, max } => {
                write!(f, "這一句有 {got} 個子句，上限是 {max} 個")
            }
        }
    }
}

/// 一句話有沒有超出上限，超在哪裡。
///
/// 兩層一起跑：收錄過的句型走 [`PatternSet::over_level`]，
/// 其餘的句子走 [`complexity`]。回傳空陣列**不等於「這句一定不難」**，
/// 只等於「我們量得到的那些都沒超標」。
pub fn exceeds(
    text: &str,
    lang: &str,
    patterns: &PatternSet,
    limit: &DifficultyLimit,
) -> Vec<Exceeds> {
    let mut out = Vec::new();

    if let Some(ceiling) = limit.level_ceiling {
        for pattern in patterns.over_level(text, ceiling) {
            out.push(Exceeds::Pattern {
                point: pattern.point.clone(),
                name: pattern.name.clone(),
                level: pattern.level.clone(),
            });
        }
    }

    if limit.max_words.is_some() || limit.max_clause_markers.is_some() {
        let c = complexity(text, lang);
        if let Some(max) = limit.max_words
            && c.words > max
        {
            out.push(Exceeds::Words { got: c.words, max });
        }
        if let Some(max) = limit.max_clause_markers
            && c.clause_markers > max
        {
            out.push(Exceeds::Clauses {
                got: c.clause_markers,
                max,
            });
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detector(value: &str, positive: &[&str], negative: &[&str]) -> Detector {
        Detector {
            kind: "regex".into(),
            value: value.into(),
            positive: positive.iter().map(|s| s.to_string()).collect(),
            negative: negative.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn def(point: &str, ordinal: i64, value: &str, positive: &[&str]) -> PatternDef {
        PatternDef {
            point: point.into(),
            name: point.into(),
            level_ordinal: Some(ordinal),
            level: Some(format!("第 {ordinal} 級")),
            detectors: vec![detector(value, positive, &[])],
        }
    }

    /// 這條測試存在的理由：一條什麼都比對不到的 regex 會讓「超綱檢查」
    /// 永遠通過，而畫面上完全正常。沒有控制組就沒辦法分辨
    /// 「沒有超綱句型」與「偵測器根本沒在跑」。
    #[test]
    fn a_detector_without_a_control_example_is_rejected() {
        let d = Detector {
            kind: "regex".into(),
            value: r"\bif\b".into(),
            positive: vec![],
            negative: vec![],
        };
        let err = compile_detector(&d).unwrap_err();
        assert!(err.contains("positive"), "{err}");
    }

    #[test]
    fn a_detector_that_misses_its_own_example_is_rejected() {
        // 打錯字：想寫 would 卻寫成 wuold
        let d = detector(
            r"\bif\b[^.?!]*\bwuold\b",
            &["If I had money, I would buy it."],
            &[],
        );
        let err = compile_detector(&d).unwrap_err();
        assert!(err.contains("positive"), "{err}");
    }

    #[test]
    fn a_detector_that_matches_its_counter_example_is_rejected() {
        let d = detector(r"\bwould\b", &["I would buy it."], &["I would buy it."]);
        let err = compile_detector(&d).unwrap_err();
        assert!(err.contains("negative"), "{err}");
    }

    /// 認不得的偵測方式要出聲，不能默默當成沒有——將來多一種偵測方式時，
    /// 舊版必須說得出「我看不懂這條」，否則檢查會安靜地少做一半。
    #[test]
    fn an_unknown_detector_kind_is_reported_not_ignored() {
        let d = Detector {
            kind: "pos-sequence".into(),
            value: "AUX VERB".into(),
            positive: vec!["whatever".into()],
            negative: vec![],
        };
        let err = compile_detector(&d).unwrap_err();
        assert!(err.contains("pos-sequence"), "{err}");
    }

    #[test]
    fn a_good_detector_compiles_and_matches() {
        let d = detector(
            r"\bif\b[^.?!]*\bwould\b",
            &["If I had money, I would buy it."],
            &["I would buy it.", "I don't know if he is here."],
        );
        let re = compile_detector(&d).unwrap();
        assert!(re.is_match("If it rained, we would stay home."));
    }

    /// 句首的 `If` 跟句中的 `if` 是同一件事。要求寫的人自己處理大小寫
    /// 只會製造一堆「看起來有寫、其實抓不到」的偵測器。
    #[test]
    fn matching_ignores_case() {
        let d = detector(r"\bif\b", &["if it rains"], &[]);
        let re = compile_detector(&d).unwrap();
        assert!(re.is_match("If it rains, we stay."));
    }

    /// 壞掉的那一條被丟掉並回報，其餘照樣可用——一份 80 條的清單裡
    /// 有一條打錯字，不該讓另外 79 條也失效。
    #[test]
    fn one_broken_detector_does_not_sink_the_rest() {
        let good = def(
            "conditional",
            3,
            r"\bif\b[^.?!]*\bwould\b",
            &["If I knew, I would say."],
        );
        let broken = PatternDef {
            point: "broken".into(),
            name: "壞的".into(),
            level_ordinal: Some(3),
            level: None,
            detectors: vec![detector(r"[unclosed", &["whatever"], &[])],
        };
        let (set, problems) = PatternSet::compile(&[good, broken]);
        assert_eq!(set.len(), 1);
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].point, "broken");
    }

    /// 偵測器全壞的句型不該留在集合裡：它偵測不到任何東西，
    /// 留著只會讓人以為它在運作。
    #[test]
    fn a_pattern_with_no_working_detector_is_dropped() {
        let broken = PatternDef {
            point: "broken".into(),
            name: "壞的".into(),
            level_ordinal: Some(1),
            level: None,
            detectors: vec![detector(r"\bnope\b", &["這句不含那個字"], &[])],
        };
        let (set, problems) = PatternSet::compile(&[broken]);
        assert!(set.is_empty());
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn over_level_only_flags_patterns_above_the_ceiling() {
        let (set, problems) = PatternSet::compile(&[
            def("present-simple", 1, r"\bI\s+\w+\b", &["I eat rice."]),
            def(
                "conditional-2",
                5,
                r"\bif\b[^.?!]*\bwould\b",
                &["If I knew, I would say."],
            ),
        ]);
        assert!(problems.is_empty());

        let hard = "If I had time, I would help you.";
        assert_eq!(set.over_level(hard, 5).len(), 0, "上限就在那一級，不算超綱");
        let over = set.over_level(hard, 3);
        assert_eq!(over.len(), 1);
        assert_eq!(over[0].point, "conditional-2");
    }

    /// 沒有分級的句型不能被當成超綱。我們不知道它難不難，猜一個等於
    /// 憑空造出一條規則，而使用者只會看到題目一直被退回去重寫。
    #[test]
    fn an_unlevelled_pattern_is_never_over_level() {
        let mut d = def("mystery", 9, r"\bmystery\b", &["a mystery"]);
        d.level_ordinal = None;
        let (set, _) = PatternSet::compile(&[d]);
        assert!(set.over_level("a mystery", 1).is_empty());
        // 但它仍然偵測得到——只是不參與上限判斷
        assert_eq!(set.detect("a mystery").len(), 1);
    }

    #[test]
    fn just_above_lists_the_nearest_levels_first() {
        let (set, _) = PatternSet::compile(&[
            def("a", 9, r"\ba\b", &["a"]),
            def("b", 3, r"\bb\b", &["b"]),
            def("c", 5, r"\bc\b", &["c"]),
        ]);
        let above: Vec<&str> = set
            .just_above(2, 2)
            .iter()
            .map(|p| p.point.as_str())
            .collect();
        assert_eq!(above, vec!["b", "c"], "先列剛好高一級的");
    }

    /// 命中的範圍要拿得到：批改時要靠它分辨「句型用錯了」與
    /// 「句型是對的，錯在別的地方」。
    #[test]
    fn a_match_reports_where_it_landed() {
        let (set, _) = PatternSet::compile(&[def(
            "conditional-2",
            5,
            r"\bif\b[^.?!]*\bwould\b",
            &["If I knew, I would say."],
        )]);
        let text = "Sorry. If I knew, I would say.";
        let span = set.get("conditional-2").unwrap().find(text).unwrap();
        assert_eq!(&text[span], "If I knew, I would");
    }

    #[test]
    fn complexity_counts_words_and_clause_markers() {
        let c = complexity("Because it rained, we stayed home.", "en");
        assert_eq!(c.words, 6);
        assert_eq!(c.clause_markers, 1);

        let simple = complexity("We stayed home.", "en");
        assert_eq!(simple.clause_markers, 0);
    }

    /// 沒收錄的語言降級成「少一層過濾」，不是降級成誤判。
    /// `wordlist::is_function_word` 對未知語言回 false 是同一個約定。
    #[test]
    fn an_unknown_language_still_gets_a_length_but_no_clause_count() {
        let c = complexity("昨日は雨だったので、家にいました。", "ja");
        assert!(c.words > 0, "長度對每個語言都量得到");
        assert_eq!(c.clause_markers, 0, "沒有標記表就不要猜");
    }

    #[test]
    fn nothing_is_checked_when_no_limit_is_set() {
        let (set, _) = PatternSet::compile(&[def(
            "conditional-2",
            5,
            r"\bif\b[^.?!]*\bwould\b",
            &["If I knew, I would say."],
        )]);
        let limit = DifficultyLimit::default();
        assert!(limit.is_off());
        assert!(exceeds("If I knew, I would say.", "en", &set, &limit).is_empty());
    }

    #[test]
    fn both_layers_report_independently() {
        let (set, _) = PatternSet::compile(&[def(
            "conditional-2",
            5,
            r"\bif\b[^.?!]*\bwould\b",
            &["If I knew, I would say."],
        )]);
        let limit = DifficultyLimit {
            level_ceiling: Some(2),
            max_words: Some(5),
            max_clause_markers: Some(0),
        };
        let found = exceeds(
            "If I had known that, I would have told you.",
            "en",
            &set,
            &limit,
        );
        assert_eq!(found.len(), 3, "句型、長度、子句各報一次：{found:?}");
    }

    /// 第二層的存在理由：沒收錄的難句照樣要被擋下來。
    #[test]
    fn a_hard_sentence_nobody_catalogued_still_trips_the_length_limit() {
        let (set, _) = PatternSet::compile(&[]);
        let limit = DifficultyLimit {
            level_ceiling: Some(1),
            max_words: Some(12),
            max_clause_markers: None,
        };
        let sentence = "Having finished the report that his manager had requested, he left the office quietly.";
        let found = exceeds(sentence, "en", &set, &limit);
        assert_eq!(found.len(), 1, "句型清單是空的，靠長度接住");
        assert!(matches!(found[0], Exceeds::Words { .. }));
    }
}
