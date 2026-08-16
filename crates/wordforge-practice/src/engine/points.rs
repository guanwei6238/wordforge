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
        let points = self.grammar_points(now).await?;

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
                        .and_then(|p| self.normalize_point(&points, p))
                    else {
                        continue;
                    };
                    let correct =
                        input.choices.get(i).copied().flatten() == Some(item.answer_index);
                    grammar::record(self.db, pid, &point, correct, &self.scheduler, now).await?;
                }
            }

            // 翻譯題沒有標準答案可以比對，只能採信批改指出來的錯誤。
            // 這裡沒有「答對」的資訊——沒被指出來不代表用對了，
            // 可能只是那句話根本沒用到這個文法。
            ExerciseBody::Translation { .. } => {
                for correction in &feedback.corrections {
                    let Some(point) = correction
                        .grammar_point
                        .as_deref()
                        .and_then(|p| self.normalize_point(&points, p))
                    else {
                        continue;
                    };
                    grammar::record(self.db, pid, &point, false, &self.scheduler, now).await?;
                }
            }
        }

        Ok(())
    }
}
