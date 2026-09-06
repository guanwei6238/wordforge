//! 句子難度：上限從哪裡來、這次要練哪個句型。
//!
//! 出題的難度一直只有**詞彙**那一半有依據（覆蓋率實算、指派的字實際檢查），
//! 句法那一半完全沒有。這個模組把設定、句型定義與排程收成一份東西，
//! 給 prompt 用（[`Difficulty::brief`]）也給驗收用（[`Difficulty::spec`]）——
//! 兩邊拿的必須是同一份，否則會重演那個坑：prompt 告訴模型「他掌握 5200
//! 個字」，驗收卻用另一套標準，於是每一題都重寫三次而驗收本身毫無作用。

use wordforge_core::patterns::{DifficultyLimit, PatternSet};

use super::*;

/// prompt 裡最多列幾個「不要用」的句型。
///
/// 只列剛好超過上限的那幾個：允許的清單可能有上百條，列完就把 prompt
/// 撐爆，而且清單越長模型的注意力越稀薄。**最可能不小心用到的永遠是
/// 剛好高一級的那些。**
const AVOID_LIMIT: usize = 12;

/// prompt 裡最多列幾個「他已經會」的句型。
///
/// 由難到易取前幾個：模型需要的是他會的**上緣**在哪，
/// 而不是一份從 be 動詞開始的完整清單。
const KNOWN_LIMIT: usize = 20;

/// 挑「這次要練的句型」時看幾個候選。
///
/// 要看不只一個，因為偵測不到的句型會被跳過（見 [`PracticeEngine::difficulty`]）。
const PRACTISE_CANDIDATES: i64 = 6;

/// 這次出題的難度上限與指派。
#[derive(Default)]
pub(super) struct Difficulty {
    /// 編譯好、通過控制組的句型。壞掉的偵測器已經在編譯時被丟掉。
    pub set: PatternSet,
    pub limit: DifficultyLimit,
    /// 上限那一級的顯示名稱（`"國中七年級"`）。純粹給 prompt 用。
    pub level_name: Option<String>,
    /// 不要用的句型：`(名稱, 等級名稱)`
    pub avoid: Vec<(String, String)>,
    /// 他已經會的句型的識別碼。**從禁止清單裡扣掉，驗收時也不算超綱**
    /// ——分級上限是「一般人學到哪」的預設值，他親手標記的才是事實。
    pub known_points: Vec<String>,
    /// 同一批的名稱，列給模型看用。由難到易，而且有上限：
    /// 一份課綱可以有上百條，全塞進 prompt 只是燒 token，
    /// 模型需要的是「他會的上緣在哪」。
    pub known_names: Vec<String>,
    /// 這次要練的句型。**一定是偵測得到的那些之一**，理由見指派處。
    pub practise: Option<PractisePattern>,
}

pub(super) struct PractisePattern {
    pub point: String,
    pub name: String,
    pub explanation: Option<String>,
    pub examples: Vec<String>,
}

impl Difficulty {
    /// 給 prompt 的那一份。
    pub fn brief(&self) -> prompts::DifficultyBrief<'_> {
        prompts::DifficultyBrief {
            level_name: self.level_name.as_deref(),
            avoid: &self.avoid,
            known: &self.known_names,
            max_words: self.limit.max_words.map(|n| n as i64),
            max_clauses: self.limit.max_clause_markers.map(|n| n as i64),
            practise: self.practise.as_ref().map(|p| prompts::PatternBrief {
                point: &p.point,
                name: &p.name,
                explanation: p.explanation.as_deref(),
                examples: &p.examples,
            }),
        }
    }

    /// 給驗收的那一份。跟 [`Difficulty::brief`] 出自同一個來源——
    /// 兩邊各算各的話，「prompt 講的」與「驗收認的」就會漂移。
    pub fn spec<'a>(&'a self, lang: &'a str) -> crate::validate::DifficultySpec<'a> {
        crate::validate::DifficultySpec {
            patterns: &self.set,
            limit: &self.limit,
            lang,
            practise: self.practise.as_ref().map(|p| p.point.as_str()),
            known: &self.known_points,
        }
    }
}

impl PracticeEngine<'_> {
    /// 這次出題的難度上限，以及（`assign` 為真時）要練的句型。
    ///
    /// ## 為什麼翻譯要指派句型、閱讀不要
    ///
    /// 一篇 300 詞的文章本來就會用上幾十種結構，硬指派一個等於把文章綁死。
    /// 翻譯題不一樣：它本來就是「照這個要求寫一句」，而且**驗得到**——
    /// 使用者的作答是目標語言，regex 直接跑得動。
    pub(super) async fn difficulty(
        &self,
        profile_id: i64,
        assign: bool,
        now: OffsetDateTime,
    ) -> Result<Difficulty> {
        let settings = profiles::study_settings(self.db, ProfileId(profile_id)).await?;
        let limit = DifficultyLimit {
            level_ceiling: settings.pattern_ceiling,
            max_words: settings.sentence_max_words.map(|n| n as usize),
            max_clause_markers: settings.sentence_max_clauses.map(|n| n as usize),
        };

        let defs = grammar::pattern_defs(self.db, &self.target_lang).await?;
        // 沒匯入句型、也沒設任何上限：這套機制完全沒開，不要白編譯 regex，
        // 也不要在 prompt 裡留一段空的「句型難度」
        if defs.is_empty() && limit.is_off() {
            // 沒有句型定義就不可能有「已經會的句型」，這條路不必查
            return Ok(Difficulty::default());
        }

        let (set, problems) = PatternSet::compile(&defs);
        if !problems.is_empty() {
            // 這裡**不能安靜跳過**：一條壞掉的偵測器的症狀是「這個句型的
            // 檢查永遠通過」，畫面上完全正常。匯入時已經擋過一次，
            // 走到這裡表示是舊資料或手改過的。
            let listed: Vec<String> = problems.iter().map(|p| p.to_string()).collect();
            tracing::warn!(?listed, "有偵測器沒通過控制組，這些句型的檢查不會生效");
        }

        let level_name = match limit.level_ceiling {
            Some(ceiling) => grammar::level_options(self.db, &self.target_lang)
                .await?
                .into_iter()
                .find(|l| l.ordinal == ceiling)
                .and_then(|l| l.level),
            None => None,
        };

        // 他自己標記會了的句型。這份清單同時做兩件事：列給模型看
        // 「這些放心用」，以及從下面的禁止清單裡扣掉。
        let known = grammar::known_patterns(
            self.db,
            ProfileId(profile_id),
            &self.target_lang,
            KNOWN_STABILITY_DAYS,
        )
        .await?;
        let known_points: Vec<String> = known.iter().map(|(point, _)| point.clone()).collect();
        let known_names: Vec<String> = known
            .iter()
            .take(KNOWN_LIMIT)
            .map(|(_, name)| name.clone())
            .collect();

        let avoid = match limit.level_ceiling {
            Some(ceiling) => set
                .just_above(ceiling, AVOID_LIMIT)
                .iter()
                // 標記會了的不列進「不要用」。上限講的是「一般人學到哪」，
                // 他親手標記的是事實——事實贏過預設值，否則他標記完之後
                // 那個句型還是會被退回去重寫，而畫面上看不出為什麼。
                .filter(|p| !known_points.contains(&p.point))
                .map(|p| (p.name.clone(), p.level.clone().unwrap_or_default()))
                .collect(),
            None => Vec::new(),
        };

        let practise = if assign {
            self.pick_practise_pattern(profile_id, &set, limit.level_ceiling, now)
                .await?
        } else {
            None
        };

        Ok(Difficulty {
            set,
            limit,
            level_name,
            avoid,
            known_points,
            known_names,
            practise,
        })
    }

    /// 挑這次要練的句型：到期的優先，其次是還沒練過的，由簡入難。
    ///
    /// ## 為什麼只挑偵測得到的
    ///
    /// 指派一個偵測不到的句型，出題之後沒辦法確認它真的出現在句子裡，
    /// 批改時也沒辦法確認使用者真的用對了——但系統仍然會把這次算成
    /// 「練了它」。那是「量不到卻回報 0」的那種錯：看起來有在進步，
    /// 實際上那個數字沒有依據。
    ///
    /// 偵測不到的句型還是列在文法頁上、還是可以按「請 AI 講解」，
    /// 只是不會被自動指派——那是誠實的降級。
    async fn pick_practise_pattern(
        &self,
        profile_id: i64,
        set: &PatternSet,
        ceiling: Option<i64>,
        now: OffsetDateTime,
    ) -> Result<Option<PractisePattern>> {
        let candidates = grammar::due_patterns(
            self.db,
            ProfileId(profile_id),
            &self.target_lang,
            ceiling,
            now,
            PRACTISE_CANDIDATES,
        )
        .await?;

        for point in candidates {
            if set.get(&point).is_none() {
                continue;
            }
            let Some(def) = grammar::get_def(self.db, &self.target_lang, &point).await? else {
                continue;
            };
            return Ok(Some(PractisePattern {
                point: def.point,
                name: def.name,
                explanation: def.explanation,
                examples: def.examples.into_iter().map(|e| e.text).collect(),
            }));
        }
        Ok(None)
    }
}
