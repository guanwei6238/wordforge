//! 教學用 prompt 模板。
//!
//! ## 為什麼 prompt 不是唯一保險
//!
//! 「請把生詞控制在 5% 以內」這種要求，模型只會**大致**遵守。
//! 真正的保證來自產生之後的本地驗收：用 `wordforge_core::coverage::analyze`
//! 算出實際覆蓋率，落在目標帶之外就帶著具體數字重試。
//! 這裡的 prompt 負責提高一次就過的機率，驗收迴圈負責保證品質。
//!
//! ## 已知詞不能全塞進 prompt
//!
//! 學到中級就有五千到一萬個已知詞，全列出來會吃掉整個 context。
//! 因此改為傳遞：詞彙量級距、CEFR 等級、抽樣詞彙、以及**明確的新詞白名單**。

use crate::{ChatRequest, Message};

/// 文法點標籤的規則。
///
/// 有受控清單的語言（目前只有英文）就把清單列出來並禁止其他寫法——
/// 否則同一個文法點會散成 tense / past tense / Verb Tense 好幾個標籤。
///
/// 沒有清單的語言只能請它自己保持一致。這是誠實的作法：
/// 硬套英文的 articles、gerund-infinitive 去標日文的錯誤只會產生垃圾資料。
fn grammar_point_rule(target_lang: &str) -> String {
    match wordforge_core::grammar_points::prompt_list(target_lang) {
        Some(list) => format!(
            "**只能從下面這份清單挑一個**：{list}。\n\
             清單以外的說法一律不接受——這些標籤會累積成長期的弱點紀錄，\n\
             每次換一種寫法就會被當成不同的問題。真的都不適用時就省略這個欄位。"
        ),
        None => "請用一致的英文術語命名（例如 tense、word-order）；\n\
             同一種問題每次都要用同一個標籤，否則無法累積成弱點紀錄。"
            .to_string(),
    }
}

/// 產生閱讀理解的規格。
#[derive(Debug, Clone)]
pub struct ReadingSpec<'a> {
    /// 學習的目標語言，如 "English"
    pub target_lang: &'a str,
    /// 學習者母語，用於題目說明與翻譯，如 "繁體中文"
    pub native_lang: &'a str,
    /// 文章長度（詞數）
    pub word_count: usize,
    /// 目標已知詞覆蓋率，通常 0.95~0.98
    pub target_coverage: f64,
    /// 學習者的已知詞總量，讓模型抓得到程度
    pub known_word_count: usize,
    /// CEFR 等級，若已知
    pub cefr: Option<&'a str>,
    /// 已知詞抽樣，讓模型感受實際用字範圍
    pub known_sample: &'a [String],
    /// 這篇文章要教的新詞（白名單，除此之外不應出現生詞）。
    ///
    /// 這些必須是學習者**還不會**的字，否則整篇沒有東西可學：
    /// 實測拿「今天到期的複習字」來填，產出的文章覆蓋率 99%，
    /// 遠高於目標的 96%——90% 法則的重點正是那不足 10%。
    pub target_words: &'a [String],
    /// 順便複習到的字。這些他已經會，不佔生詞預算。
    pub review_words: &'a [String],
    pub topic: Option<&'a str>,
    /// 自訂教材摘錄。有值時模型只能用這份材料的內容與用字。
    pub material_excerpt: Option<&'a str>,
    pub question_count: usize,
}

impl<'a> ReadingSpec<'a> {
    /// 每個新詞應該在文章中出現幾次，才有足夠上下文可以推敲。
    pub const REPEATS_PER_NEW_WORD: usize = 2;

    /// 依覆蓋率換算允許的生詞詞元數。
    pub fn unknown_budget(&self) -> usize {
        wordforge_core::coverage::unknown_token_budget(self.word_count, self.target_coverage)
    }
}

/// `option_notes` 的規則，三種選擇題共用。
///
/// ## 為什麼要逐個選項寫
///
/// 選擇題在本地判分，模型從頭到尾沒看過學習者選了什麼，所以
/// 「你選的那個為什麼不行」這件事**只能在出題時先備好**——
/// 每個選項各寫一句，判分時挑他按的那一句出來。
///
/// 這比批改時再打一次模型好：不多花一次呼叫、不用等 CLI 冷啟動，
/// 而且重做同一份題目時解說還在。
fn option_notes_rule(native_lang: &str) -> String {
    format!(
        "option_notes 要跟 options **一樣長、一樣順序**，一個選項一句話：\n\
         - 正確的那個：為什麼它成立（依據文章哪裡、哪個文法規則）。\n\
         - 錯的那些：**針對這個選項本身**講它錯在哪。「因為正確答案是 X」\n\
           不算——那沒有解釋他為什麼會被這個選項騙到。要講的是這個選項\n\
           哪裡不成立：意思不對？時態不對？文章沒這樣說？搭配詞不能這樣用？\n\
         每句話用{native_lang}，控制在一兩句，不要重複整題的 explanation。\n\
         提到別的選項時要引用它的內容，不要寫「選項 B」或「第二個」——\n\
         選項的順序會被系統重新排過，那樣寫出來會對不上。"
    )
}

/// 一份閱讀測驗裡各題的難度。
///
/// 不指定的話模型會出一整份同一個難度的題目——通常全是「在第幾段找得到」
/// 的送分題，做完不知道自己讀懂了沒有；偶爾反過來全是推論題，
/// 那又變成在考智力測驗而不是閱讀。
///
/// 循環使用這個樣式：題數多的時候比例維持不變。
pub const QUESTION_DIFFICULTIES: &[(&str, &str)] = &[
    ("easy", "答案在文章裡明講了，找到那一句就能答"),
    (
        "medium",
        "要整合兩個以上的地方，或從上下文推出沒有明講的關係",
    ),
    ("hard", "要推論作者的態度、言外之意，或把整篇的主旨講出來"),
    ("medium", "考一個新詞在這個語境裡的意思，選項要像但不對"),
];

/// 這份測驗每一題各要多難。
fn difficulty_plan(question_count: usize) -> String {
    (0..question_count)
        .map(|i| {
            let (level, what) = QUESTION_DIFFICULTIES[i % QUESTION_DIFFICULTIES.len()];
            format!("   第 {} 題：{level}——{what}\n", i + 1)
        })
        .collect()
}

/// 閱讀理解出題。
pub fn reading_comprehension(spec: &ReadingSpec) -> ChatRequest {
    let system = format!(
        "你是一位專業的{target}教師，專長是為學習者設計「可理解輸入」教材。\n\
         你的文章必須符合 i+1 原則：絕大部分用學習者已經會的字，只帶入少量新詞，\n\
         且新詞要能從上下文推敲出意思。\n\
         你只輸出 JSON，不輸出任何其他文字。",
        target = spec.target_lang
    );

    let mut prompt = String::new();

    prompt.push_str(&format!(
        "# 學習者程度\n\
         - 已掌握約 {known} 個{target}單字{cefr}\n\
         - 已知詞抽樣（僅供你感受用字範圍，不必全用）：{sample}\n\n",
        known = spec.known_word_count,
        target = spec.target_lang,
        cefr = spec
            .cefr
            .map(|c| format!("，程度約 CEFR {c}"))
            .unwrap_or_default(),
        sample = spec.known_sample.join("、"),
    ));

    prompt.push_str(&format!(
        "# 硬性要求\n\
         1. 文章長度約 {words} 個詞（±10%）。\n\
         2. 學習者不認識的詞元不可超過 {budget} 個，也就是已知詞覆蓋率至少 {cov:.0}%。\n\
         3. 唯一允許出現的新詞是下列白名單：{targets}\n\
            每個新詞請自然地出現 {repeats} 次以上，並讓上下文足以推敲其意思。\n\
            **白名單裡的字盡量全部用到**——少用一個，這篇就少教一個字。\n\
            唯一的例外：某個字如果粗俗、冒犯，或明顯不適合出現在\n\
            學習教材裡，就跳過它，不要為了湊數硬寫進去。\n\
         4. 白名單以外，不要使用專有名詞、縮寫、俚語或罕見字。\n\
         5. 文章要有完整的起承轉合，不要像單字例句的拼貼。\n\
         6. **題目與選項一律用{target}寫**，因為那是他要練的語言；\n\
            只有 explanation 與 gloss 用{native}。\n\n",
        words = spec.word_count,
        target = spec.target_lang,
        native = spec.native_lang,
        budget = spec.unknown_budget(),
        cov = spec.target_coverage * 100.0,
        targets = if spec.target_words.is_empty() {
            "（無，這次只做複習）".to_string()
        } else {
            spec.target_words.join("、")
        },
        repeats = ReadingSpec::REPEATS_PER_NEW_WORD,
    ));

    if !spec.review_words.is_empty() {
        prompt.push_str(&format!(
            "# 順便複習\n\
             這些字他已經學過、今天剛好該複習。能自然帶進去就帶，\n\
             但不要為了硬塞而讓句子變得奇怪，也不要因此擠掉上面的新詞：\n{words}\n\n",
            words = spec.review_words.join("、"),
        ));
    }

    if let Some(topic) = spec.topic {
        prompt.push_str(&format!("# 主題\n{topic}\n\n"));
    }

    if let Some(excerpt) = spec.material_excerpt {
        prompt.push_str(&format!(
            "# 指定教材\n\
             以下是學習者指定的教材內容。你的文章與題目**只能**依據這份材料的\n\
             主題、情境與用字，不可引入教材以外的知識點：\n\
             ---\n{excerpt}\n---\n\n"
        ));
    }

    prompt.push_str(&format!(
        "# 出題\n\
         共出 {n} 題選擇題，題目要考理解而不是找關鍵字。\n\
         **每一題的難度不同**，照這個分配：\n{plan}\n\
         # 出完題之後，回頭檢查文章\n\
         為了塞進指定的新詞，文章很容易寫出前後兜不起來的句子。\n\
         輸出之前一定要自己讀一遍，確認：\n\
         - 每一段接得上上一段，代名詞找得到它指的是誰\n\
         - 時態與人稱一致，沒有中途換掉\n\
         - 沒有只為了用掉某個新詞而硬插進去、拿掉也不影響的句子\n\
         - 每一題的答案在文章裡真的成立，而且只有一個選項對\n\
         有任何一項不成立就把文章改好再輸出，不要輸出你自己都覺得卡的版本。\n\n\
         # 輸出格式\n\
         只輸出這個 JSON 物件：\n\
         {{\n\
         \x20 \"title\": \"文章標題（用{target}）\",\n\
         \x20 \"passage\": \"文章內容\",\n\
         \x20 \"translation\": \"整篇文章的{native}翻譯，逐段對應，讓他讀完可以自己對照\",\n\
         \x20 \"new_words\": [{{\"word\": \"新詞\", \"gloss\": \"{native}解釋\", \"line_hint\": \"可推敲出意思的那句話\"}}],\n\
         \x20 \"questions\": [{{\"question\": \"問題（用{target}）\", \
         \"options\": [\"用{target}寫的四個選項\"], \"answer_index\": 0, \
         \"option_notes\": [\"每個選項各一句{native}說明\"], \
         \"difficulty\": \"easy｜medium｜hard\", \
         \"explanation\": \"以{native}整體說明這一題在考什麼\"}}]\n\
         }}\n\
         {notes}",
        native = spec.native_lang,
        target = spec.target_lang,
        n = spec.question_count,
        plan = difficulty_plan(spec.question_count),
        notes = option_notes_rule(spec.native_lang),
    ));

    ChatRequest {
        system: Some(system),
        messages: vec![Message::user(prompt)],
        json_only: true,
    }
}

/// 產生克漏字的規格。
///
/// 跟 `ReadingSpec` 一樣做成結構而不是一長串參數：這種
/// 「六個 &str 加兩個 Option」的簽章，呼叫端很容易把兩個語言傳反，
/// 而傳反的樣子是「題目語言不對」，跑起來完全不會報錯。
#[derive(Debug, Clone, Copy)]
pub struct ClozeSpec<'a> {
    /// 學習的目標語言，如 "English"
    pub target_lang: &'a str,
    /// 學習者母語，用於解說與翻譯
    pub native_lang: &'a str,
    /// 短文長度（詞數）
    pub word_count: usize,
    /// 學習者的已知詞總量，讓模型抓得到程度
    pub known_word_count: usize,
    /// 已知詞抽樣，讓模型感受實際用字範圍
    pub known_sample: &'a [String],
    /// 要挖成空格的字。這些是他**已經學過**的字，不是生詞——
    /// 克漏字考的是想不想得起來，放生詞會變成考閱讀。
    pub blank_words: &'a [String],
    pub topic: Option<&'a str>,
    /// 自訂教材摘錄。有值時模型只能用這份材料的內容與用字。
    pub material_excerpt: Option<&'a str>,
}

/// 克漏字出題。
///
/// ## 跟閱讀測驗的分工
///
/// 閱讀測驗是**輸入**：讀一篇有生詞的文章，考的是看不看得懂。
/// 克漏字是**取用**：文章用他已經會的字寫，把該複習的字挖掉，
/// 考的是在情境裡想不想得起來那個字。所以這裡不放生詞，
/// 挖掉的都是今天該複習的字——填對一次就等於複習了一次。
///
/// 先前選「克漏字」拿到的是閱讀測驗，連存進資料庫的題型都寫成 reading。
pub fn cloze_passage(spec: &ClozeSpec) -> ChatRequest {
    let ClozeSpec {
        target_lang,
        native_lang,
        word_count,
        known_word_count,
        known_sample,
        blank_words,
        topic,
        material_excerpt,
    } = *spec;

    let system = format!(
        "你是一位{target}教師，正在出克漏字練習。\n\
         你只輸出 JSON，不輸出任何其他文字。",
        target = target_lang
    );

    let n = blank_words.len();
    let mut prompt = format!(
        "# 學習者程度\n\
         - 已掌握約 {known} 個{target}單字\n\
         - 已知詞抽樣（僅供你感受用字範圍）：{sample}\n\n\
         # 硬性要求\n\
         1. 寫一篇約 {words} 個詞的{target}短文，內容連貫、有頭有尾。\n\
         2. 除了要挖掉的字以外，**只用學習者已經會的字**。\n\
            克漏字考的是「想不想得起來這個字」，不是「看不看得懂這篇」；\n\
            旁邊出現生詞的話，答不出來就分不清是哪個原因。\n\
         3. 下面這 {n} 個字各在文章裡用剛好一次，並且把它挖成空格：\n{words_list}\n\
            空格寫成 {{{{1}}}}、{{{{2}}}}… 依序編號，編號要跟 items 的順序一致。\n\
            **不要跳號、不要重複，也不要出現沒有對應題目的空格。**\n\
         4. 每一格的空缺處要有足夠線索能推出答案：前後文、搭配詞、時態。\n\
            四個選項都要是同一個詞類、看起來都放得進去，只有一個真的對。\n\
            拿另外三個要挖的字互相當選項是不行的——那樣一格答錯會連累好幾格。\n\n",
        known = known_word_count,
        target = target_lang,
        sample = known_sample.join("、"),
        words = word_count,
        n = n,
        words_list = blank_words.join("、"),
    );

    if let Some(topic) = topic {
        prompt.push_str(&format!("# 主題\n{topic}\n\n"));
    }

    if let Some(excerpt) = material_excerpt {
        prompt.push_str(&format!(
            "# 指定教材\n\
             短文的情境、用字與句型**只能**取材自以下內容：\n---\n{excerpt}\n---\n\n"
        ));
    }

    prompt.push_str(&format!(
        "# 輸出格式\n\
         只輸出這個 JSON 物件：\n\
         {{\n\
         \x20 \"title\": \"標題（用{target}）\",\n\
         \x20 \"passage\": \"挖好空格的短文\",\n\
         \x20 \"translation\": \"整篇的{native}翻譯，空格處填上正確答案再翻\",\n\
         \x20 \"items\": [{{\"options\": [\"四個{target}選項\"], \"answer_index\": 0, \
         \"option_notes\": [\"每個選項各一句{native}說明\"], \
         \"explanation\": \"用{native}說明這一格在考什麼\"}}]\n\
         }}\n\
         items 要剛好 {n} 題，第 k 題對應 {{{{k}}}} 那一格。\n\
         {notes}",
        target = target_lang,
        native = native_lang,
        n = n,
        notes = option_notes_rule(native_lang),
    ));

    ChatRequest {
        system: Some(system),
        messages: vec![Message::user(prompt)],
        json_only: true,
    }
}

/// AI 對話練習的 system prompt。
///
/// 重點在「不要用學習者看不懂的字」與「糾錯但不打斷對話」之間取得平衡。
pub fn conversation_system(
    target_lang: &str,
    native_lang: &str,
    known_word_count: usize,
    cefr: Option<&str>,
    topic: Option<&str>,
) -> String {
    format!(
        "你是一位親切的{target}對話夥伴，正在陪一位{native}母語者練習口說。\n\n\
         # 對話原則\n\
         - 學習者約會 {known} 個單字{cefr}。請把用字控制在這個範圍，\n\
           偶爾（每 3~4 句一次）帶入一個略高於程度的新詞，並在同一句用簡單的說法解釋。\n\
         - 你的回覆保持 2~4 句，並且**總是**以一個開放式問題結尾，讓對話繼續。\n\
         - 不要說教、不要列清單，像真人聊天。\n\n\
         # 糾錯原則\n\
         - 學習者說錯時，先自然地用正確說法回應（recast），維持對話流暢。\n\
         - 只在錯誤會造成誤解、或同一個錯誤重複出現時，才明確指出。\n\
         - 明確糾正時用{native}簡短說明，然後立刻回到對話。\n\n\
         {topic}",
        target = target_lang,
        native = native_lang,
        known = known_word_count,
        cefr = cefr.map(|c| format!("（CEFR {c}）")).unwrap_or_default(),
        topic = topic
            .map(|t| format!("# 今天的主題\n{t}"))
            .unwrap_or_else(|| "# 今天的主題\n由你先起頭，問一個輕鬆的日常問題。".into()),
    )
}

/// 寫作 / 翻譯批改。
///
/// 輸出結構化的逐句修正，讓 App 能把每個錯誤對應到文法點，
/// 累積成弱點清單再回頭出題。
pub fn writing_feedback(
    target_lang: &str,
    native_lang: &str,
    task: &str,
    submission: &str,
) -> ChatRequest {
    let system = format!(
        "你是一位嚴謹但鼓勵人的{target}寫作老師。你只輸出 JSON。",
        target = target_lang
    );

    let points = grammar_point_rule(target_lang);
    let prompt = format!(
        "# 題目\n{task}\n\n\
         # 學習者的作答\n{submission}\n\n\
         # 批改要求\n\
         - 逐句比對，只標出真正的錯誤或明顯不自然的表達，不要為了改而改。\n\
         - 每個問題都要標註文法點。{points}\n\
         - severity 用 major（造成誤解）或 minor（可理解但不自然）。\n\
         - 講評用{native}，鼓勵具體的優點，不要空泛稱讚。\n\n\
         # 輸出格式\n\
         {{\n\
         \x20 \"score\": 0-100 的整數,\n\
         \x20 \"corrections\": [{{\"original\": \"原句\", \"corrected\": \"修改後\", \"grammar_point\": \"tense\", \"severity\": \"major\", \"explanation\": \"{native}說明\"}}],\n\
         \x20 \"rewritten\": \"整篇的自然版本\",\n\
         \x20 \"strengths\": [\"具體優點\"],\n\
         \x20 \"next_focus\": [\"接下來該加強的文法點\"]\n\
         }}",
        native = native_lang,
    );

    ChatRequest {
        system: Some(system),
        messages: vec![Message::user(prompt)],
        json_only: true,
    }
}

/// 文法練習出題。針對學習者的錯誤紀錄出題，而不是隨機出題。
pub fn grammar_drill(
    target_lang: &str,
    native_lang: &str,
    weak_points: &[String],
    known_sample: &[String],
    question_count: usize,
    material_excerpt: Option<&str>,
) -> ChatRequest {
    let system = format!(
        "你是一位{target}文法老師。你只輸出 JSON。",
        target = target_lang
    );

    let mut prompt = format!(
        "# 這位學習者最近常錯的文法點\n{points}\n\n\
         # 用字限制\n\
         題目請盡量使用這些學習者已經會的字，避免因為單字不會而答錯文法題：\n{sample}\n\n",
        points = if weak_points.is_empty() {
            "（沒有紀錄，請出基礎綜合練習）".to_string()
        } else {
            weak_points.join("、")
        },
        sample = known_sample.join("、"),
    );

    if let Some(excerpt) = material_excerpt {
        prompt.push_str(&format!(
            "# 指定教材\n題目的句子與情境只能取材自以下內容：\n---\n{excerpt}\n---\n\n"
        ));
    }

    prompt.push_str(&format!(
        "# 輸出格式\n\
         出 {n} 題，每題聚焦一個文法點。{points}\n\
         {{\n\
         \x20 \"items\": [{{\"prompt\": \"題目（含填空）\", \"options\": [\"A\", \"B\", \"C\", \"D\"], \
         \"option_notes\": [\"每個選項各一句{native}說明\"], \
         \"answer_index\": 0, \"grammar_point\": \"tense\", \
         \"explanation\": \"用{native}說明這一題在考什麼文法\"}}]\n\
         }}\n\
         {notes}",
        n = question_count,
        native = native_lang,
        points = grammar_point_rule(target_lang),
        notes = option_notes_rule(native_lang),
    ));

    ChatRequest {
        system: Some(system),
        messages: vec![Message::user(prompt)],
        json_only: true,
    }
}

/// 批改翻譯，並判斷學習者不懂哪些字。
///
/// `unknown_words` 是這整套設計的關鍵：批改不只是打分數，還要看出
/// 「他其實不會這個字」，把那些字排進複習。使用者不必自己判斷哪裡不會——
/// 從錯誤裡看出來本來就是老師的工作。
pub fn translation_feedback(
    target_lang: &str,
    native_lang: &str,
    direction_to_target: bool,
    items: &[(String, String)],
    known_weak_points: &[String],
) -> ChatRequest {
    let system = format!("你是一位{target_lang}老師，正在批改翻譯練習。你只輸出 JSON。");

    let body: String = items
        .iter()
        .enumerate()
        .map(|(i, (source, answer))| {
            let answer = if answer.trim().is_empty() {
                "（沒有作答）"
            } else {
                answer.trim()
            };
            format!("{}. 題目：{source}\n   學習者的翻譯：{answer}\n", i + 1)
        })
        .collect();

    let direction = if direction_to_target {
        format!("{native_lang} → {target_lang}")
    } else {
        format!("{target_lang} → {native_lang}")
    };

    // 讓模型知道這個人的老毛病。同一個錯誤重複出現，
    // 跟第一次犯的意義完全不同——前者要講得更具體。
    let history = if known_weak_points.is_empty() {
        String::new()
    } else {
        format!(
            "# 這位學習者最近常犯的錯\n{}\n\
             如果這次又犯了同樣的錯，請在 explanation 裡指出這是重複出現的問題，\n\
             並給一個能記住的判斷方式，而不是重複同樣的說明。\n\n",
            known_weak_points.join("、")
        )
    };

    let points = grammar_point_rule(target_lang);
    let prompt = format!(
        "# 練習方向\n{direction}\n\n{history}# 作答\n{body}\n\
         # 批改要求\n\
         - 意思對就算對，不要為了語法完美而挑剔可接受的說法。\n\
         - 每個問題都要標註文法點。{points}\n\
         - **判斷學習者不懂哪些字**：翻錯、漏譯、或用了明顯繞路的說法，\n\
           都代表他不會那個字。把那些{target_lang}單字列在 unknown_words，\n\
           系統會自動排進他的複習。\n\
         - 沒有作答的題目，題目裡的關鍵字就是他不會的字。\n\
         - unknown_words 只放單字原形，不要放片語或整句。\n\n\
         # 輸出格式\n\
         {{\n\
         \x20 \"score\": 0 到 100 的整數,\n\
         \x20 \"items\": [{{\"index\": 1, \"correct\": true, \"reference\": \"參考答案\", \
         \"comment\": \"用{native_lang}說明，答對就寫得簡短\"}}],\n\
         \x20 \"corrections\": [{{\"original\": \"原句\", \"corrected\": \"修正後\", \
         \"grammar_point\": \"tense\", \"severity\": \"minor\", \
         \"explanation\": \"用{native_lang}說明\"}}],\n\
         \x20 \"unknown_words\": [\"他不會的{target_lang}單字\"]\n\
         }}"
    );

    ChatRequest {
        system: Some(system),
        messages: vec![Message::user(prompt)],
        json_only: true,
    }
}

/// 批改閱讀測驗，並從答錯的題目往回推斷不懂的字。
pub fn reading_feedback(
    target_lang: &str,
    native_lang: &str,
    passage: &str,
    questions: &[(String, String, String)],
) -> ChatRequest {
    let system = format!("你是一位{target_lang}閱讀理解老師。你只輸出 JSON。");

    let body: String = questions
        .iter()
        .enumerate()
        .map(|(i, (question, answer, correct))| {
            let answer = if answer.trim().is_empty() {
                "（沒有作答）"
            } else {
                answer.trim()
            };
            format!(
                "{}. {question}\n   學習者選了：{answer}\n   正確答案：{correct}\n",
                i + 1
            )
        })
        .collect();

    let prompt = format!(
        "# 文章\n{passage}\n\n# 作答\n{body}\n\
         # 批改要求\n\
         - **針對他選的那個選項講**，不是只講正確答案為什麼對。\n\
           答錯時要說出他挑的那一個哪裡不成立、文章的哪一句讓他誤會了；\n\
           「正確答案是 C 因為…」對他沒有用，他要知道的是自己那條路錯在哪。\n\
           答對的就寫得簡短，一句話確認他抓到重點就好。\n\
         - 用{native_lang}寫。\n\
         - **判斷他不懂哪些字**：從答錯的題目往回看，那一段裡有哪些字\n\
           是他看不懂才會選錯的？列在 unknown_words，系統會排進複習。\n\
         - 只放文章裡真的出現過的單字原形。\n\n\
         # 輸出格式\n\
         {{\n\
         \x20 \"score\": 0 到 100 的整數,\n\
         \x20 \"items\": [{{\"index\": 1, \"correct\": true, \
         \"comment\": \"用{native_lang}說明\"}}],\n\
         \x20 \"unknown_words\": [\"他看不懂的單字\"]\n\
         }}"
    );

    ChatRequest {
        system: Some(system),
        messages: vec![Message::user(prompt)],
        json_only: true,
    }
}

/// 翻譯出題，刻意使用到期複習的單字。
///
/// `direction_to_target` 決定題目句子要用哪個語言寫。**這件事一定要
/// 講到 prompt 裡**：先前不管哪個方向都說「請出 N 個{母語}句子」，
/// 於是「英翻中」出來的題目也是中文句子，那個題型等於不存在。
pub fn translation_task(
    target_lang: &str,
    native_lang: &str,
    direction_to_target: bool,
    material_excerpt: Option<&str>,
    due_words: &[String],
    count: usize,
) -> ChatRequest {
    let system = format!(
        "你是一位{target}翻譯練習出題老師。你只輸出 JSON。",
        target = target_lang
    );

    // 題目句子的語言與作答的語言。兩個都要明講，而且要講「不可以」，
    // 光說「請出 N 個 X 句子」模型還是會照著它心裡的預設走。
    let (source_lang, answer_lang) = if direction_to_target {
        (native_lang, target_lang)
    } else {
        (target_lang, native_lang)
    };

    let mut prompt = format!(
        "# 出題要求\n\
         練習方向是「{source} → {answer}」：\n\
         請出 {count} 個**{source}**句子，讓學習者翻譯成{answer}。\n\
         `source` 欄位一定要是{source}，寫成{answer}就是出錯了，這一題會作廢。\n\
         每個句子要自然、日常，並且**必須**用到下列其中一個今天該複習的\n\
         {target}單字（學習者翻譯時就等於複習了這個字）：\n{words}\n\
         方向是{target} → {native}時，那個字直接出現在題目句子裡；\n\
         方向是{native} → {target}時，題目句子要寫成翻出來自然會用到它。\n\n",
        count = count,
        native = native_lang,
        target = target_lang,
        source = source_lang,
        answer = answer_lang,
        words = due_words.join("、"),
    );

    if let Some(excerpt) = material_excerpt {
        prompt.push_str(&format!(
            "# 指定教材\n\
             句子的情境、用字與句型**只能**取材自以下內容，\n\
             不要引入教材以外的說法：\n---\n{excerpt}\n---\n\n"
        ));
    }

    prompt.push_str(&format!(
        "# 輸出格式\n\
         {{\n\
         \x20 \"items\": [{{\"source\": \"要翻譯的{source}句子\", \
         \"target_word\": \"必須用到的{target}字\", \
         \"reference\": \"{answer}參考答案\", \"acceptable_variants\": [\"其他可接受的說法\"]}}]\n\
         }}",
        source = source_lang,
        answer = answer_lang,
        target = target_lang,
    ));

    ChatRequest {
        system: Some(system),
        messages: vec![Message::user(prompt)],
        json_only: true,
    }
}

/// 產生的文章沒通過本地覆蓋率驗收時，用這個追加訊息要求重寫。
///
/// 帶上實際超標的詞，比單純說「太難了」有效得多。
pub fn coverage_retry(
    actual_coverage: f64,
    target_coverage: f64,
    offenders: &[String],
    previous_passage: &str,
) -> Message {
    // 一定要把上一篇附上。這則訊息說「其餘內容盡量保留」，
    // 但模型看不到自己上次寫了什麼——API 那邊我們沒有把它的回答加進
    // messages，CLI 更是每次都全新的行程，完全無狀態。
    // 少了這段，模型只能重寫一篇不相干的文章。
    Message::user(format!(
        "這篇文章沒有通過檢核：實際已知詞覆蓋率只有 {actual:.1}%，低於要求的 {target:.1}%。\n\n\
         你上一次寫的是：\n---\n{previous_passage}\n---\n\n\
         以下這些詞學習者不認識，也不在允許的新詞白名單中：{offenders}\n\
         請把它們換成學習者會的字，其餘內容與情節盡量保留，並重新輸出完整 JSON。",
        actual = actual_coverage * 100.0,
        target = target_coverage * 100.0,
        offenders = offenders.join("、"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_words() -> Vec<String> {
        ["cat", "run", "happy"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    fn spec<'a>(
        target_words: &'a [String],
        known_sample: &'a [String],
        excerpt: Option<&'a str>,
    ) -> ReadingSpec<'a> {
        ReadingSpec {
            target_lang: "English",
            native_lang: "繁體中文",
            word_count: 300,
            target_coverage: 0.96,
            known_word_count: 1500,
            cefr: Some("A2"),
            known_sample,
            target_words,
            review_words: &[],
            topic: Some("學校生活"),
            material_excerpt: excerpt,
            question_count: 4,
        }
    }

    fn cloze_spec<'a>(
        blank_words: &'a [String],
        known_sample: &'a [String],
        topic: Option<&'a str>,
        excerpt: Option<&'a str>,
    ) -> ClozeSpec<'a> {
        ClozeSpec {
            target_lang: "English",
            native_lang: "繁體中文",
            word_count: 120,
            known_word_count: 2_000,
            known_sample,
            blank_words,
            topic,
            material_excerpt: excerpt,
        }
    }

    #[test]
    fn reading_prompt_states_a_concrete_unknown_budget() {
        let targets = sample_words();
        let known = sample_words();
        let s = spec(&targets, &known, None);
        assert_eq!(s.unknown_budget(), 12); // 300 詞 × 4%

        let req = reading_comprehension(&s);
        let text = &req.messages[0].content;
        assert!(
            text.contains("不可超過 12 個"),
            "沒有給出具體生詞上限：{text}"
        );
        assert!(text.contains("96%"));
        assert!(text.contains("學校生活"));
        assert!(req.json_only);
    }

    /// 新詞與複習字是兩件事：新詞佔生詞預算、必須全部用到；
    /// 複習字他已經會，能帶就帶。混在一起講的話模型會分不清楚。
    #[test]
    fn review_words_are_asked_for_separately_from_new_words() {
        let to_vec = |xs: &[&str]| -> Vec<String> { xs.iter().map(|s| s.to_string()).collect() };
        let targets = to_vec(&["hierarchy", "offend"]);
        let sample = to_vec(&["the", "cat"]);
        let review = to_vec(&["apple", "run"]);

        let mut s = spec(&targets, &sample, None);
        s.review_words = &review;
        let text = reading_comprehension(&s).messages[0].content.clone();

        assert!(text.contains("hierarchy"));
        assert!(text.contains("apple"));
        assert!(text.contains("順便複習"), "{text}");
        assert!(
            text.contains("盡量全部用到"),
            "新詞要講明必須全用，不然模型會挑著用：{text}"
        );
        assert!(
            text.contains("粗俗"),
            "字典的 register 標不出粗話（實測 bitch 只標了 colloquial），\
             所以只能請模型自己跳過：{text}"
        );

        // 沒有複習字時整段不該出現
        let plain = reading_comprehension(&spec(&targets, &sample, None)).messages[0]
            .content
            .clone();
        assert!(!plain.contains("順便複習"));
    }

    /// 題目寫成母語的話，閱讀測驗就只剩下「讀文章」，題目本身沒在練語言。
    /// 實際看到過整份題目都是中文的。
    #[test]
    fn questions_are_asked_in_the_language_being_learned() {
        let targets = sample_words();
        let known = sample_words();
        let text = reading_comprehension(&spec(&targets, &known, None)).messages[0]
            .content
            .clone();

        assert!(
            text.contains("題目與選項一律用English寫"),
            "沒有規定題目的語言：{text}"
        );
        assert!(
            text.contains("只有 explanation 與 gloss 用繁體中文"),
            "解說仍該用母語，不然看不懂為什麼錯：{text}"
        );
    }

    /// 解析要能給出全文翻譯，讀完才對照得起來。
    #[test]
    fn reading_prompt_asks_for_a_full_translation() {
        let targets = sample_words();
        let known = sample_words();
        let text = reading_comprehension(&spec(&targets, &known, None)).messages[0]
            .content
            .clone();
        assert!(text.contains("\"translation\""), "{text}");
        assert!(text.contains("整篇文章的繁體中文翻譯"), "{text}");
    }

    /// 一整份同一個難度的題目，做完不知道自己讀懂了沒有。
    #[test]
    fn questions_are_spread_across_difficulties() {
        let targets = sample_words();
        let known = sample_words();
        let mut s = spec(&targets, &known, None);
        s.question_count = 4;
        let text = reading_comprehension(&s).messages[0].content.clone();

        for level in ["easy", "medium", "hard"] {
            assert!(text.contains(level), "難度分配少了 {level}：{text}");
        }
        assert!(text.contains("第 4 題"), "四題都要指定難度：{text}");

        // 題數變少時樣式要跟著縮，不能講到不存在的題號
        s.question_count = 2;
        let short = reading_comprehension(&s).messages[0].content.clone();
        assert!(!short.contains("第 3 題"), "{short}");
    }

    /// 為了塞新詞，文章很容易寫出前後兜不起來的句子。
    #[test]
    fn the_model_is_told_to_reread_its_own_article() {
        let targets = sample_words();
        let known = sample_words();
        let text = reading_comprehension(&spec(&targets, &known, None)).messages[0]
            .content
            .clone();
        assert!(text.contains("回頭檢查文章"), "{text}");
        assert!(text.contains("代名詞找得到它指的是誰"), "{text}");
        assert!(
            text.contains("只有一個選項對"),
            "檢查也要涵蓋題目本身：{text}"
        );
    }

    #[test]
    fn reading_prompt_whitelists_target_words() {
        let targets = sample_words();
        let known = sample_words();
        let req = reading_comprehension(&spec(&targets, &known, None));
        let text = &req.messages[0].content;
        for w in &targets {
            assert!(text.contains(w.as_str()), "白名單漏了 {w}");
        }
    }

    /// 沒有新詞時是純複習，prompt 不能出現空白的白名單。
    #[test]
    fn reading_prompt_handles_review_only_sessions() {
        let known = sample_words();
        let req = reading_comprehension(&spec(&[], &known, None));
        assert!(req.messages[0].content.contains("這次只做複習"));
    }

    /// 翻譯題也要吃教材，不然「只考課本」只有閱讀成立。
    #[test]
    fn translation_can_be_confined_to_a_material() {
        let due = vec!["weather".to_string()];
        let req = translation_task(
            "English",
            "繁體中文",
            true,
            Some("Lesson 3: At the market."),
            &due,
            3,
        );
        let text = req.messages[0].content.clone();
        assert!(text.contains("At the market."));
        assert!(text.contains("只能"), "沒有講清楚是硬限制：{text}");

        // 沒指定教材時整段不該出現，否則模型會看到一個空的「指定教材」
        let free = translation_task("English", "繁體中文", true, None, &due, 3);
        assert!(!free.messages[0].content.contains("指定教材"));
    }

    /// 選擇題在本地判分，模型從頭到尾沒看過學習者選了什麼。
    /// 「你選的那個為什麼不行」只能在出題時先備好——每個選項各一句，
    /// 判分時挑他按的那一句出來。三種選擇題都要有。
    #[test]
    fn every_option_carries_its_own_note() {
        let targets = sample_words();
        let known = sample_words();
        let due = vec!["borrow".to_string()];
        let weak = vec!["tense".to_string()];

        let prompts = [
            reading_comprehension(&spec(&targets, &known, None)).messages[0]
                .content
                .clone(),
            grammar_drill("English", "繁體中文", &weak, &known, 5, None).messages[0]
                .content
                .clone(),
            cloze_passage(&cloze_spec(&due, &known, None, None)).messages[0]
                .content
                .clone(),
        ];

        for text in prompts {
            assert!(text.contains("option_notes"), "沒有要求逐選項說明：{text}");
            assert!(
                text.contains("一樣長、一樣順序"),
                "沒說要跟 options 對齊，配錯了畫面還是看起來合理：{text}"
            );
            assert!(
                text.contains("「因為正確答案是 X」"),
                "沒擋掉「因為正確答案是 X」那種等於沒解釋的寫法：{text}"
            );
        }
    }

    /// 批改閱讀時模型看得到他選了什麼，那就該針對那個選項講。
    #[test]
    fn reading_feedback_addresses_the_option_the_learner_picked() {
        let questions = vec![(
            "Why did she leave?".to_string(),
            "A".to_string(),
            "C".to_string(),
        )];
        let text = reading_feedback("English", "繁體中文", "She left because...", &questions)
            .messages[0]
            .content
            .clone();

        assert!(text.contains("學習者選了：A"), "要帶上他實際選的：{text}");
        assert!(
            text.contains("針對他選的那個選項講"),
            "沒有要求針對作答說明：{text}"
        );
    }

    /// 這條測試存在的理由是它曾經是錯的：不管哪個方向，出題 prompt 都說
    /// 「請出 N 個{母語}句子」，於是「英翻中」拿到的題目也是中文句子，
    /// 那個題型等於不存在。方向一定要走進 prompt。
    #[test]
    fn the_source_sentence_language_follows_the_direction() {
        let due = vec!["weather".to_string()];

        let to_target = translation_task("English", "繁體中文", true, None, &due, 3);
        let text = &to_target.messages[0].content;
        assert!(
            text.contains("繁體中文 → English"),
            "沒有講出練習方向：{text}"
        );
        assert!(
            text.contains("**繁體中文**句子"),
            "中翻英的題目句子該是中文：{text}"
        );

        let to_native = translation_task("English", "繁體中文", false, None, &due, 3);
        let text = &to_native.messages[0].content;
        assert!(text.contains("English → 繁體中文"), "{text}");
        assert!(
            text.contains("**English**句子"),
            "英翻中的題目句子該是英文：{text}"
        );
        assert!(
            text.contains("寫成繁體中文就是出錯了"),
            "只說「請出 X 句子」模型還是會照它的預設走，要講「不可以」：{text}"
        );
    }

    /// 指定教材時必須明確限制範圍，否則 AI 會自由發揮到課本以外。
    #[test]
    fn material_excerpt_constrains_the_model() {
        let targets = sample_words();
        let known = sample_words();
        let req = reading_comprehension(&spec(&targets, &known, Some("Lesson 3: My Family")));
        let text = &req.messages[0].content;
        assert!(text.contains("Lesson 3: My Family"));
        assert!(text.contains("不可引入教材以外的知識點"));
    }

    /// 這條測試存在的理由：選「克漏字」從來沒有真的出過克漏字——
    /// `generate` 直接轉去閱讀測驗，連存進資料庫的題型都寫成 reading。
    #[test]
    fn cloze_blanks_the_words_that_are_due() {
        let due = vec!["borrow".to_string(), "return".to_string()];
        let known = sample_words();
        let req = cloze_passage(&cloze_spec(&due, &known, None, None));
        let text = &req.messages[0].content;

        assert!(text.contains("borrow"));
        assert!(text.contains("這 2 個字"), "{text}");
        assert!(text.contains("{{1}}"), "沒有講清楚空格怎麼寫：{text}");
        assert!(text.contains("剛好 2 題"), "題數要對得上空格數：{text}");
        assert!(
            text.contains("只用學習者已經會的字"),
            "克漏字考的是想不想得起來，不是看不看得懂：{text}"
        );
        assert!(req.json_only);
    }

    #[test]
    fn cloze_can_be_confined_to_a_material_and_a_topic() {
        let due = vec!["weather".to_string()];
        let known = sample_words();
        let req = cloze_passage(&cloze_spec(
            &due,
            &known,
            Some("旅行"),
            Some("Lesson 3: At the market."),
        ));
        let text = &req.messages[0].content;
        assert!(text.contains("旅行"));
        assert!(text.contains("At the market."));

        // 沒指定時整段不該出現，否則模型會看到一個空的「指定教材」
        let free = cloze_passage(&cloze_spec(&due, &known, None, None));
        let text = &free.messages[0].content;
        assert!(!text.contains("指定教材"));
        assert!(!text.contains("# 主題"));
    }

    #[test]
    fn conversation_prompt_caps_vocabulary_and_defines_correction_style() {
        let p = conversation_system("English", "繁體中文", 800, Some("A2"), None);
        assert!(p.contains("800 個單字"));
        assert!(p.contains("A2"));
        assert!(p.contains("recast"), "缺少糾錯策略");
        assert!(p.contains("由你先起頭"), "沒指定主題時該由 AI 開場");
    }

    #[test]
    fn feedback_prompt_requires_grammar_tags() {
        let req = writing_feedback(
            "English",
            "繁體中文",
            "描述你的週末",
            "I go to park yesterday.",
        );
        let text = &req.messages[0].content;
        assert!(text.contains("I go to park yesterday."));
        assert!(
            text.contains("grammar_point"),
            "沒有要求標註文法點就無法累積弱點"
        );
        assert!(text.contains("severity"));
        assert!(req.json_only);
    }

    #[test]
    fn grammar_drill_targets_recorded_weaknesses() {
        let weak = vec!["past tense".to_string(), "articles".to_string()];
        let known = sample_words();
        let req = grammar_drill("English", "繁體中文", &weak, &known, 5, None);
        let text = &req.messages[0].content;
        assert!(text.contains("past tense"));
        assert!(text.contains("出 5 題"));

        // 沒有弱點紀錄時要有合理的退路
        let fallback = grammar_drill("English", "繁體中文", &[], &known, 3, None);
        assert!(fallback.messages[0].content.contains("基礎綜合練習"));
    }

    #[test]
    fn translation_task_reuses_due_words() {
        let due = vec!["borrow".to_string(), "return".to_string()];
        let req = translation_task("English", "繁體中文", true, None, &due, 3);
        let text = &req.messages[0].content;
        assert!(text.contains("borrow"));
        assert!(text.contains("出 3 個"));
    }

    /// 批改的重點不只是分數，而是找出「他其實不會這個字」。
    #[test]
    fn translation_feedback_asks_for_unknown_words() {
        let items = vec![
            (
                "我昨天去了公園".to_string(),
                "I go to park yesterday".to_string(),
            ),
            ("他很勤奮".to_string(), String::new()),
        ];
        let req = translation_feedback("English", "繁體中文", true, &items, &[]);
        let text = &req.messages[0].content;

        assert!(text.contains("I go to park yesterday"), "要帶上實際作答");
        assert!(text.contains("（沒有作答）"), "空白作答要標示出來");
        assert!(text.contains("unknown_words"), "沒有要求列出不懂的字");
        assert!(text.contains("grammar_point"), "沒有要求標註文法點");
        assert!(text.contains("繁體中文 → English"), "要說明翻譯方向");
        assert!(req.json_only);
    }

    #[test]
    fn translation_feedback_states_the_other_direction() {
        let items = vec![("The weather is nice".to_string(), "天氣很好".to_string())];
        let req = translation_feedback("English", "繁體中文", false, &items, &[]);
        assert!(req.messages[0].content.contains("English → 繁體中文"));
    }

    /// 同一個錯誤重複出現，跟第一次犯的意義不同——模型要知道這件事。
    #[test]
    fn translation_feedback_carries_the_learners_history() {
        let items = vec![("我昨天去了公園".to_string(), "I go to park".to_string())];
        let weak = vec!["tense".to_string(), "articles".to_string()];

        let with_history = translation_feedback("English", "繁體中文", true, &items, &weak);
        let text = &with_history.messages[0].content;
        assert!(text.contains("tense"), "沒有帶上既有的弱點");
        assert!(text.contains("重複出現"), "沒有要求指出這是老毛病");

        // 沒有紀錄時不該憑空生出一段空白的「常犯的錯」
        let fresh = translation_feedback("English", "繁體中文", true, &items, &[]);
        assert!(!fresh.messages[0].content.contains("常犯的錯"));
    }

    /// 閱讀測驗要從答錯的題目往回推斷哪些字看不懂。
    #[test]
    fn reading_feedback_traces_mistakes_back_to_words() {
        let questions = vec![(
            "Why did she leave?".to_string(),
            "A".to_string(),
            "C".to_string(),
        )];
        let req = reading_feedback("English", "繁體中文", "She left because...", &questions);
        let text = &req.messages[0].content;

        assert!(text.contains("She left because..."));
        assert!(text.contains("正確答案：C"));
        assert!(text.contains("unknown_words"));
        assert!(text.contains("只放文章裡真的出現過的單字原形"));
    }

    /// 重試訊息要自帶上一篇文章。
    ///
    /// 模型看不到自己上次的回答：API 那邊我們沒把它加進 messages，
    /// CLI 更是每次全新行程。沒附上的話，「其餘內容盡量保留」這句
    /// 根本無從執行。
    #[test]
    fn retry_message_carries_the_previous_attempt() {
        let m = coverage_retry(
            0.88,
            0.96,
            &["ubiquitous".into(), "paradigm".into()],
            "The ubiquitous paradigm shifted.",
        );

        assert!(m.content.contains("88.0%"));
        assert!(m.content.contains("96.0%"));
        assert!(m.content.contains("ubiquitous"));
        assert!(
            m.content.contains("The ubiquitous paradigm shifted."),
            "沒有附上上一篇，模型無從「保留其餘內容」"
        );
    }
}
