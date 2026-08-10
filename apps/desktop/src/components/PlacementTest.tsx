import { useState } from "react";
import {
  errorMessage,
  type PlacementAnswer,
  type PlacementItem,
  placementItems,
  type PlacementOutcome,
  submitPlacement,
} from "../api";

interface Props {
  /** 測驗結束後通知外層重新載入牌組統計 */
  onFinished: () => void;
}

/**
 * 分級測驗。
 *
 * 從各個詞頻層各抽幾個字問「認不認識」，用認識率推估詞彙量，
 * 決定新卡要從哪裡開始排。學過幾年英文的人不必從 the、go、make 重來。
 *
 * 作答後才顯示中文意思——先看到答案就沒有測到任何東西了。
 */
export default function PlacementTest({ onFinished }: Props) {
  const [items, setItems] = useState<PlacementItem[] | null>(null);
  const [index, setIndex] = useState(0);
  const [answers, setAnswers] = useState<PlacementAnswer[]>([]);
  const [revealed, setRevealed] = useState(false);
  const [outcome, setOutcome] = useState<PlacementOutcome | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  async function start() {
    setLoading(true);
    setError(null);
    setOutcome(null);
    try {
      const fetched = await placementItems();
      if (fetched.length === 0) {
        setError("字典裡還沒有詞頻資料，無法出題。請先匯入 ECDICT 或詞頻表。");
        return;
      }
      setItems(fetched);
      setIndex(0);
      setAnswers([]);
      setRevealed(false);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setLoading(false);
    }
  }

  async function answer(known: boolean) {
    if (!items) return;
    const next = [...answers, { band_index: items[index].band_index, known }];
    setAnswers(next);
    setRevealed(false);

    if (index + 1 < items.length) {
      setIndex(index + 1);
      return;
    }

    // 最後一題：送出並顯示結果
    try {
      setLoading(true);
      setOutcome(await submitPlacement(next));
      setItems(null);
      onFinished();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setLoading(false);
    }
  }

  if (error) {
    return (
      <section className="panel">
        <h2>分級測驗</h2>
        <p className="error">{error}</p>
        <div className="row">
          <button onClick={start}>再試一次</button>
        </div>
      </section>
    );
  }

  if (outcome) {
    return (
      <section className="panel">
        <h2>測驗結果</h2>
        <p>
          估計你掌握約 <strong>{outcome.estimated_vocabulary.toLocaleString()}</strong> 個英文單字。
        </p>
        <ul className="bands">
          {outcome.band_rates.map(([band, rate]) => (
            <li key={band.start_rank}>
              <span className="band-label">
                {band.start_rank.toLocaleString()}–{band.end_rank.toLocaleString()}
              </span>
              <progress value={rate} max={1} />
              <span className="muted band-rate">{Math.round(rate * 100)}%</span>
            </li>
          ))}
        </ul>
        <p className="muted">
          之後加入的新字會從第 {outcome.start_rank.toLocaleString()} 名開始。
          {outcome.suspended_cards > 0 &&
            ` 牌組裡有 ${outcome.suspended_cards} 張太簡單的卡已經收起來（沒有刪除，之後想學可以恢復）。`}
        </p>
        <p className="muted">
          這是自評測驗，看起來眼熟就按認識的話會高估。覺得結果不準可以重測。
        </p>
        <div className="row">
          <button onClick={start}>重新測驗</button>
        </div>
      </section>
    );
  }

  if (!items) {
    return (
      <section className="panel">
        <h2>分級測驗</h2>
        <p className="muted">
          從各個難度抽 35 個字問你認不認識，用來估計詞彙量、決定新字從哪裡開始排。
          大約三分鐘，之後隨時可以重測。
        </p>
        <div className="row">
          <button className="primary" onClick={start} disabled={loading}>
            {loading ? "準備中…" : "開始測驗"}
          </button>
        </div>
      </section>
    );
  }

  const item = items[index];
  return (
    <section className="panel placement">
      <h2>
        分級測驗
        <span className="muted">
          {" "}
          {index + 1} / {items.length}
        </span>
      </h2>
      <progress value={index} max={items.length} />

      <p className="placement-word">{item.text}</p>

      {revealed ? (
        <p className="placement-meaning">{item.translation ?? "（沒有翻譯）"}</p>
      ) : (
        <button className="link" onClick={() => setRevealed(true)}>
          先看意思再判斷
        </button>
      )}

      <div className="row">
        <button className="primary" onClick={() => answer(true)} disabled={loading}>
          認識
        </button>
        <button onClick={() => answer(false)} disabled={loading}>
          不認識
        </button>
      </div>
      <p className="muted">
        照直覺回答就好。說得出大概意思就算認識，只是看起來眼熟不算。
      </p>
    </section>
  );
}
