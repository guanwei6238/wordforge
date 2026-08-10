# 間隔重複：FSRS-5

實作在 [`crates/wordforge-core/src/srs/fsrs.rs`](../crates/wordforge-core/src/srs/fsrs.rs)。

## 為什麼不用 SM-2

Anki 預設的 SM-2（1987 年）用「連續答對次數 × ease factor」推算間隔。
它假設每張卡的遺忘曲線形狀相同，只有速度不同。實際上不是——
`the` 和 `ubiquitous` 的遺忘方式差很多。

FSRS 用三個變數建模：

| 變數 | 意義 |
| --- | --- |
| **S** stability | 記憶能維持多少天而仍有 90% 想得起來 |
| **D** difficulty | 這張卡本質上多難，1~10 |
| **R** retrievability | 此刻還記得的機率 |

在相同的記憶留存率下，FSRS 的複習量通常比 SM-2 少 20~30%。

## 遺忘曲線

```
R(t, S) = (1 + FACTOR · t / S)^DECAY
```

其中 `DECAY = -0.5`，`FACTOR = 0.9^(1/DECAY) - 1 = 19/81`。

這兩個常數的關係讓 `R(t = S) = 0.9` 恰好成立——
這正是 stability 的定義。程式裡有一個測試就在驗這件事。

反過來，要讓記憶掉到目標留存率 `r`，該排的間隔是：

```
I(r, S) = S / FACTOR · (r^(1/DECAY) - 1)
```

在預設 `r = 0.9` 時，`I = S`。

## 目標留存率的取捨

`SchedulerConfig::desired_retention` 預設 0.9。

| 設定 | 效果 |
| --- | --- |
| 0.97 | 記得很牢，但複習次數大幅增加 |
| 0.90 | 平衡點，大多數人適用 |
| 0.80 | 複習量少，但遺忘明顯變多 |

留存率與總複習量不是線性關係：從 0.9 拉到 0.97，複習量大約會翻倍。
準備考試前可以調高，長期維持則不建議超過 0.92。

## 狀態機

```
        Again                Good ×N
  New ──────→ Learning ─────────────→ Review
                 ↑                       │
                 │                       │ Again
                 └──── Relearning ←──────┘
                          Good
```

- **learning steps**：預設 1 分鐘、10 分鐘。當天真的記起來才畢業。
- **relearning steps**：預設 10 分鐘。
- 只有在 `Review` 狀態按 `Again` 才算一次 lapse。學習階段答錯是正常的。

## 同日重複複習

間隔不足一天的複習用獨立公式：

```
S' = S · exp(w17 · (G - 3 + w18))
```

因為「五分鐘前才看過」提供的記憶強度資訊，遠低於「三週後還記得」。
用同一條公式會嚴重高估 stability。

## 權重

`FsrsParams::default()` 是 FSRS-5 官方預設值，由大規模匿名複習資料訓練而得。
`review_log` 表保留了重新訓練所需的全部欄位，累積約 1000 次複習後
可以針對個人重新訓練。這部分尚未實作，見 roadmap。

## 測試涵蓋

`fsrs.rs` 的測試不驗證特定數值，而是驗證**行為性質**：

- `R(t = S) = 0.9`
- 目標留存率越高，間隔越短
- 連續答對，間隔單調遞增
- 忘記之後 stability 下降、lapses 增加、狀態退回 relearning
- 在 40 次交替 Again / Easy 的極端輸入下，difficulty 仍夾在 1~10

這樣寫的原因：權重之後可能會換成個人化訓練的結果，
硬編數值的測試會全部失效，但性質不會變。

## 參考

- [FSRS 演算法說明](https://github.com/open-spaced-repetition/fsrs4anki/wiki/The-Algorithm)
- [open-spaced-repetition/fsrs-rs](https://github.com/open-spaced-repetition/fsrs-rs)（官方 Rust 實作，含 optimizer）
