//! 把做過的句子存下來，並建立「哪個字出現在哪一句」的索引。
//!
//! 複習單字時看得到的是字典的釋義與別人寫的例句。真正記得住的是自己
//! 做過的那一句——所以每出一份練習，就把裡面的句子留下來，複習頁與
//! 字典頁都查得到。
//!
//! ## 存所有句子，不只目標字的
//!
//! 原本只存「那份練習指派的目標字所在的句子」。那讓兩件事永遠不成立：
//!
//! - 句子裡順帶用到的字一句都拿不到。`final` 出現在
//!   「before the final exam」裡，但那題練的是 `ahead`，查 `final` 空空如也。
//! - 今天才學的字，回頭看不到三個月前做過、明明用到它的句子——
//!   因為連哪些字是在**出題當下**決定的。
//!
//! 所以改成：句子全部存，索引由句子本身的內容決定（分詞 → 查字典 →
//! 那句出現了哪些詞條）。索引不看使用者的牌組，否則只是把同一個
//! 寫死換個地方。
//!
//! 三個題型的「句子」長得不一樣：
//!
//! - **翻譯題**：一題就是一句。
//! - **閱讀 / 克漏字**：一整篇文章，要先切句、再對齊全文翻譯
//!   （見 `text::align_sentences`）。
//! - **文法題**：那些句子是為了考一個文法點寫的，不存。
//!
//! 索引存的是 lemma 不是字串：查 `ran` 要看得到練 `run` 時寫的句子。

use std::collections::HashSet;

use wordforge_db::word_sentences::{self, NewSentence};

use super::*;

/// 補寫舊資料的版號。**改動連結邏輯就要加一**，否則已經跑過的資料庫
/// 永遠看不到新的連結。
///
/// 2：句子對齊從「句數對不上就整段」改成比例對齊。舊資料裡每一句配到的
/// 都是整段譯文（一句英文底下掛著八句中文），要整批重算。
///
/// 3：改成存所有句子、索引由句子內容決定。舊資料只有目標字的那幾句，
/// 而且索引只連得到目標字。
const BACKFILL_VERSION: i64 = 3;

impl PracticeEngine<'_> {
    /// 把這一份練習的句子存下來，並建立索引。
    ///
    /// 存不下就跳過（句子是空的、字典查不到任何詞），不讓它擋住出題——
    /// 這是加分項，不是必要條件。
    pub(super) async fn link_sentences(
        &self,
        profile_id: i64,
        exercise_id: i64,
        body: &ExerciseBody,
        now: OffsetDateTime,
    ) -> Result<()> {
        match body {
            ExerciseBody::Translation { to_target, items } => {
                for (index, item) in items.iter().enumerate() {
                    // 目標語言那一句：中翻英時在參考答案，英翻中時就是題目
                    let (text, translation) = if *to_target {
                        (item.reference.as_deref(), Some(item.source.as_str()))
                    } else {
                        (Some(item.source.as_str()), item.reference.as_deref())
                    };
                    let Some(text) = text else {
                        continue;
                    };
                    self.store_sentence(
                        profile_id,
                        exercise_id,
                        text,
                        translation,
                        "translation",
                        Some(index as i64),
                        now,
                    )
                    .await?;
                }
            }

            ExerciseBody::Reading {
                passage,
                translation,
                sentences,
                ..
            } => {
                self.store_passage(
                    profile_id,
                    exercise_id,
                    passage,
                    translation.as_deref(),
                    sentences,
                    "reading",
                    now,
                )
                .await?;
            }

            ExerciseBody::Cloze {
                passage,
                translation,
                sentences,
                items,
                ..
            } => {
                // 空格要先填回正確答案：`{{3}}` 不是句子的一部分，
                // 而挖掉的那個字往往正是最值得查到的那個
                let filled = fill_blanks(passage, items);
                self.store_passage(
                    profile_id,
                    exercise_id,
                    &filled,
                    translation.as_deref(),
                    sentences,
                    "cloze",
                    now,
                )
                .await?;
            }

            // 文法題沒有句子層級的教學目標：那些句子是為了考一個文法點寫的，
            // 掛到某個單字底下只會讓「我用過這個字」變得不精確
            ExerciseBody::Choices { .. } => {}
        }
        Ok(())
    }

    /// 一篇文章的每一句都存下來，各自配上那一句的譯文。
    #[allow(clippy::too_many_arguments)]
    async fn store_passage(
        &self,
        profile_id: i64,
        exercise_id: i64,
        passage: &str,
        translation: Option<&str>,
        given: &[SentencePair],
        origin: &str,
        now: OffsetDateTime,
    ) -> Result<()> {
        if passage.trim().is_empty() {
            return Ok(());
        }
        // 模型給的逐句對照（出題時已經驗過接得回原文）優先；
        // 舊練習與驗不過的情況退回本地切句
        let pairs: Vec<(&str, Option<String>)> = if given.is_empty() {
            wordforge_core::text::align_sentences(passage, translation.unwrap_or(""))
        } else {
            given
                .iter()
                .map(|s| (s.text.as_str(), s.translation.clone()))
                .collect()
        };

        for (sentence, sentence_translation) in pairs {
            self.store_sentence(
                profile_id,
                exercise_id,
                sentence,
                sentence_translation.as_deref(),
                origin,
                // 文章的一句不是「一題」：對不回排程，也沒有對錯可言
                None,
                now,
            )
            .await?;
        }
        Ok(())
    }

    /// 記一句，並把它連到句子裡出現的每個詞條。
    #[allow(clippy::too_many_arguments)]
    async fn store_sentence(
        &self,
        profile_id: i64,
        exercise_id: i64,
        text: &str,
        translation: Option<&str>,
        origin: &str,
        item_index: Option<i64>,
        now: OffsetDateTime,
    ) -> Result<()> {
        let stored = word_sentences::record(
            self.db,
            NewSentence {
                profile_id: ProfileId(profile_id),
                exercise_id,
                text,
                translation,
                origin,
                item_index,
            },
            now,
        )
        .await?;
        let Some(sentence_id) = stored else {
            return Ok(());
        };
        let lemma_ids = self.lemmas_in(text).await?;
        word_sentences::index(self.db, sentence_id, &lemma_ids).await?;
        Ok(())
    }

    /// 這一句裡出現了哪些詞條。
    ///
    /// **不看使用者的牌組**：今天才學的字，回頭要查得到三個月前做過、
    /// 明明用到它的句子。只索引牌組裡的字就是把「連哪些字」這個決定
    /// 又寫死回出題當下。
    ///
    /// 片語也要查（n-gram）：字典裡有 69 萬個多詞條目，`search for`
    /// 拆成兩個字分開查得不到「尋找」那個意思。
    ///
    /// 詞形交給 `base_forms` 決定，跟讀取端的 `family` 走同一份字典——
    /// 兩邊挑到不同的詞條的話，句子存進去了卻查不出來，而畫面上
    /// 只是少一塊。
    async fn lemmas_in(&self, text: &str) -> Result<Vec<LemmaId>> {
        let tokens = wordforge_core::text::tokenize(text);
        if tokens.is_empty() {
            return Ok(Vec::new());
        }
        let mut forms = tokens.clone();
        forms.extend(wordforge_core::text::ngrams(
            &tokens,
            &self.target_lang,
            MAX_PHRASE_LEN,
        ));
        let found = lemmas::base_forms(self.db, &self.target_lang, &forms).await?;
        // 同一個詞條會從好幾個詞形連過來（`run` / `ran`），去重
        let unique: HashSet<LemmaId> = found.into_values().collect();
        Ok(unique.into_iter().collect())
    }

    /// 把既有練習的句子補進連結表。
    ///
    /// 新功能對老使用者一開始是空的，而他做過的每一份練習裡都有句子。
    /// 靠 `app_meta` 的版號只跑一次：每次開 App 都掃一遍的話，
    /// 練習累積到幾百份時每次啟動都要等。
    ///
    /// 回傳補了幾份練習。
    pub async fn backfill_sentences(&self, profile_id: i64, now: OffsetDateTime) -> Result<u64> {
        let key = format!("sentence_backfill:{profile_id}");
        let applied = wordforge_db::meta::get_i64(self.db, &key).await?;
        if applied == Some(BACKFILL_VERSION) {
            return Ok(0);
        }
        // 跑過但版號舊 = 連結邏輯改了，那些句子要重算一次。
        // 不能沿用「已經連過就跳過」，否則改進永遠只對新練習生效。
        let rebuilding = applied.is_some();

        let mut done = 0;
        let mut offset = 0i64;
        loop {
            let batch = exercises::recent(self.db, ProfileId(profile_id), 100, offset).await?;
            if batch.is_empty() {
                break;
            }
            offset += batch.len() as i64;

            for record in batch {
                // 第一次補寫時，已經連過的跳過：重跑不該產生重複，UNIQUE
                // 擋得住，但沒必要為此把每一句都解析一遍。
                //
                // 重算時**不能**跳過——那正是要更新的對象。重跑靠 UNIQUE
                // 收斂：同一句會走到 ON CONFLICT，只更新譯文，
                // `misses` 那些累計的東西留著。
                if !rebuilding && word_sentences::has_any(self.db, record.id).await? {
                    continue;
                }
                let Ok(body) = serde_json::from_str::<ExerciseBody>(&record.payload_json) else {
                    continue;
                };
                self.link_sentences(profile_id, record.id, &body, now)
                    .await?;
                done += 1;
            }
        }

        wordforge_db::meta::set_i64(self.db, &key, BACKFILL_VERSION).await?;
        Ok(done)
    }
}

/// 收下模型給的逐句對照——**但要先驗它對得回原文**。
///
/// 逐句對照是額外要來的一份（全文翻譯照舊），用途是「這個字出現在哪一句」。
/// 它壞掉的方式很安靜：漏一句、把兩句併成一句、或順手改了幾個字，
/// 而畫面上每一句看起來都很正常，只是掛在別的字底下。
///
/// 驗法是把每一句接起來，正規化空白之後要跟原文一樣。對不上就回空的，
/// 呼叫端會退回本地切句——那條路已經在四篇實測資料上跑過。
pub(super) fn checked_sentences(value: &serde_json::Value, passage: &str) -> Vec<SentencePair> {
    let Some(raw) = value.get("sentences").and_then(|s| s.as_array()) else {
        return Vec::new();
    };
    let pairs: Vec<SentencePair> = raw
        .iter()
        .filter_map(|s| serde_json::from_value::<SentencePair>(s.clone()).ok())
        .filter(|s| !s.text.trim().is_empty())
        .collect();
    if pairs.is_empty() {
        return Vec::new();
    }

    let squashed = |text: &str| {
        text.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .replace(' ', "")
    };
    let joined: String = squashed(
        &pairs
            .iter()
            .map(|s| s.text.trim())
            .collect::<Vec<_>>()
            .join(" "),
    );

    if joined == squashed(passage) {
        pairs
    } else {
        tracing::debug!("逐句對照接不回原文，改用本地切句");
        Vec::new()
    }
}

/// 把 `{{n}}` 換成第 n 題的正解。
///
/// 克漏字的文章挖掉的正是要複習的字，不填回去的話那一句會變成
/// 「I had to {{1}} the umbrella」——句子在，但要連的那個字不在。
fn fill_blanks(passage: &str, items: &[ChoiceItem]) -> String {
    let mut out = passage.to_string();
    for (i, item) in items.iter().enumerate() {
        let Some(answer) = item.options.get(item.answer_index) else {
            continue;
        };
        // 空格的寫法允許中間有空白（`{{ 1 }}`），跟 `practice::BLANK_PATTERN` 一致
        for candidate in [format!("{{{{{}}}}}", i + 1), format!("{{{{ {} }}}}", i + 1)] {
            out = out.replace(&candidate, answer);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(options: &[&str], answer: usize) -> ChoiceItem {
        ChoiceItem {
            question: String::new(),
            options: options.iter().map(|s| s.to_string()).collect(),
            option_notes: Vec::new(),
            answer_index: answer,
            explanation: None,
            grammar_point: None,
            difficulty: None,
        }
    }

    /// 挖掉的那個字正是要連的字，不填回去的話句子裡只有一個編號。
    #[test]
    fn blanks_are_filled_with_the_right_answer() {
        let passage = "I had to {{1}} the umbrella because it {{2}} hard.";
        let items = [item(&["borrow", "lend"], 0), item(&["rained", "rains"], 0)];
        assert_eq!(
            fill_blanks(passage, &items),
            "I had to borrow the umbrella because it rained hard."
        );
    }

    /// 模型偶爾會寫成 `{{ 1 }}`，跟出題端的容忍度保持一致。
    #[test]
    fn spaced_blanks_are_filled_too() {
        assert_eq!(
            fill_blanks("Please {{ 1 }} it.", &[item(&["return"], 0)]),
            "Please return it."
        );
    }
}
