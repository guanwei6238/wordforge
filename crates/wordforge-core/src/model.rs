//! 跨層共用的領域型別。
//!
//! 這些型別對應 `wordforge-db` 的資料表，但刻意不帶任何 sqlx 標記，
//! 讓核心層可以在沒有資料庫的情況下被測試。

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// 詞條（lemma）的識別碼。一個 lemma 是「字典會收錄的那個形式」，
/// 例如 `run`；`running`、`ran` 是它的表面形（surface form）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LemmaId(pub i64);

/// 學習者 profile。單機版預設只有一個，但保留多 profile 以支援
/// 「同一台電腦上不同人 / 同一人學不同語言」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProfileId(pub i64);

/// 卡片識別碼。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CardId(pub i64);

/// 同一個單字可以有多張卡，分別訓練不同能力。
///
/// 這是刻意的設計：「看得懂 apple」和「講得出 apple」是兩種不同的記憶強度，
/// 混成一張卡會讓排程失準。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardKind {
    /// 看到目標語 → 想出意思（被動、閱讀用）
    Recognition,
    /// 看到母語 → 想出目標語（主動、口說寫作用）
    Recall,
    /// 聽到發音 → 辨識出字（聽力）
    Listening,
    /// 聽到發音 → 拼出來（拼寫）
    Spelling,
}

impl CardKind {
    pub const ALL: [CardKind; 4] = [
        CardKind::Recognition,
        CardKind::Recall,
        CardKind::Listening,
        CardKind::Spelling,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            CardKind::Recognition => "recognition",
            CardKind::Recall => "recall",
            CardKind::Listening => "listening",
            CardKind::Spelling => "spelling",
        }
    }
}

/// 卡片在學習流程中的階段。與 Anki / FSRS 的定義一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardState {
    /// 還沒學過
    New,
    /// 首次學習中（分鐘級的 learning steps）
    Learning,
    /// 已進入長期複習（天級間隔）
    Review,
    /// 忘記後重新學習中
    Relearning,
}

impl CardState {
    pub fn as_str(&self) -> &'static str {
        match self {
            CardState::New => "new",
            CardState::Learning => "learning",
            CardState::Review => "review",
            CardState::Relearning => "relearning",
        }
    }
}

/// 複習時使用者的自評。FSRS 的四級評分。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum Rating {
    /// 完全想不起來
    Again = 1,
    /// 想起來了但很吃力
    Hard = 2,
    /// 正常想起來
    Good = 3,
    /// 毫不猶豫
    Easy = 4,
}

impl Rating {
    /// FSRS 公式裡的 G（grade），值域 1..=4。
    pub fn grade(self) -> f64 {
        self as u8 as f64
    }

    pub fn is_forget(self) -> bool {
        matches!(self, Rating::Again)
    }

    pub fn from_i64(v: i64) -> Option<Self> {
        match v {
            1 => Some(Rating::Again),
            2 => Some(Rating::Hard),
            3 => Some(Rating::Good),
            4 => Some(Rating::Easy),
            _ => None,
        }
    }
}

/// FSRS 的記憶狀態：穩定度與難度。
///
/// - `stability`：記憶可以維持多少天而保有 90% 回憶率
/// - `difficulty`：這張卡本質上有多難，值域 1..=10
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MemoryState {
    pub stability: f64,
    pub difficulty: f64,
}

/// 一張學習卡的完整狀態。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Card {
    pub id: Option<CardId>,
    pub profile_id: ProfileId,
    pub lemma_id: LemmaId,
    pub kind: CardKind,
    pub state: CardState,
    /// 新卡為 `None`，第一次複習後才有值。
    pub memory: Option<MemoryState>,
    pub due: OffsetDateTime,
    pub last_review: Option<OffsetDateTime>,
    /// 目前位於第幾個 learning / relearning step。
    /// 只有 `Learning` 與 `Relearning` 狀態下有意義。
    pub step: u8,
    /// 累計複習次數（含當日重複）
    pub reps: u32,
    /// 累計遺忘次數（Review 狀態下按 Again）
    pub lapses: u32,
    /// 上次排程給出的間隔（天）
    pub scheduled_days: i64,
    pub suspended: bool,
}

impl Card {
    /// 建立一張尚未學習的新卡，立刻到期。
    pub fn new(
        profile_id: ProfileId,
        lemma_id: LemmaId,
        kind: CardKind,
        now: OffsetDateTime,
    ) -> Self {
        Self {
            id: None,
            profile_id,
            lemma_id,
            kind,
            state: CardState::New,
            memory: None,
            due: now,
            last_review: None,
            step: 0,
            reps: 0,
            lapses: 0,
            scheduled_days: 0,
            suspended: false,
        }
    }

    pub fn is_due(&self, now: OffsetDateTime) -> bool {
        !self.suspended && self.due <= now
    }
}

/// 一次複習的完整紀錄。
///
/// 保留所有輸入欄位是為了日後能用使用者自己的複習歷程重新訓練 FSRS 參數
/// （FSRS 的 optimizer 需要這些欄位）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewLog {
    pub card_id: Option<CardId>,
    pub rating: Rating,
    /// 複習「之前」的狀態
    pub state: CardState,
    /// 複習「之後」的記憶狀態
    pub memory: MemoryState,
    /// 距離上次複習經過的天數
    pub elapsed_days: i64,
    /// 複習後排定的間隔（天）
    pub scheduled_days: i64,
    pub reviewed_at: OffsetDateTime,
    /// 作答花費時間，用於偵測「其實不會只是猜對」
    pub duration_ms: Option<u32>,
}
