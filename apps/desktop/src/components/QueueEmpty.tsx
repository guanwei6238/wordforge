import { useState } from "react";
import { errorMessage, type QueueStatus, studyMore, unsuspendCards } from "../api";

interface Props {
  status: QueueStatus;
  /** 使用者選擇繼續學習後，重新載入佇列 */
  onResume: () => void;
}

function formatDue(iso: string): string {
  const due = new Date(iso);
  const days = Math.ceil((due.getTime() - Date.now()) / 86_400_000);
  if (days <= 0) return "稍後";
  if (days === 1) return "明天";
  return `${days} 天後（${due.toLocaleDateString("zh-TW")}）`;
}

/**
 * 佇列空了的畫面。
 *
 * 「今天的份做完了」只有在**真的做完**時才該出現。分級測驗把整個牌組收起來、
 * 或牌組根本是空的，都會讓佇列變空，但原因完全不同、該做的事也不同。
 * 一律顯示同一句話會讓人以為系統壞了。
 */
export default function QueueEmpty({ status, onResume }: Props) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function more(extra: number) {
    setBusy(true);
    setError(null);
    try {
      await studyMore(extra);
      onResume();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  async function restore(count: number) {
    setBusy(true);
    setError(null);
    try {
      await unsuspendCards(count);
      onResume();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  // 今天的額度用完，但牌組裡還有字：這才是「做完了」
  const quotaReached = status.new_in_deck > 0 && status.new_today === 0;
  // 牌組裡沒有可學的字，但有一堆被收起來
  const allSuspended = status.new_in_deck === 0 && status.suspended > 0;
  // 牌組是空的
  const deckEmpty = status.new_in_deck === 0 && status.suspended === 0;

  return (
    <section className="done">
      {quotaReached && (
        <>
          <p>今天的 {status.new_per_day} 個新字學完了 🎉</p>
          <p className="muted">
            每天固定少量、隔天再複習，才是記得住的方式。牌組裡還有{" "}
            {status.new_in_deck.toLocaleString()} 個字在排隊。
          </p>
          <div className="row">
            <button onClick={() => more(10)} disabled={busy}>
              再學 10 個
            </button>
            <button onClick={() => more(30)} disabled={busy}>
              再學 30 個
            </button>
          </div>
        </>
      )}

      {allSuspended && (
        <>
          <p>目前沒有可以學的字</p>
          <p className="muted">
            牌組裡的 {status.suspended.toLocaleString()} 張卡都被分級測驗判定「你已經會了」
            而收起來。如果覺得判斷不準，可以恢復一部分；或者到「牌組」頁加入
            更符合你程度的範圍（學測、多益、雅思…）。
          </p>
          <div className="row">
            <button onClick={() => restore(50)} disabled={busy}>
              恢復 50 張（最常用的優先）
            </button>
            <button onClick={() => restore(status.suspended)} disabled={busy}>
              全部恢復
            </button>
          </div>
        </>
      )}

      {deckEmpty && (
        <>
          <p>牌組是空的</p>
          <p className="muted">到「牌組」頁選一個範圍加入單字，就可以開始了。</p>
        </>
      )}

      {status.due_reviews === 0 && status.next_due && (
        <p className="muted">
          下一批複習：{formatDue(status.next_due)}。
          間隔是依你答題的表現算出來的，提早複習反而會讓記憶效果打折。
        </p>
      )}

      {error && <p className="error">{error}</p>}
    </section>
  );
}
