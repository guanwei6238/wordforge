//! 把做過的句子接回單字。
//!
//! 複習單字時看得到的是字典的釋義與別人寫的例句。真正記得住的是自己
//! 做過的那一句——所以每出一份練習，就把裡面的句子連到它練的那個字，
//! 複習頁與字典頁都拿得到。
//!
//! 三個題型的「句子」長得不一樣：
//!
//! - **翻譯題**：一題就是一句，指派的字也是明講的，直接連。
//! - **閱讀 / 克漏字**：一整篇文章，要先切句、再對齊全文翻譯，
//!   然後找出那個字真的出現在哪一句（見 `text::align_sentences`）。
//! - **文法題**：沒有句子層級的教學目標，不連。
//!
//! 連的是 lemma 不是字串：查 `ran` 要看得到練 `run` 時寫的句子。

use std::collections::HashSet;

use wordforge_db::word_sentences::{self, NewSentence};

use super::*;

/// 補寫舊資料的版號。**改動連結邏輯就要加一**，否則已經跑過的資料庫
/// 永遠看不到新的連結。
///
/// 2：句子對齊從「句數對不上就整段」改成比例對齊。舊資料裡每一句配到的
/// 都是整段譯文（一句英文底下掛著八句中文），要整批重算。
const BACKFILL_VERSION: i64 = 2;

impl PracticeEngine<'_> {
    /// 把這一份練習的句子連回單字。
    ///
    /// 連不上就跳過（字典查不到那個字、句子是空的），不讓它擋住出題——
    /// 這是加分項，不是必要條件。
    pub(super) async fn link_sentences(
        &self,
        profile_id: i64,
        exercise_id: i64,
        body: &ExerciseBody,
        target_words: &[String],
        now: OffsetDateTime,
    ) -> Result<()> {
        match body {
            ExerciseBody::Translation { to_target, items } => {
                for (index, item) in items.iter().enumerate() {
                    let Some(word) = item.target_word.as_deref() else {
                        continue;
                    };
                    // 目標語言那一句：中翻英時在參考答案，英翻中時就是題目
                    let (text, translation) = if *to_target {
                        (item.reference.as_deref(), Some(item.source.as_str()))
                    } else {
                        (Some(item.source.as_str()), item.reference.as_deref())
                    };
                    let Some(text) = text else {
                        continue;
                    };
                    self.link_one(
                        profile_id,
                        exercise_id,
                        word,
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
                self.link_passage(
                    profile_id,
                    exercise_id,
                    passage,
                    translation.as_deref(),
                    sentences,
                    target_words,
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
                // 而挖掉的那個字往往正是要連的字
                let filled = fill_blanks(passage, items);
                self.link_passage(
                    profile_id,
                    exercise_id,
                    &filled,
                    translation.as_deref(),
                    sentences,
                    target_words,
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

    /// 從一篇文章裡找出每個目標詞出現的那一句。
    ///
    /// 一個字只記**第一句**：同一篇文章裡出現三次，三句都記下來只是把
    /// 複習畫面塞滿同一篇文章的內容。
    #[allow(clippy::too_many_arguments)]
    async fn link_passage(
        &self,
        profile_id: i64,
        exercise_id: i64,
        passage: &str,
        translation: Option<&str>,
        given: &[SentencePair],
        words: &[String],
        origin: &str,
        now: OffsetDateTime,
    ) -> Result<()> {
        if passage.trim().is_empty() || words.is_empty() {
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

        for word in words {
            let forms: HashSet<String> = lemmas::forms(self.db, &self.target_lang, word)
                .await?
                .into_iter()
                .collect();
            let found = pairs.iter().find(|(sentence, _)| {
                wordforge_core::text::mentions_any(sentence, &forms, &self.target_lang)
            });
            let Some((sentence, sentence_translation)) = found else {
                continue;
            };
            self.link_one(
                profile_id,
                exercise_id,
                word,
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

    /// 記一句。字典查不到那個字就跳過——連結要指到 lemma，
    /// 不然「查 ran 看得到練 run 的句子」不成立。
    #[allow(clippy::too_many_arguments)]
    async fn link_one(
        &self,
        profile_id: i64,
        exercise_id: i64,
        word: &str,
        text: &str,
        translation: Option<&str>,
        origin: &str,
        item_index: Option<i64>,
        now: OffsetDateTime,
    ) -> Result<()> {
        let Some(lemma) = lemmas::base_form(self.db, &self.target_lang, word).await? else {
            return Ok(());
        };
        word_sentences::record(
            self.db,
            NewSentence {
                profile_id: ProfileId(profile_id),
                lemma_id: lemma,
                exercise_id,
                text,
                translation,
                origin,
                item_index,
            },
            now,
        )
        .await?;
        Ok(())
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
                self.link_sentences(profile_id, record.id, &body, &record.target_words, now)
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
