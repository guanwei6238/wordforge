//! FSRS-5 排程器。
//!
//! 符號對照（與官方 wiki 一致）：
//! - `S` stability：記憶維持在 90% 回憶率所能撐的天數
//! - `D` difficulty：卡片難度，1..=10
//! - `R` retrievability：此刻還記得的機率
//! - `G` grade：使用者評分，1..=4

use time::{Duration, OffsetDateTime};

use crate::model::{Card, CardState, MemoryState, Rating, ReviewLog};
use crate::{CoreError, Result};

/// 遺忘曲線的冪次。FSRS-5 固定為 -0.5。
pub const DECAY: f64 = -0.5;

/// 由 DECAY 推導出來的常數，使 `R(t = S) == 0.9` 恰好成立。
/// FACTOR = 0.9^(1/DECAY) - 1 = 19/81
pub const FACTOR: f64 = 19.0 / 81.0;

/// stability 的下限，避免公式在極端輸入下塌成 0 或負值。
const S_MIN: f64 = 0.01;
/// stability 的上限（100 年）。
const S_MAX: f64 = 36_500.0;

/// 一個「要記住的東西」的排程狀態。
///
/// 單字卡與文法點都用它：兩者都是「錯了要再練、對了可以拉遠」，
/// 沒有理由各寫一套排程。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReviewState {
    pub state: CardState,
    pub memory: Option<MemoryState>,
    pub due: OffsetDateTime,
    pub last_review: Option<OffsetDateTime>,
    /// learning / relearning 走到第幾步
    pub step: u8,
    /// 這次排出來的間隔（天）
    pub scheduled_days: i64,
}

impl ReviewState {
    /// 還沒學過的初始狀態。
    pub fn new(now: OffsetDateTime) -> Self {
        Self {
            state: CardState::New,
            memory: None,
            due: now,
            last_review: None,
            step: 0,
            scheduled_days: 0,
        }
    }
}

/// FSRS-5 的 19 個權重。
#[derive(Debug, Clone, PartialEq)]
pub struct FsrsParams(pub [f64; 19]);

impl Default for FsrsParams {
    /// FSRS-5 官方預設權重，由大規模匿名複習資料訓練而得。
    ///
    /// 累積足夠的 `review_log` 之後，可以用 FSRS optimizer 針對個人重新訓練，
    /// 再把結果寫回 profile 設定。
    fn default() -> Self {
        Self([
            0.40255, 1.18385, 3.17300,
            15.69105, // w0..w3：Again/Hard/Good/Easy 的初始 stability
            7.19490, 0.53450, // w4, w5：初始 difficulty
            1.46040, 0.00460, // w6, w7：difficulty 的變化與均值回歸
            1.54575, 0.11920, 1.01925, // w8..w10：答對時的 stability 增益
            1.93950, 0.11000, 0.29605, 2.26980, // w11..w14：答錯後的 stability
            0.23150, 2.98980, // w15, w16：Hard 懲罰 / Easy 加成
            0.51655, 0.66210, // w17, w18：同日重複複習
        ])
    }
}

impl FsrsParams {
    pub fn from_slice(v: &[f64]) -> Result<Self> {
        let arr: [f64; 19] = v.try_into().map_err(|_| CoreError::ParamCount {
            expected: 19,
            got: v.len(),
        })?;
        Ok(Self(arr))
    }

    #[inline]
    fn w(&self, i: usize) -> f64 {
        self.0[i]
    }
}

/// 排程器設定。這些是使用者可以在 UI 調整的旋鈕。
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    /// 目標記憶留存率。0.9 表示「複習時希望有 90% 想得起來」。
    /// 調高 → 複習變密、記得更牢；調低 → 複習變少、遺忘變多。
    pub desired_retention: f64,
    /// 間隔上限（天）。
    pub maximum_interval: i64,
    /// 新卡的學習步驟（分鐘級）。走完才畢業成長期複習。
    pub learning_steps: Vec<Duration>,
    /// 忘記之後的重新學習步驟。
    pub relearning_steps: Vec<Duration>,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            desired_retention: 0.9,
            maximum_interval: 36_500,
            learning_steps: vec![Duration::minutes(1), Duration::minutes(10)],
            relearning_steps: vec![Duration::minutes(10)],
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Scheduler {
    pub params: FsrsParams,
    pub config: SchedulerConfig,
}

impl Scheduler {
    pub fn new(params: FsrsParams, config: SchedulerConfig) -> Result<Self> {
        if config.desired_retention <= 0.0 || config.desired_retention >= 1.0 {
            return Err(CoreError::InvalidRetention(config.desired_retention));
        }
        Ok(Self { params, config })
    }

    // ---------- 遺忘曲線 ----------

    /// 距離上次複習 `elapsed_days` 天後，還記得的機率。
    pub fn retrievability(&self, elapsed_days: f64, stability: f64) -> f64 {
        if stability <= 0.0 {
            return 0.0;
        }
        (1.0 + FACTOR * elapsed_days.max(0.0) / stability).powf(DECAY)
    }

    /// 由 stability 反推「掉到目標留存率」需要幾天。
    pub fn next_interval(&self, stability: f64) -> i64 {
        let r = self.config.desired_retention;
        let days = stability / FACTOR * (r.powf(1.0 / DECAY) - 1.0);
        days.round().clamp(1.0, self.config.maximum_interval as f64) as i64
    }

    // ---------- 初始狀態 ----------

    fn initial_stability(&self, rating: Rating) -> f64 {
        clamp_s(self.params.w(rating as usize - 1))
    }

    fn initial_difficulty(&self, rating: Rating) -> f64 {
        let g = rating.grade();
        clamp_d(self.params.w(4) - (self.params.w(5) * (g - 1.0)).exp() + 1.0)
    }

    // ---------- 狀態轉移 ----------

    /// 難度更新：先依評分線性調整，再往「Easy 的初始難度」做均值回歸，
    /// 避免難度單向漂移到 10 之後永遠回不來。
    fn next_difficulty(&self, d: f64, rating: Rating) -> f64 {
        let delta = -self.params.w(6) * (rating.grade() - 3.0);
        // linear damping：越接近上限 10，同樣的評分推動得越少
        let damped = d + delta * (10.0 - d) / 9.0;
        let reverted = self.params.w(7) * self.initial_difficulty(Rating::Easy)
            + (1.0 - self.params.w(7)) * damped;
        clamp_d(reverted)
    }

    /// 答對時的新 stability。
    fn next_recall_stability(&self, d: f64, s: f64, r: f64, rating: Rating) -> f64 {
        let hard_penalty = if rating == Rating::Hard {
            self.params.w(15)
        } else {
            1.0
        };
        let easy_bonus = if rating == Rating::Easy {
            self.params.w(16)
        } else {
            1.0
        };

        clamp_s(
            s * (1.0
                + self.params.w(8).exp()
                    * (11.0 - d)
                    * s.powf(-self.params.w(9))
                    * ((1.0 - r) * self.params.w(10)).exp_m1()
                    * hard_penalty
                    * easy_bonus),
        )
    }

    /// 答錯（Again）時的新 stability。
    fn next_forget_stability(&self, d: f64, s: f64, r: f64) -> f64 {
        clamp_s(
            self.params.w(11)
                * d.powf(-self.params.w(12))
                * ((s + 1.0).powf(self.params.w(13)) - 1.0)
                * ((1.0 - r) * self.params.w(14)).exp(),
        )
    }

    /// 同一天內再次複習（間隔不足一天）時的 stability。
    /// 同日重複的資訊量低，所以用一條獨立且增幅較小的公式。
    fn next_short_term_stability(&self, s: f64, rating: Rating) -> f64 {
        clamp_s(s * (self.params.w(17) * (rating.grade() - 3.0 + self.params.w(18))).exp())
    }

    // ---------- 對外主要入口 ----------

    /// 一個「要記住的東西」的排程狀態。
    ///
    /// 抽出來是因為需要間隔重複的不只單字：文法點同樣是「錯了要再練、
    /// 對了可以拉遠」，用同一套 FSRS 才不會出現兩種行為不一致的排程。
    pub fn schedule(
        &self,
        current: ReviewState,
        rating: Rating,
        now: OffsetDateTime,
    ) -> ReviewState {
        let elapsed = current
            .last_review
            .map(|lr| (now - lr).as_seconds_f64() / 86_400.0)
            .unwrap_or(0.0)
            .max(0.0);

        let memory = match current.memory {
            None => MemoryState {
                stability: self.initial_stability(rating),
                difficulty: self.initial_difficulty(rating),
            },
            Some(prev) => {
                let r = self.retrievability(elapsed, prev.stability);
                let stability = if elapsed < 1.0 {
                    self.next_short_term_stability(prev.stability, rating)
                } else if rating.is_forget() {
                    self.next_forget_stability(prev.difficulty, prev.stability, r)
                } else {
                    self.next_recall_stability(prev.difficulty, prev.stability, r, rating)
                };
                MemoryState {
                    stability,
                    difficulty: self.next_difficulty(prev.difficulty, rating),
                }
            }
        };

        let (state, step, due, scheduled_days) =
            self.next_schedule_for(current.state, current.step, rating, now, &memory);

        ReviewState {
            state,
            memory: Some(memory),
            due,
            last_review: Some(now),
            step,
            scheduled_days,
        }
    }

    /// 送出一次複習，回傳更新後的卡片與這次的複習紀錄。
    ///
    /// 這是純函數：不寫資料庫、不讀時鐘，`now` 由呼叫端傳入，測試才好控制。
    pub fn review(
        &self,
        card: &Card,
        rating: Rating,
        now: OffsetDateTime,
        duration_ms: Option<u32>,
    ) -> (Card, ReviewLog) {
        let elapsed_days = card
            .last_review
            .map(|lr| ((now - lr).as_seconds_f64() / 86_400.0).max(0.0))
            .unwrap_or(0.0);

        let scheduled = self.schedule(
            ReviewState {
                state: card.state,
                memory: card.memory,
                due: card.due,
                last_review: card.last_review,
                step: card.step,
                scheduled_days: card.scheduled_days,
            },
            rating,
            now,
        );
        let memory = scheduled.memory.expect("排程後一定有記憶狀態");

        let mut next = card.clone();
        next.state = scheduled.state;
        next.step = scheduled.step;
        next.memory = Some(memory);
        next.due = scheduled.due;
        next.last_review = Some(now);
        next.scheduled_days = scheduled.scheduled_days;
        next.reps = card.reps.saturating_add(1);
        if card.state == CardState::Review && rating.is_forget() {
            next.lapses = card.lapses.saturating_add(1);
        }

        let log = ReviewLog {
            card_id: card.id,
            rating,
            state: card.state,
            memory,
            elapsed_days: elapsed_days.round() as i64,
            scheduled_days: scheduled.scheduled_days,
            reviewed_at: now,
            duration_ms,
        };

        (next, log)
    }

    /// 決定下一次該在什麼時候出現。
    ///
    /// learning / relearning 階段走分鐘級的固定步驟（讓當天真的記起來），
    /// 畢業之後才交給 FSRS 的天級間隔。
    fn next_schedule_for(
        &self,
        current_state: CardState,
        current_step: u8,
        rating: Rating,
        now: OffsetDateTime,
        memory: &MemoryState,
    ) -> (CardState, u8, OffsetDateTime, i64) {
        let graduate = |s: f64| {
            let days = self.next_interval(s);
            (CardState::Review, 0u8, now + Duration::days(days), days)
        };

        match current_state {
            CardState::New | CardState::Learning => {
                let steps = &self.config.learning_steps;
                if steps.is_empty() {
                    return graduate(memory.stability);
                }
                match rating {
                    Rating::Again => (CardState::Learning, 0, now + steps[0], 0),
                    Rating::Hard => {
                        let idx = (current_step as usize).min(steps.len() - 1);
                        (CardState::Learning, idx as u8, now + steps[idx], 0)
                    }
                    Rating::Good => {
                        let next_idx = current_step as usize + 1;
                        if next_idx >= steps.len() {
                            graduate(memory.stability)
                        } else {
                            (
                                CardState::Learning,
                                next_idx as u8,
                                now + steps[next_idx],
                                0,
                            )
                        }
                    }
                    Rating::Easy => graduate(memory.stability),
                }
            }
            CardState::Review => {
                if rating.is_forget() {
                    let steps = &self.config.relearning_steps;
                    if steps.is_empty() {
                        graduate(memory.stability)
                    } else {
                        (CardState::Relearning, 0, now + steps[0], 0)
                    }
                } else {
                    graduate(memory.stability)
                }
            }
            CardState::Relearning => {
                let steps = &self.config.relearning_steps;
                if steps.is_empty() {
                    return graduate(memory.stability);
                }
                match rating {
                    Rating::Again => (CardState::Relearning, 0, now + steps[0], 0),
                    Rating::Hard => {
                        let idx = (current_step as usize).min(steps.len() - 1);
                        (CardState::Relearning, idx as u8, now + steps[idx], 0)
                    }
                    Rating::Good | Rating::Easy => {
                        let next_idx = current_step as usize + 1;
                        if next_idx >= steps.len() {
                            graduate(memory.stability)
                        } else {
                            (
                                CardState::Relearning,
                                next_idx as u8,
                                now + steps[next_idx],
                                0,
                            )
                        }
                    }
                }
            }
        }
    }
}

fn clamp_s(s: f64) -> f64 {
    if s.is_nan() {
        S_MIN
    } else {
        s.clamp(S_MIN, S_MAX)
    }
}

fn clamp_d(d: f64) -> f64 {
    if d.is_nan() { 1.0 } else { d.clamp(1.0, 10.0) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CardKind, LemmaId, ProfileId};

    fn t0() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
    }

    fn new_card(now: OffsetDateTime) -> Card {
        Card::new(ProfileId(1), LemmaId(1), CardKind::Recognition, now)
    }

    /// FACTOR 的定義就是為了讓「經過 S 天後恰好剩 90% 記得」成立。
    #[test]
    fn retrievability_is_90_percent_after_one_stability() {
        let s = Scheduler::default();
        let r = s.retrievability(10.0, 10.0);
        assert!((r - 0.9).abs() < 1e-9, "R = {r}");
    }

    /// 目標留存率 0.9 時，間隔應該等於 stability。
    #[test]
    fn interval_at_default_retention_equals_stability() {
        let s = Scheduler::default();
        assert_eq!(s.next_interval(21.0), 21);
        assert_eq!(s.next_interval(1.0), 1);
    }

    /// 調高留存率必須讓間隔變短。
    #[test]
    fn higher_retention_shortens_interval() {
        let strict = Scheduler::new(
            FsrsParams::default(),
            SchedulerConfig {
                desired_retention: 0.97,
                ..Default::default()
            },
        )
        .unwrap();
        let loose = Scheduler::new(
            FsrsParams::default(),
            SchedulerConfig {
                desired_retention: 0.80,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(strict.next_interval(100.0) < loose.next_interval(100.0));
    }

    #[test]
    fn first_review_uses_initial_weights() {
        let s = Scheduler::default();
        let (card, log) = s.review(&new_card(t0()), Rating::Good, t0(), None);
        let m = card.memory.unwrap();
        assert!((m.stability - FsrsParams::default().w(2)).abs() < 1e-9);
        assert!((1.0..=10.0).contains(&m.difficulty));
        assert_eq!(log.state, CardState::New);
        assert_eq!(card.reps, 1);
    }

    /// Easy 的初始 stability 必須高於 Again。
    #[test]
    fn easy_starts_stronger_than_again() {
        let s = Scheduler::default();
        let (easy, _) = s.review(&new_card(t0()), Rating::Easy, t0(), None);
        let (again, _) = s.review(&new_card(t0()), Rating::Again, t0(), None);
        assert!(easy.memory.unwrap().stability > again.memory.unwrap().stability);
    }

    /// 新卡按 Easy 應該直接畢業成長期複習，且間隔至少一天。
    #[test]
    fn easy_graduates_immediately() {
        let s = Scheduler::default();
        let (card, _) = s.review(&new_card(t0()), Rating::Easy, t0(), None);
        assert_eq!(card.state, CardState::Review);
        assert!(card.scheduled_days >= 1);
    }

    /// 新卡按 Again 停留在 learning，並在分鐘級之後再出現。
    #[test]
    fn again_stays_in_learning() {
        let s = Scheduler::default();
        let (card, _) = s.review(&new_card(t0()), Rating::Again, t0(), None);
        assert_eq!(card.state, CardState::Learning);
        assert_eq!(card.due, t0() + Duration::minutes(1));
        assert_eq!(card.scheduled_days, 0);
    }

    /// 一路答對，間隔必須單調變長。
    #[test]
    fn intervals_grow_monotonically_with_good_reviews() {
        let s = Scheduler::default();
        let mut card = new_card(t0());
        let mut now = t0();
        let mut last = 0i64;
        let mut graduated_rounds = 0;

        for _ in 0..8 {
            let (next, _) = s.review(&card, Rating::Good, now, None);
            if next.state == CardState::Review {
                assert!(
                    next.scheduled_days >= last,
                    "間隔應遞增，但 {} < {last}",
                    next.scheduled_days
                );
                last = next.scheduled_days;
                graduated_rounds += 1;
            }
            now = next.due;
            card = next;
        }
        assert!(graduated_rounds >= 5);
        assert!(last > 1, "連續答對 8 次後間隔應該明顯拉長，實際 {last} 天");
    }

    /// 忘記一張已進入長期複習的卡：stability 下降、lapses 增加、回到 relearning。
    #[test]
    fn lapse_reduces_stability_and_counts() {
        let s = Scheduler::default();
        let (mut card, _) = s.review(&new_card(t0()), Rating::Easy, t0(), None);
        let before = card.memory.unwrap().stability;

        let later = card.due;
        let (after, log) = s.review(&card, Rating::Again, later, None);

        assert_eq!(after.state, CardState::Relearning);
        assert_eq!(after.lapses, 1);
        assert!(
            after.memory.unwrap().stability < before,
            "忘記後 stability 應下降：{} -> {}",
            before,
            after.memory.unwrap().stability
        );
        assert_eq!(log.rating, Rating::Again);

        card = after;
        let (recovered, _) = s.review(&card, Rating::Good, card.due, None);
        assert_eq!(recovered.state, CardState::Review);
    }

    /// 難度永遠被夾在 1..=10。
    #[test]
    fn difficulty_stays_in_range_under_pressure() {
        let s = Scheduler::default();
        let mut card = new_card(t0());
        let mut now = t0();
        for i in 0..40 {
            let rating = if i % 2 == 0 {
                Rating::Again
            } else {
                Rating::Easy
            };
            let (next, _) = s.review(&card, rating, now, None);
            let d = next.memory.unwrap().difficulty;
            assert!((1.0..=10.0).contains(&d), "difficulty 越界：{d}");
            now = next.due;
            card = next;
        }
    }

    #[test]
    fn rejects_invalid_retention() {
        let bad = Scheduler::new(
            FsrsParams::default(),
            SchedulerConfig {
                desired_retention: 1.0,
                ..Default::default()
            },
        );
        assert!(bad.is_err());
    }

    #[test]
    fn rejects_wrong_param_count() {
        assert!(FsrsParams::from_slice(&[0.1, 0.2]).is_err());
        assert!(FsrsParams::from_slice(&FsrsParams::default().0).is_ok());
    }
}
