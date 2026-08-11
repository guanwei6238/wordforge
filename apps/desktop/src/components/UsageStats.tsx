import { useCallback, useEffect, useState } from "react";
import { errorMessage, llmUsage, PURPOSE_LABELS, type UsageSummary } from "../api";

/** 幾萬個字元直接印出來看不懂，換成 k 比較好比較。 */
function chars(n: number): string {
  return n >= 10_000 ? `${(n / 1000).toFixed(0)}k` : n.toLocaleString();
}

/**
 * AI 用量。
 *
 * ## 為什麼主要顯示字元數而不是 token
 *
 * 拿不拿得到 token 取決於後端：HTTP API 的回應裡就有，但
 * `claude -p` 與 `codex exec` 的輸出只有內容，沒有任何用量資訊。
 *
 * 把字元數乘個係數謊報成 token 只會讓人做出錯誤判斷。字元數是
 * 每個後端都量得到的東西，要回答「我今天用了多少」已經夠了；
 * 後端真的有回報 token 時再額外顯示。
 */
export default function UsageStats() {
  const [today, setToday] = useState<UsageSummary | null>(null);
  const [week, setWeek] = useState<UsageSummary | null>(null);
  const [purposes, setPurposes] = useState<[string, number, number][]>([]);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const [t, w, p] = await llmUsage();
      setToday(t);
      setWeek(w);
      setPurposes(p);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  if (!today || !week) {
    return null;
  }

  return (
    <section className="panel">
      <h2>AI 用量</h2>

      {today.calls === 0 && week.calls === 0 ? (
        <p className="muted">還沒用過 AI 出題。到「練習」頁試一題就會出現在這裡。</p>
      ) : (
        <>
          <div className="usage-row">
            <div>
              <span className="usage-label">今天</span>
              <strong>{today.calls}</strong> 次呼叫 · 送出 {chars(today.prompt_chars)} 字元 · 收到{" "}
              {chars(today.response_chars)} 字元
              {today.failed > 0 && <span className="error"> · {today.failed} 次失敗</span>}
            </div>
            <div>
              <span className="usage-label">近七天</span>
              <strong>{week.calls}</strong> 次呼叫 · 送出 {chars(week.prompt_chars)} 字元 · 收到{" "}
              {chars(week.response_chars)} 字元
            </div>
          </div>

          {purposes.length > 0 && (
            <p className="muted hint">
              今天用在：
              {purposes
                .map(([p, n, c]) => `${PURPOSE_LABELS[p] ?? p} ${n} 次（${chars(c)} 字元）`)
                .join("、")}
            </p>
          )}

          {today.input_tokens !== null || week.input_tokens !== null ? (
            <p className="muted hint">
              後端回報的 token（近七天）：輸入 {week.input_tokens?.toLocaleString() ?? "—"}、 輸出{" "}
              {week.output_tokens?.toLocaleString() ?? "—"}
              {week.calls_with_tokens < week.calls && (
                <>
                  {" "}
                  ——只涵蓋 {week.calls_with_tokens}/{week.calls} 次呼叫，
                  其餘是不回報用量的 CLI 後端。
                </>
              )}
            </p>
          ) : (
            <p className="muted hint">
              目前的後端不回報 token 數（<code>claude -p</code> 與 <code>codex exec</code>
              的輸出只有內容）。 字元數是每個後端都量得到的，拿來比較「哪天用得比較兇」一樣有效。
            </p>
          )}
        </>
      )}

      {error && <p className="error">{error}</p>}
    </section>
  );
}
