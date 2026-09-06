//! 文法點：受控清單、標籤正規化、把對錯記進排程。
//!
//! 清單住在資料庫（`grammar_def`），不是寫死的常數——「匯入什麼就能學什麼」
//! 對文法跟對字典是同一個承諾。這裡負責把它讀出來、把模型回的標籤收斂回去。

use super::*;

impl PracticeEngine<'_> {
    /// 請模型講解一個文法點，並把結果存進 `grammar_def`。
    ///
    /// ## 為什麼要存起來
    ///
    /// 沒有可以直接匯入的開源文法書，所以講解一開始是空的。生成一次就
    /// 存下來：之後開這一頁不必再等模型，也不會每看一次燒一次額度。
    /// 存下來還有一個好處——使用者可以自己編輯，模型講得不好就改掉。
    pub async fn explain_grammar(
        &self,
        profile_id: i64,
        point: &str,
        now: OffsetDateTime,
    ) -> Result<wordforge_db::grammar::GrammarDef> {
        let Some(mut def) = grammar::get_def(self.db, &self.target_lang, point).await? else {
            return Err(PracticeError::NotFound);
        };

        let learner = self.learner_profile(profile_id, now).await?;
        let req = prompts::grammar_explanation(
            self.target_name(),
            self.native_name(),
            &def.point,
            &def.name,
            learner.vocabulary as usize,
            GRAMMAR_EXAMPLES,
        );

        let value = self.ask_json(profile_id, "explain", &req).await?;

        let explanation = value
            .get("explanation")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| PracticeError::BadResponse("沒有產出講解".into()))?;

        def.explanation = Some(explanation.to_string());
        def.examples = value
            .get("examples")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|e| serde_json::from_value(e.clone()).ok())
                    .collect()
            })
            .unwrap_or_default();

        grammar::upsert_def(self.db, &def, now).await?;
        Ok(def)
    }

    /// 把模型給的文法標籤收斂到該語言的受控清單。
    ///
    /// 模型即使被告知只能從清單挑，還是會偶爾寫成 `past tense` 或 `Articles`。
    /// 沒有這一步的話，同一個文法點會散成好幾個各自排程的標籤。
    pub(super) fn normalize_point(&self, points: &[String], raw: &str) -> Option<String> {
        let normalized = wordforge_core::grammar_points::normalize_point(points, raw);
        if normalized.is_none() && !raw.trim().is_empty() {
            tracing::debug!(raw, "認不出來的文法標籤，略過");
        }
        normalized
    }

    /// 這個語言目前的受控文法點清單。
    ///
    /// 從 `grammar_def` 讀，不是寫死的常數——「匯入什麼就能學什麼」
    /// 對文法跟對字典是同一個承諾。第一次讀到空的就把種子寫進去，
    /// 讓英文開箱有東西可用；沒有種子的語言仍然是空的，
    /// 那時 prompt 會退回「請自己保持一致」。
    pub(super) async fn grammar_points(&self, now: OffsetDateTime) -> Result<Vec<String>> {
        grammar::seed_defs(self.db, &self.target_lang, now).await?;
        Ok(grammar::list_points(self.db, &self.target_lang).await?)
    }

    /// 把這次的文法表現記進 FSRS。
    ///
    /// 答錯的縮短間隔、答對的拉遠。這是「練熟的文法點不再出現」的機制，
    /// 也是下次出題時 `due_points` 的資料來源。
    pub(super) async fn record_grammar_results(
        &self,
        profile_id: i64,
        body: &ExerciseBody,
        input: &GradeInput,
        feedback: &Feedback,
        now: OffsetDateTime,
    ) -> Result<()> {
        let pid = ProfileId(profile_id);
        // 批改用的**標籤選單**（窄，只有錯誤標籤）
        let labels = self.grammar_points(now).await?;
        // 認回來用的**字典**（寬，含句型）。兩份不能共用：指定句型出的
        // 練習，題目標籤就是那個句型的識別碼，拿窄的去比對會認不出來，
        // 然後整份練習的對錯被無聲丟掉。
        let all = grammar::list_all_points(self.db, &self.target_lang).await?;

        match body {
            // 選擇題知道每一題在考什麼，對錯都能記。
            // 這類題目的 corrections 是本地判分產生的，內容與 items 重複，
            // 兩邊都記會讓同一次錯誤算成兩次。
            ExerciseBody::Choices { items }
            | ExerciseBody::Cloze { items, .. }
            | ExerciseBody::Reading {
                questions: items, ..
            } => {
                for (i, item) in items.iter().enumerate() {
                    let Some(point) = item
                        .grammar_point
                        .as_deref()
                        // 用**寬的**那份：這個標籤是我們自己指定的
                        // （`grammar_drill` 要求每一題都填指定的識別碼），
                        // 而使用者指定得到的包含句型
                        .and_then(|p| self.normalize_point(&all, p))
                    else {
                        continue;
                    };
                    let correct =
                        input.choices.get(i).copied().flatten() == Some(item.answer_index);
                    grammar::record(self.db, pid, &point, correct, &self.scheduler, now).await?;
                }
            }

            // 翻譯題沒有標準答案可以比對，錯誤那一側只能採信批改指出來的。
            //
            // **答對那一側**曾經是整個缺口：這裡只記 `false`，於是主要在做
            // 翻譯題的人，文法點的 stability 只會被拉低，沒有任何東西拉高它
            // ——練越多看起來越不熟。缺的不是誠意，是**量得到的證據**：
            // 沒被指出來不代表用對了，可能只是那句話根本沒用到這個文法。
            //
            // 有偵測器的句型就有那個證據，見 [`Self::record_pattern_use`]。
            ExerciseBody::Translation { to_target, .. } => {
                for correction in &feedback.corrections {
                    let Some(point) = correction
                        .grammar_point
                        .as_deref()
                        // 這裡用**窄的**那份：批改 prompt 只列了錯誤標籤，
                        // 模型只可能從那份挑。拿寬的去比對，關鍵字匹配會
                        // 把一條修正誤掛到某個句型上
                        .and_then(|p| self.normalize_point(&labels, p))
                    else {
                        continue;
                    };
                    grammar::record(self.db, pid, &point, false, &self.scheduler, now).await?;
                }

                // 只有「母語 → 目標語」量得到：那個方向使用者寫的是目標語言，
                // regex 跑得動。反方向他寫的是母語，偵測器對它沒有意義——
                // 硬跑只會什麼都比對不到，然後把每個句型都記成沒練。
                if *to_target {
                    self.record_pattern_use(profile_id, input, feedback, now)
                        .await?;
                }
            }
        }

        Ok(())
    }

    /// 從**使用者自己寫的那一句**量句型：他用了什麼、用對了沒有。
    ///
    /// ## 為什麼這件事量得到
    ///
    /// 「母語 → 目標語」的作答是目標語言，偵測器直接跑得動。
    /// 這是翻譯題唯一一處拿得到「答對」證據的地方——其餘全是
    /// 「批改沒提到」，而那不等於用對了。
    ///
    /// ## 三種情況，第三種刻意什麼都不記
    ///
    /// | 這一題的狀況 | 記什麼 |
    /// | --- | --- |
    /// | 命中句型，而且被指出來的錯誤**壓在這個句型上** | 答錯 |
    /// | 命中句型，這一題的錯誤都在別的地方（或根本沒錯） | 答對 |
    /// | 命中句型，但有錯誤片段**對不回這一句的位置** | 什麼都不記 |
    ///
    /// 第三種是重點：對不回位置就不知道錯在哪，那時記「答對」會高估、
    /// 記「答錯」會冤枉。**量不到就回 `None`，不要回 0**——這跟 CLI
    /// 後端不回報 token 數時寧可留白、不寫 0 是同一條原則。
    async fn record_pattern_use(
        &self,
        profile_id: i64,
        input: &GradeInput,
        feedback: &Feedback,
        now: OffsetDateTime,
    ) -> Result<()> {
        let defs = grammar::pattern_defs(self.db, &self.target_lang).await?;
        if defs.is_empty() {
            return Ok(());
        }
        let (set, problems) = wordforge_core::patterns::PatternSet::compile(&defs);
        if !problems.is_empty() {
            let listed: Vec<String> = problems.iter().map(|p| p.to_string()).collect();
            tracing::warn!(?listed, "有偵測器沒通過控制組，這些句型量不到");
        }
        if set.is_empty() {
            return Ok(());
        }

        let attributed = attribute_all(&feedback.corrections, &input.answers);
        let pid = ProfileId(profile_id);

        for (i, answer) in input.answers.iter().enumerate() {
            if answer.trim().is_empty() {
                continue;
            }

            // 這一題被指出來的錯誤，各自落在作答的哪個位置
            let mut flaws: Vec<std::ops::Range<usize>> = Vec::new();
            let mut unlocated = 0usize;
            for (item, correction) in &attributed {
                if *item != Some(i) {
                    continue;
                }
                let fragment = correction.original.trim();
                match (!fragment.is_empty())
                    .then(|| answer.find(fragment))
                    .flatten()
                {
                    Some(start) => flaws.push(start..start + fragment.len()),
                    None => unlocated += 1,
                }
            }

            for (pattern, span) in set.detect(answer) {
                let broken = flaws
                    .iter()
                    .any(|flaw| flaw.start < span.end && span.start < flaw.end);
                if broken {
                    grammar::record(self.db, pid, &pattern.point, false, &self.scheduler, now)
                        .await?;
                } else if unlocated == 0 {
                    grammar::record(self.db, pid, &pattern.point, true, &self.scheduler, now)
                        .await?;
                } else {
                    // 有錯誤片段對不回位置：這個句型是對是錯我們不知道。
                    // 猜一個會讓掌握度變成一個沒有依據的數字。
                    tracing::debug!(
                        point = pattern.point,
                        "有對不回位置的修正，這個句型這次不計入"
                    );
                }
            }
        }

        Ok(())
    }
}

/// 每一條修正屬於哪一題（對不回去的是 `None`）。
///
/// 拆出來是因為有兩個用途：[`attribute_corrections`] 只要帶文法標籤的，
/// 而句型那條路要**全部**——「這一題有沒有被指出錯誤」跟那條修正有沒有
/// 標籤無關。兩邊各寫一份歸屬邏輯的話，一定會漂移。
fn attribute_all<'a>(
    corrections: &'a [Correction],
    answers: &[String],
) -> Vec<(Option<usize>, &'a Correction)> {
    corrections
        .iter()
        .map(|correction| {
            // 模型給的題號優先，但要在範圍內——超出範圍的 index 比沒有更糟
            let by_index = correction
                .index
                .and_then(|n| n.checked_sub(1))
                .filter(|i| *i < answers.len());

            let by_text = || {
                let needle = correction.original.trim();
                if needle.is_empty() {
                    return None;
                }
                answers.iter().position(|a| a.contains(needle))
            };

            (by_index.or_else(by_text), correction)
        })
        .collect()
}

/// 把每一條修正歸到它屬於的那一題。
///
/// ## 為什麼需要兩條路
///
/// prompt 要求每條修正帶 `index`，但那只是請求——模型漏填的時候，
/// 「這一句你錯在哪個文法點」就整個說不出來。
///
/// 好在 `original` 存的是**使用者當時寫的句子片段**，拿它去比對作答就
/// 對得回題號。實測一份五題的翻譯練習，九條修正全部對得回去。
///
/// 兩條都不成立時就丟掉那一條：寧可少一個標籤，也不要把「你在冠詞上
/// 錯過」掛到一句根本沒有冠詞問題的話上——那種錯畫面上看不出來。
///
/// 回傳 `(題號從 0 起算, 文法點識別碼)`，同一題可能有好幾個。
pub(super) fn attribute_corrections(
    corrections: &[Correction],
    answers: &[String],
) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (item, correction) in attribute_all(corrections, answers) {
        let Some(point) = correction
            .grammar_point
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
        else {
            continue;
        };
        if let Some(item) = item {
            out.push((item, point.to_string()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn correction(index: Option<usize>, original: &str, point: &str) -> Correction {
        Correction {
            index,
            original: original.into(),
            corrected: String::new(),
            grammar_point: Some(point.into()),
            severity: None,
            explanation: None,
        }
    }

    #[test]
    fn a_correction_with_an_index_goes_to_that_item() {
        let answers = vec!["第一句".to_string(), "第二句".to_string()];
        let got = attribute_corrections(&[correction(Some(2), "第二句", "tense")], &answers);
        assert_eq!(got, vec![(1, "tense".to_string())]);
    }

    /// 這條路是實測出來的：模型漏填 index 時，`original` 存的是使用者
    /// 當時寫的片段，拿它比對作答就對得回去（實測九條全中）。
    #[test]
    fn a_correction_without_an_index_is_matched_by_what_you_wrote() {
        let answers = vec![
            "In debate class, I would like prepare three reasons.".to_string(),
            "After club end, we cancelled it.".to_string(),
        ];
        let got = attribute_corrections(
            &[
                correction(None, "I would like prepare", "gerund-infinitive"),
                correction(None, "After club end", "articles"),
            ],
            &answers,
        );
        assert_eq!(
            got,
            vec![
                (0, "gerund-infinitive".to_string()),
                (1, "articles".to_string())
            ]
        );
    }

    /// 超出範圍的題號比沒有更糟：會把標籤掛到不存在的題目上。
    #[test]
    fn an_out_of_range_index_falls_back_to_matching() {
        let answers = vec!["I would like prepare it.".to_string()];
        let got = attribute_corrections(
            &[correction(
                Some(9),
                "I would like prepare",
                "gerund-infinitive",
            )],
            &answers,
        );
        assert_eq!(got, vec![(0, "gerund-infinitive".to_string())]);
    }

    /// 兩條路都不成立就丟掉：把標籤掛到錯的句子上，畫面看不出來。
    #[test]
    fn a_correction_that_matches_nothing_is_dropped() {
        let answers = vec!["完全不相干".to_string()];
        assert!(
            attribute_corrections(&[correction(None, "something else", "tense")], &answers)
                .is_empty()
        );
        // 沒有文法點的修正本來就沒有東西可以掛
        assert!(
            attribute_corrections(&[correction(Some(1), "完全不相干", "  ")], &answers).is_empty()
        );
    }
}
